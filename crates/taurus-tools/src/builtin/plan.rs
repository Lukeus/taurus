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

use crate::plan::{same_steps, ChangedBy, PlanBoard, PlanChange, Rules};
use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};
use crate::view::{Step, StepState, TranscriptView};

/// Steps one plan may hold.
///
/// Not a context limit — the reminder is small at any plausible size. It is a
/// judgement about granularity: past a dozen steps the model is writing out the
/// task instead of planning it, and a checklist that long stops being something
/// either party can hold in view. The failure it produces is a model that
/// spends its iterations maintaining the list.
pub(crate) const MAX_STEPS: usize = 12;

/// Longest a single step may be.
///
/// A step is a thing to do, not a paragraph explaining it. The cap is what
/// stops a plan turning into the model's notes, which is where the reminder's
/// cost stops being negligible.
pub(crate) const MAX_STEP_CHARS: usize = 120;

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
         the one running. When a step can be proved by running something, give it a 'check' too: \
         the command that shows it worked, like 'cargo test -p parser'. Run it before you mark \
         the step done."
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
        let rules = self.board.rules();
        let (steps, _) = honor_edits(input.steps, &rules);
        check(&steps, &rules).ok()?;
        Some(TranscriptView::Plan { steps })
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: UpdatePlanInput = parse_input(input)?;
        let rules = self.board.rules();
        let (steps, kept) = honor_edits(input.steps, &rules);
        // A list that only differs from the board by the person's edit comes
        // out of the repair as the board itself, and is refused as a list
        // sent twice. Without the note, that refusal never says why.
        if let Err(error) = check(&steps, &rules) {
            return Err(match (error, kept) {
                (ToolError::InvalidInput(m), Some(k)) => {
                    ToolError::InvalidInput(format!("{m}. {k}"))
                }
                (ToolError::Rejected(m), Some(k)) => ToolError::Rejected(format!("{m}. {k}")),
                (error, _) => error,
            });
        }
        let mut result = self.board.set(steps.clone());
        if let Some(change) = PlanChange::between(
            &rules.previous,
            &steps,
            ChangedBy::Model,
            self.board.revision(),
        ) {
            change.announce(
                ctx.hooks.as_ref(),
                &ctx.workspace,
                ctx.session_id.as_deref(),
            );
        }
        if let Some(kept) = kept {
            result.push(' ');
            result.push_str(&kept);
        }
        Ok(result.into())
    }
}

/// The model's list with the person's edits put back: their steps kept, the
/// ones they took off left off. Says what it changed, for the tool result.
///
/// Repaired where everything in [`check`] is refused, because the model can't
/// be relied on to comply. Its history holds the list it wrote before the
/// edit, and a small model sends that list again however plainly the reminder
/// says otherwise: refused, it sends it again, until the turn gives up. The
/// person's edit is a fact the harness holds, not a mistake the model made,
/// so the harness applies it and says so, and the list the model reads back
/// next is the one it's told it set.
///
/// A step put back keeps its place after the step it followed, and its state,
/// moved only as far as the order rule needs.
fn honor_edits(mut steps: Vec<Step>, rules: &Rules) -> (Vec<Step>, Option<String>) {
    let previous = &rules.previous[..];
    // A carried plan replaced by one with nothing in common is the request
    // having moved on, and the person's steps belonged to the plan it left.
    if rules.carried && !previous.iter().any(|old| has(&steps, &old.text)) {
        return (steps, None);
    }

    let mut left_off = Vec::new();
    steps.retain(|s| {
        let removed = rules.removed.iter().any(|t| t == s.text.trim());
        if removed {
            left_off.push(s.text.trim().to_string());
        }
        !removed
    });

    let mut kept = Vec::new();
    for (index, old) in previous.iter().enumerate() {
        let text = old.text.trim();
        if !rules.yours.iter().any(|t| t == text) || has(&steps, text) {
            continue;
        }
        let mut at = previous[..index]
            .iter()
            .filter_map(|before| {
                steps
                    .iter()
                    .position(|s| s.text.trim() == before.text.trim())
            })
            .max()
            .map_or(0, |n| n + 1);
        let mut step = old.clone();
        let first_open = steps
            .iter()
            .position(|s| s.state != StepState::Done)
            .unwrap_or(steps.len());
        if step.state == StepState::Done {
            at = at.min(first_open);
        } else {
            at = at.max(first_open);
            if step.state == StepState::Active && steps.iter().any(|s| s.state == StepState::Active)
            {
                step.state = StepState::Todo;
            }
        }
        steps.insert(at, step);
        kept.push(text.to_string());
    }

    let quoted = |texts: &[String]| {
        texts
            .iter()
            .map(|t| format!("'{t}'"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut said = Vec::new();
    if !kept.is_empty() {
        said.push(format!(
            "Kept {} on the plan: the user added it, so it stays, worded as it is.",
            quoted(&kept)
        ));
    }
    if !left_off.is_empty() {
        said.push(format!(
            "Left {} off: the user took it off the plan, so don't do it.",
            quoted(&left_off)
        ));
    }
    let note = (!said.is_empty()).then(|| {
        let list: Vec<String> = steps
            .iter()
            .enumerate()
            .map(|(n, s)| format!("{}. {} {}", n + 1, s.state.marker(), s.text))
            .collect();
        said.push(format!(
            "The plan is now this, and it's the list to send next time:\n{}",
            list.join("\n")
        ));
        said.join(" ")
    });
    (steps, note)
}

/// Everything about a plan that has to hold before it is worth keeping.
///
/// Refused rather than repaired. Every one of these is the model having lost
/// track of its own list, and a plan quietly corrected into something it did
/// not write is worse than a rejected call: the checklist it then reads back
/// would not be the one it believes it set.
///
/// `rules.previous` is the plan on the board, which the later rules are about.
/// A whole-list replacement carries no memory, so the only place a step that
/// silently un-finished itself can be noticed is here, against what it replaced.
fn check(steps: &[Step], rules: &Rules) -> Result<(), ToolError> {
    let previous = &rules.previous[..];
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

    /*
     * The plan that is already on the board, sent again.
     *
     * This is the loop that prompted the rule, and it is worth describing
     * exactly because it looks like nothing going wrong. A model calls
     * update_plan with a list it has already set; the call is valid, so it
     * succeeds; the result says "Plan updated: 0 of 5 steps done. Now working
     * on: X"; the next iteration's system prompt carries the same reminder, in
     * front of the same history, and asks the same question. Nothing in the
     * loop differs, so the model does the same thing again. Observed eight
     * times in a row on one turn, each one a full request against the whole
     * prompt, until a context trim happened to perturb it enough to break the
     * tie.
     *
     * Nothing above catches it: every rule up to here is about a list that is
     * malformed, and this list is perfectly formed. It is the *call* that is
     * empty, not the plan.
     *
     * So it is refused, and the error names the two things that are actually
     * next — which is the whole point of failing rather than quietly
     * succeeding. It also has a second effect worth knowing about: a refused
     * call is a failed round, and the agent loop's stall detector only counts
     * rounds where everything failed. A no-op that reports success is
     * invisible to it; a no-op that reports failure is caught after
     * `stall_limit` of them and the turn is stopped with a reason.
     *
     * Compared on text and state alone. `active_form` is left out deliberately:
     * it is the same step said differently, so a list that differs only there
     * has not moved either — and if it counted, a model could stay in the loop
     * by rewording the participle every round.
     *
     * A *carried* plan is exempt. Re-sending one unchanged is how a model
     * takes ownership of a plan an earlier turn left behind, and that changes
     * something real even though no step moved: `PlanBoard::set` clears the
     * carried flag, and the reminder stops telling the model this was written
     * before the message it is answering.
     *
     * So is a plan the person has edited since the model last wrote it.
     * Sending their list back unchanged is the model saying it has read the
     * edit, and that clears the note the reminder carries about it.
     */
    if !rules.carried && !rules.edited && same_steps(steps, previous) {
        let next = steps
            .iter()
            .position(|s| s.state != StepState::Done)
            .map(|n| n + 1);
        // `Rejected`, not `InvalidInput`: the list is well formed and the call
        // is refused on its merits, so the advice that follows must not be
        // "check the schema and retry". It was, in the first version of this,
        // and models read it exactly as written — the silent success loop
        // became a loud retry loop of the same length.
        return Err(ToolError::Rejected(match next {
            Some(n) => format!(
                "this is the plan already on the board — nothing in it changed, so there was \
                 nothing to update. Call update_plan when a step's state changes, not to restate \
                 it. Right now the thing to do is step {n}, '{}': go and do it, and call \
                 update_plan again once it is finished",
                steps[n - 1].text.trim(),
            ),
            None => "this is the plan already on the board, and every step on it is already \
                     'done'. There is nothing left to update: say what you did and stop"
                .into(),
        }));
    }

    Ok(())
}

/// Whether `steps` has one whose text is `text`, whitespace at the ends aside.
fn has(steps: &[Step], text: &str) -> bool {
    steps.iter().any(|s| s.text.trim() == text.trim())
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
            check: None,
        }
    }

    fn input(steps: Vec<Step>) -> serde_json::Value {
        serde_json::json!({ "steps": steps })
    }

    fn text(output: taurus_provider::ToolOutput) -> String {
        output.as_text().unwrap_or_default().to_string()
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
    async fn the_same_plan_sent_twice_is_refused_and_names_the_step_to_go_and_do() {
        /*
         * The loop this rule exists for. Every call is valid, so nothing above
         * catches it, and a call that succeeds while changing nothing leaves
         * the next iteration reading exactly what the last one did — which is
         * a model that does the same thing again. Observed eight times in a
         * row on one turn.
         *
         * The error has to name the step, because "nothing changed" alone
         * still leaves the model with no idea what to do instead.
         */
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        let plan = || {
            input(vec![
                step("Verify the project setup", StepState::Active),
                step("Add the token type", StepState::Todo),
            ])
        };

        tool.execute(plan(), &ctx)
            .await
            .expect("the first one sets it");

        let error = tool
            .execute(plan(), &ctx)
            .await
            .expect_err("the second one changes nothing");
        let message = error.to_model_message();
        assert!(message.contains("already on the board"), "{message}");
        assert!(message.contains("step 1"), "{message}");
        // The whole reason this is `Rejected`: what the model is told next must
        // not be "retry", because retrying is the loop.
        assert!(!message.contains("retry"), "{message}");
        assert!(message.contains("will fail the same way"), "{message}");
        assert!(message.contains("Verify the project setup"), "{message}");

        // And the board still holds it. A refused no-op must not blank a plan
        // that was perfectly good.
        assert_eq!(board.steps().len(), 2);
        assert_eq!(board.steps()[0].state, StepState::Active);
    }

    #[tokio::test]
    async fn restating_a_finished_plan_is_told_to_stop_rather_than_to_do_a_step() {
        // The same rule at the other end of the list, where there is no step
        // to name. Telling a model to "go and do step None" would be worse
        // than saying nothing.
        let (ctx, _dir) = test_ctx();
        let tool = UpdatePlan::new(PlanBoard::new());
        let plan = || input(vec![step("Add the token type", StepState::Done)]);

        tool.execute(plan(), &ctx).await.expect("a finished plan");
        let error = tool
            .execute(plan(), &ctx)
            .await
            .expect_err("nothing changed");
        assert!(
            error.to_string().contains("say what you did and stop"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_plan_that_actually_moves_is_not_touched_by_the_rule() {
        // The shape the rule must never catch: the same steps, one of them
        // further along. This is what every working turn does.
        let (ctx, _dir) = test_ctx();
        let tool = UpdatePlan::new(PlanBoard::new());
        tool.execute(
            input(vec![
                step("One", StepState::Active),
                step("Two", StepState::Todo),
            ]),
            &ctx,
        )
        .await
        .expect("a valid plan");

        tool.execute(
            input(vec![
                step("One", StepState::Done),
                step("Two", StepState::Active),
            ]),
            &ctx,
        )
        .await
        .expect("a step finished, which is exactly what to call this for");
    }

    #[tokio::test]
    async fn a_carried_plan_may_be_adopted_unchanged() {
        /*
         * The exemption. A plan left unfinished by an earlier turn is carried
         * into this one, and re-sending it as-is is how a model says it is
         * continuing that work rather than starting something else. Nothing
         * moves, but the flag does — and with it the sentence in the reminder
         * telling the model this was written before the message it is now
         * answering.
         */
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        let plan = || {
            input(vec![
                step("Verify the project setup", StepState::Active),
                step("Add the token type", StepState::Todo),
            ])
        };
        tool.execute(plan(), &ctx).await.expect("a valid plan");

        assert!(board.start_turn(), "an unfinished plan is carried");
        assert!(board.carried());
        tool.execute(plan(), &ctx)
            .await
            .expect("adopting a carried plan is a change, not a no-op");
        assert!(!board.carried(), "and it is this turn's plan now");

        // Which is also what closes the exemption: a second identical call in
        // the same turn is the loop again, and is refused.
        assert!(tool.execute(plan(), &ctx).await.is_err());
    }

    #[tokio::test]
    async fn rewording_only_the_active_form_does_not_count_as_progress() {
        // Left out of the comparison on purpose: it is the same step said
        // differently, so a list differing only there has not moved — and if
        // it counted, the loop could be sustained by rewording a participle.
        let (ctx, _dir) = test_ctx();
        let tool = UpdatePlan::new(PlanBoard::new());
        tool.execute(
            serde_json::json!({ "steps": [
                { "text": "Verify the setup", "state": "active", "active_form": "Verifying the setup" },
            ] }),
            &ctx,
        )
        .await
        .expect("a valid plan");

        let error = tool
            .execute(
                serde_json::json!({ "steps": [
                    { "text": "Verify the setup", "state": "active", "active_form": "Checking the setup" },
                ] }),
                &ctx,
            )
            .await
            .expect_err("the plan itself did not move");
        assert!(
            error.to_string().contains("already on the board"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_repeated_plan_draws_no_card_either() {
        // `view` runs the same check, so the loop does not leave a row of
        // identical plan cards down the transcript on the way to being
        // refused. The screenshot that started this had eight of them.
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        let plan = input(vec![step("One", StepState::Active)]);
        tool.execute(plan.clone(), &ctx)
            .await
            .expect("a valid plan");
        assert!(tool.view("call-2", &plan).is_none());
    }

    #[tokio::test]
    async fn nothing_about_a_plan_needs_permission() {
        // It writes no file and runs nothing. A prompt here would be a dialog
        // in front of the model saying what it intends to do.
        assert_eq!(UpdatePlan::new(PlanBoard::new()).effect(), Effect::Read);
    }

    fn texts(board: &PlanBoard) -> Vec<(String, StepState)> {
        board
            .steps()
            .into_iter()
            .map(|s| (s.text, s.state))
            .collect()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_plan_change_hook_hears_each_change_and_not_a_repeat() {
        use std::os::unix::fs::PermissionsExt;

        let (ctx, _dir) = test_ctx();
        let scripts = tempfile::TempDir::new().unwrap();
        // A file per run: they run beside the turn, so two can overlap, and
        // appending to one file would interleave them.
        let log = scripts.path().join("log");
        std::fs::create_dir(&log).unwrap();
        let script = scripts.path().join("hook");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ncat > '{}'/$$.json\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ctx = ctx.with_hooks(std::sync::Arc::new(taurus_hooks::HookRunner::new(vec![(
            "mirror".into(),
            taurus_hooks::Hook {
                on: taurus_hooks::HookEvent::PlanChange,
                command: script.display().to_string(),
                args: vec![],
                matches: None,
                timeout_seconds: 5,
                foreign: None,
                disabled: false,
            },
        )])));
        let tool = UpdatePlan::new(PlanBoard::new());

        let first = vec![
            step("Read the parser", StepState::Active),
            step("Add the token type", StepState::Todo),
        ];
        tool.execute(input(first.clone()), &ctx).await.unwrap();
        // Refused as a repeat, so nothing changed and nothing is said.
        tool.execute(input(first), &ctx).await.unwrap_err();
        tool.execute(
            input(vec![
                step("Read the parser", StepState::Done),
                step("Add the token type", StepState::Active),
            ]),
            &ctx,
        )
        .await
        .unwrap();

        // The hooks run beside the call, so wait for both to land.
        let mut lines = Vec::new();
        for _ in 0..100 {
            lines = std::fs::read_dir(&log)
                .unwrap()
                .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path()).ok())
                .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .collect();
            if lines.len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        lines.sort_by_key(|l| l["plan"]["revision"].as_u64());
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0]["event"], "plan_change");
        assert_eq!(lines[0]["plan"]["by"], "model");
        assert_eq!(
            lines[0]["plan"]["changes"][0],
            serde_json::json!({"text": "Read the parser", "from": null, "to": "active"})
        );
        assert_eq!(
            lines[1]["plan"]["changes"],
            serde_json::json!([
                {"text": "Read the parser", "from": "active", "to": "done"},
                {"text": "Add the token type", "from": "todo", "to": "active"},
            ])
        );
        assert!(lines[1]["plan"]["revision"].as_u64() > lines[0]["plan"]["revision"].as_u64());
    }

    #[tokio::test]
    async fn a_list_that_drops_a_step_the_user_added_gets_it_back() {
        // The rule the edit panel depends on. The model sends the list it
        // remembers, and a step it didn't write is exactly what it forgets.
        // Refused, a small model sends the same list again until the turn
        // gives up, so the step is put back and the model is told.
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        let mine = vec![
            step("Read the parser", StepState::Active),
            step("Add the token type", StepState::Todo),
        ];
        tool.execute(input(mine.clone()), &ctx)
            .await
            .expect("a valid plan");
        let mut edited = mine.clone();
        edited.insert(1, step("Update the changelog", StepState::Todo));
        board.edit(&mine, edited).expect("a valid edit");

        let said = text(
            tool.execute(
                input(vec![
                    step("Read the parser", StepState::Done),
                    step("Add the token type", StepState::Active),
                ]),
                &ctx,
            )
            .await
            .expect("repaired, not refused"),
        );
        assert!(
            said.contains("Kept 'Update the changelog' on the plan"),
            "{said}"
        );
        assert!(
            said.contains(
                "1. [x] Read the parser\n2. [ ] Update the changelog\n3. [>] Add the token type"
            ),
            "the list to send next time, spelled out: {said}"
        );
        // Back where the person put it: after the step it followed, and
        // still ahead of the step the model has moved on to.
        assert_eq!(
            texts(&board),
            [
                ("Read the parser".to_string(), StepState::Done),
                ("Update the changelog".to_string(), StepState::Todo),
                ("Add the token type".to_string(), StepState::Active),
            ]
        );
    }

    #[tokio::test]
    async fn a_list_that_brings_back_a_step_the_user_removed_leaves_it_off() {
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        let plan = vec![
            step("Read the parser", StepState::Active),
            step("Rewrite the lexer", StepState::Todo),
        ];
        tool.execute(input(plan.clone()), &ctx)
            .await
            .expect("a valid plan");
        board
            .edit(&plan, vec![step("Read the parser", StepState::Active)])
            .expect("a valid edit");

        let said = text(
            tool.execute(input(plan), &ctx)
                .await
                .expect("repaired, not refused"),
        );
        assert!(said.contains("Left 'Rewrite the lexer' off"), "{said}");
        assert_eq!(
            texts(&board),
            [("Read the parser".to_string(), StepState::Active)]
        );
    }

    #[tokio::test]
    async fn sending_back_the_edited_plan_unchanged_is_how_the_model_reads_the_edit() {
        // Otherwise the identical-plan rule would refuse the one call that
        // says the model has seen what the user did.
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        let plan = vec![step("Read the parser", StepState::Active)];
        tool.execute(input(plan.clone()), &ctx)
            .await
            .expect("a valid plan");
        let edited = vec![
            step("Read the parser", StepState::Active),
            step("Add a test", StepState::Todo),
        ];
        board.edit(&plan, edited.clone()).expect("a valid edit");
        assert!(board
            .reminder()
            .unwrap()
            .contains("The user edited this plan"));

        tool.execute(input(edited.clone()), &ctx)
            .await
            .expect("adopting the edit is a change");
        let reminder = board.reminder().unwrap();
        assert!(
            !reminder.contains("The user edited this plan"),
            "{reminder}"
        );
        // The step is still the user's, for as long as the plan lives.
        assert!(
            reminder.contains("Add a test (added by the user)"),
            "{reminder}"
        );
        assert!(tool.execute(input(edited), &ctx).await.is_err());
    }

    #[tokio::test]
    async fn a_new_plan_over_a_carried_one_may_leave_the_users_steps_behind() {
        // The request moved on. Holding the model to a step the user added to
        // a task they've stopped asking about would chain every later plan to
        // the first one.
        let (ctx, _dir) = test_ctx();
        let board = PlanBoard::new();
        let tool = UpdatePlan::new(board.clone());
        let plan = vec![step("Read the parser", StepState::Active)];
        tool.execute(input(plan.clone()), &ctx)
            .await
            .expect("a valid plan");
        board
            .edit(
                &plan,
                vec![
                    step("Read the parser", StepState::Active),
                    step("Add a test", StepState::Todo),
                ],
            )
            .expect("a valid edit");

        board.start_turn();
        tool.execute(
            input(vec![step("Write the release notes", StepState::Active)]),
            &ctx,
        )
        .await
        .expect("a different task");
        assert!(!board.reminder().unwrap().contains("added by the user"));
    }
}
