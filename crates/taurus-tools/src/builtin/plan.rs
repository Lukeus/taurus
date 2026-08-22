//! The tool that writes the turn's checklist.
//!
//! Sibling of [`crate::builtin::present`] and registered the same way — per
//! turn, never for a sub-agent — but not for the same reason. Those three
//! address the person watching. This one addresses the person *and* the model:
//! what it writes is drawn in the transcript and read back into the system
//! prompt on every iteration. See [`crate::plan`] for why that second half is
//! the point.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::plan::PlanBoard;
use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};
use crate::view::{Step, StepState, TranscriptView};

/// Steps one plan may hold.
///
/// Not a context limit — the reminder is small at any plausible size. It is a
/// judgement about granularity: past a dozen steps the model is writing out the
/// task instead of planning it, and a checklist that long stops being something
/// either party can hold in view. The failure it produces is a model that
/// spends its iterations maintaining the list.
const MAX_STEPS: usize = 12;

/// Longest a single step may be.
///
/// A step is a thing to do, not a paragraph explaining it. The cap is what
/// stops a plan turning into the model's notes, which is where the reminder's
/// cost stops being negligible.
const MAX_STEP_CHARS: usize = 120;

pub const UPDATE_PLAN_TOOL: &str = "update_plan";

#[derive(Deserialize, JsonSchema)]
pub struct UpdatePlanInput {
    /// Every step, in order, with its current state. Always send the whole
    /// list — this replaces the plan rather than adding to it.
    pub steps: Vec<Step>,
}

/// Writes the checklist for this turn.
pub struct UpdatePlan {
    board: PlanBoard,
}

impl UpdatePlan {
    pub fn new(board: PlanBoard) -> Self {
        Self { board }
    }
}

#[async_trait]
impl Tool for UpdatePlan {
    fn name(&self) -> &str {
        UPDATE_PLAN_TOOL
    }

    fn description(&self) -> &str {
        "Write or update the checklist for this task. Use it as soon as a request needs more than \
         two or three steps, before you start the first one, and again every single time a step \
         starts or finishes — the plan is shown to the user and repeated back to you on every \
         step, so it is how you keep track of where you are. Send the complete list every time as \
         'steps', each one an object with 'text' (what is to be done) and 'state' ('todo', \
         'active', or 'done'), like {\"steps\": [{\"text\": \"Add the token type\", \"state\": \
         \"active\", \"active_form\": \"Adding the token type\"}]}. It replaces the previous \
         plan, so anything you leave out is gone. Exactly one step may be 'active' — the one you \
         are working on right now — and a step you have marked 'done' stays 'done' in every list \
         you send after it. Steps are things to do, in order, each a short imperative like \
         'Add the token type'; do not use it for a task that is one action, and do not restate \
         the plan in your reply, since the user is already looking at it. Give every step an \
         'active_form' as well: the same step written as something under way — 'Add the token \
         type' becomes 'Adding the token type' — which is what the user sees while that step is \
         the one running."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<UpdatePlanInput>()
    }

    /// Writes no file and runs nothing. The checklist is a statement about the
    /// conversation, so there is nothing to ask permission for.
    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        match serde_json::from_value::<UpdatePlanInput>(input.clone()) {
            Ok(input) => format!("Plan: {} steps", input.steps.len()),
            Err(_) => "Plan".into(),
        }
    }

    fn view(&self, _id: &str, input: &serde_json::Value) -> Option<TranscriptView> {
        let input: UpdatePlanInput = serde_json::from_value(input.clone()).ok()?;
        // Checked here as well as in `execute` because the view is drawn first.
        // Without it the user would watch a plan with three steps in progress
        // appear, and only then be told the call failed.
        //
        // Both paths read the same board, which is why it is behind a blocking
        // lock — see [`crate::plan::PlanBoard`]. A check that ran only in
        // `execute` would draw a card the call then rejects, and the card is
        // what the pinned panel shows.
        check(&input.steps, &self.board.steps()).ok()?;
        Some(TranscriptView::Plan { steps: input.steps })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: UpdatePlanInput = parse_input(input)?;
        check(&input.steps, &self.board.steps())?;
        Ok(self.board.set(input.steps).into())
    }
}

/// Everything about a plan that has to hold before it is worth keeping.
///
/// Refused rather than repaired. Every one of these is the model having lost
/// track of its own list, and a plan quietly corrected into something it did
/// not write is worse than a rejected call: the checklist it then reads back
/// would not be the one it believes it set.
///
/// `previous` is the plan on the board, which the last two rules are about. A
/// whole-list replacement carries no memory, so the only place a step that
/// silently un-finished itself can be noticed is here, against what it replaced.
fn check(steps: &[Step], previous: &[Step]) -> Result<(), ToolError> {
    if steps.is_empty() {
        return Err(ToolError::InvalidInput(
            "a plan needs at least one step. To finish a plan, send it back with every step \
             marked 'done' rather than sending an empty list"
                .into(),
        ));
    }
    if steps.len() > MAX_STEPS {
        return Err(ToolError::InvalidInput(format!(
            "{} steps is more than a plan can usefully hold ({MAX_STEPS} at most). Group the \
             small ones together — a step should be a thing you finish, not a line you type",
            steps.len()
        )));
    }

    if let Some((n, _)) = steps
        .iter()
        .enumerate()
        .find(|(_, s)| s.text.trim().is_empty())
    {
        return Err(ToolError::InvalidInput(format!(
            "step {} has no text; every step needs to say what is to be done",
            n + 1
        )));
    }

    if let Some((n, step)) = steps
        .iter()
        .enumerate()
        .find(|(_, s)| s.text.chars().count() > MAX_STEP_CHARS)
    {
        return Err(ToolError::InvalidInput(format!(
            "step {} is {} characters, past the {MAX_STEP_CHARS}-character limit. A step is a \
             thing to do, not an explanation of it — put the detail in your reply",
            n + 1,
            step.text.chars().count()
        )));
    }

    // The same cap on the other phrasing, for the same reason: it is the same
    // step said differently, and it is shown in a single line above the list.
    if let Some((n, form)) = steps.iter().enumerate().find_map(|(n, s)| {
        s.active_form
            .as_ref()
            .filter(|f| f.chars().count() > MAX_STEP_CHARS)
            .map(|f| (n, f))
    }) {
        return Err(ToolError::InvalidInput(format!(
            "step {}'s 'active_form' is {} characters, past the {MAX_STEP_CHARS}-character \
             limit. It is the same step as 'Adding the token type', not a longer one",
            n + 1,
            form.chars().count()
        )));
    }

    // The rule that makes the checklist mean something. Three steps in progress
    // says nothing about where the turn is, which is the one question the plan
    // exists to answer.
    let active: Vec<usize> = steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.state == StepState::Active)
        .map(|(n, _)| n + 1)
        .collect();
    if active.len() > 1 {
        return Err(ToolError::InvalidInput(format!(
            "steps {} are all marked 'active'; exactly one step may be in progress. Mark the one \
             you are working on now as 'active' and leave the rest 'todo' or 'done'",
            active
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    // A finished step sitting after an unfinished one. The list is worked in
    // order — the model is told to work the step marked [>] and not to start
    // another until it is [x] — so this is not a plan that ran out of order. It
    // is a step whose state was never updated, and left alone it reads as
    // "queued" on the panel beside work that visibly happened, and comes back
    // in the prompt every iteration as work still to do.
    if let Some(open) = steps.iter().position(|s| s.state != StepState::Done) {
        if let Some(done) = steps.iter().rposition(|s| s.state == StepState::Done) {
            if done > open {
                return Err(ToolError::InvalidInput(format!(
                    "step {} is marked 'done' but step {} before it is still '{}'. Steps are \
                     worked in order, so either step {} is finished too and needs marking 'done', \
                     or the list is in the wrong order and the finished steps belong first",
                    done + 1,
                    open + 1,
                    word(steps[open].state),
                    open + 1,
                )));
            }
        }
    }

    // A step that was done and is not any more. Nearly always the model
    // re-typing the list and leaving the state off, which reads as 'todo' by
    // default — see `Step::from_json`, which is right to be forgiving about an
    // omitted state on a step it has never seen and cannot be, here.
    if let Some((n, step)) = steps.iter().enumerate().find(|(_, s)| {
        s.state != StepState::Done
            && previous
                .iter()
                .any(|p| p.state == StepState::Done && p.text.trim() == s.text.trim())
    }) {
        return Err(ToolError::InvalidInput(format!(
            "step {} '{}' is marked '{}', but you had already marked it 'done'. A finished step \
             does not reopen — send the whole list back with it still 'done'. If the request has \
             moved on, write the new plan with its own steps instead",
            n + 1,
            step.text.trim(),
            word(step.state),
        )));
    }

    Ok(())
}

/// A state in the word the model writes it with, for an error message that can
/// be acted on by copying it.
fn word(state: StepState) -> &'static str {
    match state {
        StepState::Todo => "todo",
        StepState::Active => "active",
        StepState::Done => "done",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::test_ctx;

    fn step(text: &str, state: StepState) -> Step {
        Step {
            text: text.into(),
            state,
            active_form: None,
        }
    }

    fn input(steps: Vec<Step>) -> serde_json::Value {
        serde_json::json!({ "steps": steps })
    }

    #[tokio::test]
    async fn a_plan_lands_on_the_board_the_prompt_reads_from() {
        // The whole mechanism in one test: what the tool takes is what the next
        // iteration's system prompt carries.
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());

        tool.execute(
            input(vec![
                step("Read the parser", StepState::Done),
                step("Add the token type", StepState::Active),
            ]),
            &ctx,
        )
        .await
        .expect("a valid plan");

        let reminder = board.reminder().expect("the board holds it");
        assert!(reminder.contains("[x] Read the parser"), "{reminder}");
        assert!(reminder.contains("[>] Add the token type"), "{reminder}");
    }

    #[tokio::test]
    async fn the_card_is_drawn_from_the_input_before_the_call_runs() {
        // Same identity every drawn tool keeps: the view is the input, so a
        // reopened conversation can redraw a plan nothing recomputed.
        let tool = UpdatePlan::new(PlanBoard::new());
        let view = tool
            .view("call-1", &input(vec![step("One", StepState::Active)]))
            .expect("a view");
        match view {
            TranscriptView::Plan { steps } => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].state, StepState::Active);
            }
            other => panic!("expected a plan, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_broken_plan_draws_no_card_at_all() {
        // The view is drawn first. Without the same check here the user would
        // watch an impossible plan appear and only then be told it failed.
        let tool = UpdatePlan::new(PlanBoard::new());
        let broken = input(vec![
            step("One", StepState::Active),
            step("Two", StepState::Active),
        ]);
        assert!(tool.view("call-1", &broken).is_none());
    }

    #[tokio::test]
    async fn two_steps_in_progress_is_refused_and_says_which() {
        // A model that lost track of its own list needs to be told where, not
        // that something was wrong.
        let (ctx, _dir) = test_ctx();
        let tool = UpdatePlan::new(PlanBoard::new());
        let error = tool
            .execute(
                input(vec![
                    step("One", StepState::Active),
                    step("Two", StepState::Done),
                    step("Three", StepState::Active),
                ]),
                &ctx,
            )
            .await
            .expect_err("two active steps");
        let message = error.to_string();
        assert!(message.contains("steps 1, 3"), "{message}");
    }

    #[tokio::test]
    async fn an_empty_plan_is_refused_with_the_thing_to_do_instead() {
        // A model finishing a task reaches for this. Telling it only "no" would
        // leave the last plan showing a step still in progress forever.
        let (ctx, _dir) = test_ctx();
        let tool = UpdatePlan::new(PlanBoard::new());
        let error = tool
            .execute(input(Vec::new()), &ctx)
            .await
            .expect_err("an empty plan");
        assert!(error.to_string().contains("marked 'done'"), "{error}");
    }

    #[tokio::test]
    async fn a_plan_longer_than_the_cap_is_refused() {
        let (ctx, _dir) = test_ctx();
        let tool = UpdatePlan::new(PlanBoard::new());
        let steps: Vec<Step> = (0..MAX_STEPS + 1)
            .map(|n| step(&format!("Step {n}"), StepState::Todo))
            .collect();
        let error = tool
            .execute(input(steps), &ctx)
            .await
            .expect_err("too many steps");
        assert!(
            error.to_string().contains("Group the small ones"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_step_that_is_a_paragraph_is_refused() {
        let (ctx, _dir) = test_ctx();
        let tool = UpdatePlan::new(PlanBoard::new());
        let error = tool
            .execute(
                input(vec![step(&"x".repeat(MAX_STEP_CHARS + 1), StepState::Todo)]),
                &ctx,
            )
            .await
            .expect_err("an over-long step");
        assert!(error.to_string().contains("not an explanation"), "{error}");
    }

    #[tokio::test]
    async fn an_active_form_that_grew_into_a_sentence_is_refused_too() {
        // It is shown in one line above the list, so it is capped like the
        // step it restates — otherwise the cap is trivially routed around.
        let (ctx, _dir) = test_ctx();
        let tool = UpdatePlan::new(PlanBoard::new());
        let error = tool
            .execute(
                serde_json::json!({ "steps": [{
                    "text": "Add the token type",
                    "active_form": "x".repeat(MAX_STEP_CHARS + 1),
                }] }),
                &ctx,
            )
            .await
            .expect_err("an over-long active form");
        assert!(error.to_string().contains("'active_form' is"), "{error}");
    }

    #[tokio::test]
    async fn a_step_defaults_to_todo_when_the_model_omits_the_state() {
        // Small models leave optional fields out. Failing on that would spend
        // an iteration on a plan that was perfectly clear.
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        tool.execute(
            serde_json::json!({ "steps": [{ "text": "Read the parser" }] }),
            &ctx,
        )
        .await
        .expect("a state-less step is fine");

        assert_eq!(board.steps()[0].state, StepState::Todo);
    }

    #[tokio::test]
    async fn a_step_finished_out_of_turn_is_refused_and_names_the_one_left_open() {
        // The reported failure: a two-step task where the second is marked done
        // and the first never was. Nothing else catches it — one step is active,
        // the texts are fine, and the list is the length it should be — so it
        // sits on the panel as "queued" beside work that visibly happened.
        let (ctx, _dir) = test_ctx();
        let tool = UpdatePlan::new(PlanBoard::new());
        let error = tool
            .execute(
                input(vec![
                    step("Change the greeting", StepState::Active),
                    step("Add the version field", StepState::Done),
                ]),
                &ctx,
            )
            .await
            .expect_err("a done step above an open one");
        let message = error.to_string();
        assert!(message.contains("step 2 is marked 'done'"), "{message}");
        assert!(message.contains("step 1 before it"), "{message}");
        assert!(message.contains("'active'"), "{message}");
    }

    #[tokio::test]
    async fn working_down_the_list_in_order_is_not_refused() {
        // The shape the rule above must not touch: done behind, active here,
        // todo ahead. Every plan that is going well looks like this.
        let (ctx, _dir) = test_ctx();
        let tool = UpdatePlan::new(PlanBoard::new());
        tool.execute(
            input(vec![
                step("One", StepState::Done),
                step("Two", StepState::Active),
                step("Three", StepState::Todo),
            ]),
            &ctx,
        )
        .await
        .expect("ordinary progress");
    }

    #[tokio::test]
    async fn a_step_that_un_finishes_itself_is_refused() {
        // A whole-list replacement carries no memory, and an omitted state
        // reads as 'todo' — so a model that re-types the list without the
        // states silently undoes every step it had finished. The board is the
        // only thing that can see it.
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        tool.execute(
            input(vec![
                step("Read the parser", StepState::Done),
                step("Add the token type", StepState::Active),
            ]),
            &ctx,
        )
        .await
        .expect("a valid plan");

        let error = tool
            .execute(
                serde_json::json!({ "steps": [
                    { "text": "Read the parser" },
                    { "text": "Add the token type", "state": "active" },
                ] }),
                &ctx,
            )
            .await
            .expect_err("step 1 went back to todo");
        let message = error.to_string();
        assert!(message.contains("'Read the parser'"), "{message}");
        assert!(message.contains("already marked it 'done'"), "{message}");

        // And the board still holds the plan it accepted.
        assert_eq!(board.steps()[0].state, StepState::Done);
    }

    #[tokio::test]
    async fn a_replaced_plan_may_use_states_freely_on_steps_it_never_had() {
        // The escape the error names. A follow-up that changes the subject gets
        // a new list, and nothing about the old one constrains it.
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        tool.execute(input(vec![step("Read the parser", StepState::Done)]), &ctx)
            .await
            .expect("a valid plan");

        tool.execute(
            input(vec![
                step("Draft the release notes", StepState::Active),
                step("Tag the release", StepState::Todo),
            ]),
            &ctx,
        )
        .await
        .expect("a different task is a different plan");
    }

    #[tokio::test]
    async fn a_rejected_plan_draws_no_card_over_the_one_it_failed_to_replace() {
        // `view` runs the same check against the same board as `execute`, so a
        // plan that will be rejected never becomes the card the pinned panel
        // reads. If the two disagreed, a refused call would blank the plan.
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        tool.execute(input(vec![step("One", StepState::Done)]), &ctx)
            .await
            .expect("a valid plan");

        let reopened = input(vec![
            step("One", StepState::Todo),
            step("Two", StepState::Active),
        ]);
        assert!(tool.view("call-2", &reopened).is_none());
        assert!(tool.execute(reopened, &ctx).await.is_err());
    }

    #[tokio::test]
    async fn nothing_about_a_plan_needs_permission() {
        // It writes no file and runs nothing. A prompt here would be a dialog
        // in front of the model saying what it intends to do.
        assert_eq!(UpdatePlan::new(PlanBoard::new()).effect(), Effect::Read);
    }
}
