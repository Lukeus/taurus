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
//! # How far it outlives the turn
//!
//! The board is held per *session*, in `Host`, so an unfinished plan survives
//! into the next message. It has to: a six-step task is very often six steps
//! and a question in the middle of it, and a checklist that evaporates the
//! moment you answer "yes, go on" is one the model then rebuilds from memory —
//! which is the drift this exists to stop, arriving one turn later.
//!
//! What does not survive is a *finished* plan. [`PlanBoard::start_turn`] clears
//! it, and that is the whole staleness rule: every step marked `[x]` means the
//! work it described is over, and restating it would tell a model asked about
//! something else that its instructions are "say what you did and stop". An
//! unfinished plan is carried and labelled as carried, so the model can see it
//! belongs to an earlier message and replace it if the request has moved on.
//! That judgement is the model's; the harness cannot read a follow-up and know
//! whether it continues the task or changes it.
//!
//! The rule is the same one the pinned panel in the desktop app follows, which
//! is not a coincidence: what the reader sees above the composer and what the
//! model reads in its prompt are the same checklist, and they would be worse
//! than useless if they disagreed about whether it was still live.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::view::{Step, StepState};

/// The plan for one conversation, shared between the tool that writes it and
/// the agent loop that reads it back.
///
/// Cloning shares the board, the way cloning a [`crate::ToolContext`] shares
/// its checkpoint recorder — but unlike that one, this is deliberately *not*
/// handed to sub-agents. A delegate keeping its own steps in the parent's
/// checklist would report progress against a task nobody asked it to own.
#[derive(Clone, Default)]
pub struct PlanBoard {
    /// A blocking lock rather than `tokio`'s, because one reader cannot await:
    /// [`crate::builtin::plan::UpdatePlan::view`] draws the card, and it has to
    /// check the incoming list against the board to do it — see
    /// [`crate::builtin::plan`] for why that check has to run in both places or
    /// not at all. Every critical section here is a clone or a walk of a list a
    /// dozen items long, and none of them holds the guard across an await, so
    /// the blocking lock costs nothing.
    state: Arc<Mutex<Board>>,
}

#[derive(Default)]
struct Board {
    steps: Vec<Step>,
    /// Whether these steps were written before the current turn began. Only
    /// ever changes at a turn boundary; see [`PlanBoard::start_turn`].
    carried: bool,
}

impl PlanBoard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the plan. Returns what the tool should tell the model it did.
    pub fn set(&self, steps: Vec<Step>) -> String {
        let summary = summarize(&steps);
        let mut board = self.board();
        board.steps = steps;
        // Written in this turn, so it is no longer somebody else's checklist.
        board.carried = false;
        summary
    }

    pub fn steps(&self) -> Vec<Step> {
        self.board().steps.clone()
    }

    /// Whether a plan is up and something on it is still open.
    ///
    /// The question the agent loop asks when the model stops: a `true` here is
    /// a turn ending with work the checklist still says is in progress. See
    /// `PLAN_NUDGE` in `taurus_core::agent`.
    pub fn unfinished(&self) -> bool {
        let board = self.board();
        !board.steps.is_empty() && board.steps.iter().any(|s| s.state != StepState::Done)
    }

    /// Poisoning here would mean a panic while holding a lock over a list walk
    /// that cannot panic. Taking the value back is better than bringing the
    /// session down over a checklist.
    fn board(&self) -> MutexGuard<'_, Board> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Readies the board for a new turn, and reports whether a plan survived.
    ///
    /// A finished plan is dropped: the work it described is over, and a model
    /// asked about something else would otherwise read "when every step is [x],
    /// say what you did and stop" as its instructions for the new request.
    /// Anything unfinished is kept and marked as carried — see the module doc
    /// for why that judgement belongs to the model rather than here.
    pub fn start_turn(&self) -> bool {
        let mut board = self.board();
        if board.steps.is_empty() {
            return false;
        }
        if board.steps.iter().all(|s| s.state == StepState::Done) {
            board.steps.clear();
            board.carried = false;
            return false;
        }
        board.carried = true;
        true
    }

    /// The plan as it goes into the system prompt, or `None` when there is no
    /// plan yet.
    ///
    /// `None` rather than an empty string so the caller appends nothing at all:
    /// a heading over no steps would be a standing instruction to a model that
    /// has not been asked to make a plan, which is how a turn that needed two
    /// steps ends up with a checklist for them.
    pub fn reminder(&self) -> Option<String> {
        let board = self.board();
        if board.steps.is_empty() {
            return None;
        }

        let mut out = String::from("# Your current plan\n\n");
        out.push_str(if board.carried {
            "You wrote this with update_plan earlier in this conversation, before the message you \
             are answering now. It is restated here every time because it is the record of where \
             you are:\n\n"
        } else {
            "You wrote this with update_plan. It is restated here every time because it is the \
             record of where you are:\n\n"
        });
        for (index, step) in board.steps.iter().enumerate() {
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
             until it is [x]. Closing the last step is a call to update_plan, not a sentence in \
             your reply: when the work is done, send the whole list back with every step 'done' \
             first, and then say what you did and stop.",
        );
        if board.carried {
            // The one thing the harness cannot decide. A follow-up may continue
            // the task or change the subject, and only the model has read it.
            out.push_str(
                "\n\nIf this message asks for something the plan above does not cover, call \
                 update_plan with the new list instead of working the old one.",
            );
        }
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

    #[test]
    fn an_empty_board_adds_nothing_to_the_prompt() {
        // Not an empty section — nothing. A heading over no steps is a standing
        // instruction to plan, given to a turn that was never asked to.
        assert_eq!(PlanBoard::new().reminder(), None);
    }

    #[test]
    fn the_reminder_carries_every_step_and_its_state() {
        let board = PlanBoard::new();
        board.set(vec![
            step("Read the parser", StepState::Done),
            step("Add the token type", StepState::Active),
            step("Update the tests", StepState::Todo),
        ]);

        let reminder = board.reminder().expect("a plan is set");
        assert!(reminder.contains("[x] Read the parser"), "{reminder}");
        assert!(reminder.contains("[>] Add the token type"), "{reminder}");
        assert!(reminder.contains("[ ] Update the tests"), "{reminder}");
        // Numbered, so the model can refer to a step without quoting it.
        assert!(reminder.contains("2. [>]"), "{reminder}");
    }

    #[test]
    fn the_result_names_the_step_being_worked_on() {
        // What the model reads immediately after the call. The full list
        // arrives in the system prompt, so this is the one fact worth
        // repeating: which step it just said it was on.
        let board = PlanBoard::new();
        let result = board.set(vec![
            step("Read the parser", StepState::Done),
            step("Add the token type", StepState::Active),
        ]);
        assert!(result.contains("1 of 2"), "{result}");
        assert!(result.contains("Add the token type"), "{result}");
    }

    #[test]
    fn a_finished_plan_says_so_rather_than_reporting_no_progress() {
        // "No step is marked in progress" is true of a finished plan and is
        // exactly the wrong thing to tell a model that has just finished one.
        let board = PlanBoard::new();
        let result = board.set(vec![
            step("Read the parser", StepState::Done),
            step("Add the token type", StepState::Done),
        ]);
        assert!(result.contains("Every step is done"), "{result}");
    }

    #[test]
    fn an_unfinished_plan_survives_into_the_next_turn() {
        // The case this exists for: six steps and a question in the middle.
        // A checklist that evaporates when you answer "yes, go on" is one the
        // model rebuilds from memory, which is the drift it exists to stop.
        let board = PlanBoard::new();
        board.set(vec![
            step("Read the parser", StepState::Done),
            step("Add the token type", StepState::Active),
        ]);

        assert!(board.start_turn(), "a plan survived");
        let reminder = board.reminder().expect("and is still restated");
        assert!(reminder.contains("[>] Add the token type"), "{reminder}");
    }

    #[test]
    fn a_carried_plan_says_it_came_from_an_earlier_message() {
        // Two things the model cannot work out for itself: that it wrote this
        // before the message it is answering, and that replacing it is allowed
        // when the request has moved on.
        let board = PlanBoard::new();
        board.set(vec![step("One", StepState::Active)]);

        let fresh = board.reminder().unwrap();
        assert!(!fresh.contains("earlier in this conversation"), "{fresh}");

        board.start_turn();
        let carried = board.reminder().unwrap();
        assert!(
            carried.contains("earlier in this conversation"),
            "{carried}"
        );
        assert!(
            carried.contains("call update_plan with the new list"),
            "{carried}"
        );
    }

    #[test]
    fn a_finished_plan_does_not_survive_into_the_next_turn() {
        // The whole staleness rule. Restating a completed checklist would tell
        // a model asked about something else that its instructions are "say
        // what you did and stop".
        let board = PlanBoard::new();
        board.set(vec![
            step("One", StepState::Done),
            step("Two", StepState::Done),
        ]);

        assert!(!board.start_turn(), "nothing survived");
        assert_eq!(board.reminder(), None);
        assert!(board.steps().is_empty());
    }

    #[test]
    fn a_turn_that_writes_a_plan_stops_calling_it_carried() {
        // Otherwise a plan written in turn two would keep apologizing for
        // belonging to turn one for the rest of the conversation.
        let board = PlanBoard::new();
        board.set(vec![step("One", StepState::Active)]);
        board.start_turn();
        board.set(vec![
            step("One", StepState::Done),
            step("Two", StepState::Active),
        ]);

        let reminder = board.reminder().unwrap();
        assert!(
            !reminder.contains("earlier in this conversation"),
            "{reminder}"
        );
    }

    #[test]
    fn a_board_knows_when_something_on_it_is_still_open() {
        // What the agent loop asks when the model stops talking. The case it
        // exists for is the last one: every step done but the one being worked,
        // which is where a turn ends when the model reports finishing in prose
        // instead of closing the list.
        let board = PlanBoard::new();
        assert!(!board.unfinished(), "no plan is not an unfinished plan");

        board.set(vec![step("One", StepState::Done)]);
        assert!(!board.unfinished());

        board.set(vec![
            step("One", StepState::Done),
            step("Two", StepState::Active),
        ]);
        assert!(board.unfinished());
    }

    #[test]
    fn the_reminder_says_closing_the_list_is_a_call_rather_than_a_sentence() {
        // The failure this wording is aimed at: a model that finishes the work,
        // writes "all three steps are done", and never sends the list back — so
        // the panel reads 2 of 3 forever.
        let board = PlanBoard::new();
        board.set(vec![step("One", StepState::Active)]);

        let reminder = board.reminder().unwrap();
        assert!(
            reminder.contains("not a sentence in your reply"),
            "{reminder}"
        );
    }

    #[test]
    fn starting_a_turn_on_an_empty_board_is_a_no_op() {
        let board = PlanBoard::new();
        assert!(!board.start_turn());
        assert_eq!(board.reminder(), None);
    }

    #[test]
    fn replacing_the_plan_replaces_it_rather_than_appending() {
        // Whole-list replacement is the contract. A board that accumulated
        // would grow a second copy of every step on the first update.
        let board = PlanBoard::new();
        board.set(vec![step("One", StepState::Todo)]);
        board.set(vec![
            step("One", StepState::Done),
            step("Two", StepState::Active),
        ]);

        assert_eq!(board.steps().len(), 2);
        let reminder = board.reminder().unwrap();
        assert!(!reminder.contains("[ ] One"), "{reminder}");
        assert!(reminder.contains("[x] One"), "{reminder}");
    }
}
