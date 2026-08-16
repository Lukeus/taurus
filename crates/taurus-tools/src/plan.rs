//! The checklist a turn is working through, held where both ends can see it.
//!
//! A frontier model keeps a six-step task in its head. A 9B model does not: by
//! iteration nine it has read four files, fixed two compile errors, and lost
//! the thread — not because it forgot the *steps*, which are still up there in
//! the message history, but because they are now twenty messages back, behind a
//! wall of tool output, competing with everything else for attention.
//!
//! So the plan is not left in the history. It is held here, and restated in the
//! system prompt on **every** iteration, where it is the last thing the model
//! reads before deciding what to do next. That is the whole of the mechanism,
//! and it is cheap: a plan of six steps is about sixty tokens, paid per
//! iteration, against a turn that would otherwise wander for the rest of its
//! budget. A turn with no plan pays nothing at all — the reminder is absent
//! rather than empty.
//!
//! # Why the whole list, every time
//!
//! [`crate::builtin::plan::UpdatePlan`] replaces the plan wholesale rather than
//! patching one step. There is no step id to quote back, no add/complete/remove
//! protocol to get half right, and no way to reach a state the model did not
//! literally write out — which matters far more for a small model than the
//! tokens it costs. It also makes the view identical to its own input, which is
//! what lets a reopened conversation redraw a plan card it never computed. See
//! [`crate::view`].
//!
//! # Why it does not outlive the turn
//!
//! The board is built with the turn, in `Host::build_agent`, so a plan is gone
//! when the turn that made it ends. That is the scope the problem has: a
//! six-step task is one turn with many iterations, and the drift this exists to
//! stop happens inside it. What the *next* turn sees is the plan card still
//! sitting in the transcript, which is the same thing every other tool call
//! leaves behind.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::view::{Step, StepState};

/// The plan for one turn, shared between the tool that writes it and the agent
/// loop that reads it back.
///
/// Cloning shares the board, the way cloning a [`crate::ToolContext`] shares
/// its checkpoint recorder — but unlike that one, this is deliberately *not*
/// handed to sub-agents. A delegate keeping its own steps in the parent's
/// checklist would report progress against a task nobody asked it to own.
#[derive(Clone, Default)]
pub struct PlanBoard {
    steps: Arc<Mutex<Vec<Step>>>,
}

impl PlanBoard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the plan. Returns what the tool should tell the model it did.
    pub async fn set(&self, steps: Vec<Step>) -> String {
        let summary = summarize(&steps);
        *self.steps.lock().await = steps;
        summary
    }

    pub async fn steps(&self) -> Vec<Step> {
        self.steps.lock().await.clone()
    }

    /// The plan as it goes into the system prompt, or `None` when there is no
    /// plan yet.
    ///
    /// `None` rather than an empty string so the caller appends nothing at all:
    /// a heading over no steps would be a standing instruction to a model that
    /// has not been asked to make a plan, which is how a turn that needed two
    /// steps ends up with a checklist for them.
    pub async fn reminder(&self) -> Option<String> {
        let steps = self.steps.lock().await;
        if steps.is_empty() {
            return None;
        }

        let mut out = String::from(
            "# Your current plan\n\nYou wrote this with update_plan. It is restated here every \
             time because it is the record of where you are:\n\n",
        );
        for (index, step) in steps.iter().enumerate() {
            out.push_str(&format!(
                "{}. {} {}\n",
                index + 1,
                step.state.marker(),
                step.text
            ));
        }
        out.push_str(
            "\nCall update_plan again the moment a step's state changes — send the whole list \
             back with the states updated. Work the step marked [>], and do not start another \
             until it is [x]. When every step is [x], say what you did and stop.",
        );
        Some(out)
    }
}

/// One line saying where the plan stands, for the tool result.
///
/// The tool result is deliberately not the whole list: the model is about to
/// read the whole list in the system prompt anyway, and sending it twice per
/// update buys nothing but tokens.
fn summarize(steps: &[Step]) -> String {
    let done = steps.iter().filter(|s| s.state == StepState::Done).count();
    let active = steps.iter().find(|s| s.state == StepState::Active);

    let mut line = format!("Plan updated: {done} of {} steps done.", steps.len());
    match active {
        Some(step) => line.push_str(&format!(" Now working on: {}", step.text)),
        None if done == steps.len() => line.push_str(" Every step is done."),
        // Legal, and worth naming: it is the state between finishing one step
        // and picking up the next, and a model that stays here is a model that
        // has stopped working the plan.
        None => line.push_str(" No step is marked in progress."),
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(text: &str, state: StepState) -> Step {
        Step {
            text: text.into(),
            state,
            active_form: None,
        }
    }

    #[tokio::test]
    async fn an_empty_board_adds_nothing_to_the_prompt() {
        // Not an empty section — nothing. A heading over no steps is a standing
        // instruction to plan, given to a turn that was never asked to.
        assert_eq!(PlanBoard::new().reminder().await, None);
    }

    #[tokio::test]
    async fn the_reminder_carries_every_step_and_its_state() {
        let board = PlanBoard::new();
        board
            .set(vec![
                step("Read the parser", StepState::Done),
                step("Add the token type", StepState::Active),
                step("Update the tests", StepState::Todo),
            ])
            .await;

        let reminder = board.reminder().await.expect("a plan is set");
        assert!(reminder.contains("[x] Read the parser"), "{reminder}");
        assert!(reminder.contains("[>] Add the token type"), "{reminder}");
        assert!(reminder.contains("[ ] Update the tests"), "{reminder}");
        // Numbered, so the model can refer to a step without quoting it.
        assert!(reminder.contains("2. [>]"), "{reminder}");
    }

    #[tokio::test]
    async fn the_result_names_the_step_being_worked_on() {
        // What the model reads immediately after the call. The full list
        // arrives in the system prompt, so this is the one fact worth
        // repeating: which step it just said it was on.
        let board = PlanBoard::new();
        let result = board
            .set(vec![
                step("Read the parser", StepState::Done),
                step("Add the token type", StepState::Active),
            ])
            .await;
        assert!(result.contains("1 of 2"), "{result}");
        assert!(result.contains("Add the token type"), "{result}");
    }

    #[tokio::test]
    async fn a_finished_plan_says_so_rather_than_reporting_no_progress() {
        // "No step is marked in progress" is true of a finished plan and is
        // exactly the wrong thing to tell a model that has just finished one.
        let board = PlanBoard::new();
        let result = board
            .set(vec![
                step("Read the parser", StepState::Done),
                step("Add the token type", StepState::Done),
            ])
            .await;
        assert!(result.contains("Every step is done"), "{result}");
    }

    #[tokio::test]
    async fn replacing_the_plan_replaces_it_rather_than_appending() {
        // Whole-list replacement is the contract. A board that accumulated
        // would grow a second copy of every step on the first update.
        let board = PlanBoard::new();
        board.set(vec![step("One", StepState::Todo)]).await;
        board
            .set(vec![
                step("One", StepState::Done),
                step("Two", StepState::Active),
            ])
            .await;

        assert_eq!(board.steps().await.len(), 2);
        let reminder = board.reminder().await.unwrap();
        assert!(!reminder.contains("[ ] One"), "{reminder}");
        assert!(reminder.contains("[x] One"), "{reminder}");
    }
}
