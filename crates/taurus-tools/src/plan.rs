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
//!
//! # When the person edits it
//!
//! The checklist has two authors. The person watching can add a step, reword
//! one, take one off or move one, from the panel, while a turn runs or between
//! turns — [`PlanBoard::edit`]. Nothing has to interrupt the model for that to
//! reach it: the reminder is rebuilt from the board every iteration, so an edit
//! lands at the next round boundary on its own.
//!
//! What does need saying is that whole-list replacement would undo it. The
//! model sends back the list it remembers, and an edit it didn't make is
//! exactly what it doesn't remember. So the board keeps, by step text, which
//! steps the person wrote and which they took off, and `update_plan` refuses a
//! list that drops one or brings one back. The model is told once, in the
//! reminder, what changed, and every step the person wrote stays marked as
//! theirs for as long as the plan lives.
//!
//! Finished steps are history and the person can't edit them; that is also
//! what keeps an edit inside the ordering rule `update_plan` holds the model
//! to, since everything done stays at the front.

use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::builtin::plan::{MAX_STEPS, MAX_STEP_CHARS};
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
    /// The steps the person wrote, by trimmed text. `update_plan` may change
    /// their state and nothing else.
    yours: Vec<String>,
    /// The steps the person took off, by trimmed text. `update_plan` may not
    /// put them back.
    removed: Vec<String>,
    /// What the person changed since the model last wrote the plan, said once
    /// in the reminder and cleared by the model's next [`PlanBoard::set`].
    edited: Option<PlanEdit>,
}

/// What a person changed about the plan, by step text.
///
/// By text because steps have no ids, which is deliberate — see the module
/// doc. Rewording a step is therefore one removed and one added, and that is
/// what the model is told, which is also the truth about what it has to do
/// differently.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanEdit {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// The steps that were already there are in a new order.
    pub reordered: bool,
}

impl PlanEdit {
    /// Folds a later edit into this one, for two edits the model hasn't read
    /// yet. A step added and then removed again was never news.
    pub fn then(mut self, later: PlanEdit) -> PlanEdit {
        for text in later.added {
            if let Some(at) = self.removed.iter().position(|t| *t == text) {
                self.removed.remove(at);
            } else if !self.added.contains(&text) {
                self.added.push(text);
            }
        }
        for text in later.removed {
            if let Some(at) = self.added.iter().position(|t| *t == text) {
                self.added.remove(at);
            } else if !self.removed.contains(&text) {
                self.removed.push(text);
            }
        }
        self.reordered |= later.reordered;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && !self.reordered
    }
}

/// A plan as a transcript left it, for [`PlanBoard::restore`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoredPlan {
    pub steps: Vec<Step>,
    /// Steps the person wrote.
    pub yours: Vec<String>,
    /// Steps the person took off.
    pub removed: Vec<String>,
    /// The person's edits since the model's last accepted list, which it
    /// hasn't seen yet.
    pub pending: Option<PlanEdit>,
}

/// Why an edit was not applied.
#[derive(Debug, PartialEq, Eq)]
pub enum EditRefused {
    /// The plan moved while the person was editing it, so their list was
    /// drawn from one that is no longer there.
    Stale,
    /// The list breaks a rule the plan holds. Says which, to the person.
    Invalid(String),
}

impl std::fmt::Display for EditRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditRefused::Stale => f.write_str(
                "Taurus updated the plan while you were editing it, so your changes weren't \
                 saved. The plan as it stands is back on screen.",
            ),
            EditRefused::Invalid(reason) => f.write_str(reason),
        }
    }
}

/// What `update_plan` checks a list against: the board as it stands.
pub(crate) struct Rules {
    pub previous: Vec<Step>,
    pub carried: bool,
    pub yours: Vec<String>,
    pub removed: Vec<String>,
    /// The person has changed the plan since the model last wrote it.
    pub edited: bool,
}

impl PlanBoard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the plan. Returns what the tool should tell the model it did.
    pub fn set(&self, steps: Vec<Step>) -> String {
        let summary = summarize(&steps);
        let mut board = self.board();
        // A list that shares no step with the one it replaces is a new plan,
        // which `update_plan` only allows over a carried one — the request has
        // moved on, and the person's steps belonged to the old one.
        if !board.steps.iter().any(|old| has(&steps, &old.text)) {
            board.yours.clear();
            board.removed.clear();
        }
        board.steps = steps;
        // Written in this turn, so it is no longer somebody else's checklist.
        board.carried = false;
        // Whatever the person changed, this list was written after reading it.
        board.edited = None;
        summary
    }

    /// Replaces the plan with one the person edited.
    ///
    /// `seen` is the plan they were looking at when they started. If the board
    /// has moved on since, their list was drawn from a plan that is no longer
    /// there, and applying it would silently undo whatever the model just
    /// did — so it is refused and they edit the current one instead. An empty
    /// board takes `seen` as what was there, which is the case of a process
    /// that restarted since the plan was drawn.
    ///
    /// Answers with the list as stored, which is the person's tidied of
    /// stray whitespace, and what changed. `Ok(None)` is an edit that changed
    /// nothing, which is not worth telling the model about or writing down.
    pub fn edit(
        &self,
        seen: &[Step],
        steps: Vec<Step>,
    ) -> Result<Option<(Vec<Step>, PlanEdit)>, EditRefused> {
        let mut board = self.board();
        if !board.steps.is_empty() && !same_steps(&board.steps, seen) {
            return Err(EditRefused::Stale);
        }
        let previous = if board.steps.is_empty() {
            seen.to_vec()
        } else {
            board.steps.clone()
        };
        let steps = valid_edit(&previous, steps).map_err(EditRefused::Invalid)?;

        let edit = PlanEdit {
            added: steps
                .iter()
                .filter(|s| !has(&previous, &s.text))
                .map(|s| s.text.trim().to_string())
                .collect(),
            removed: previous
                .iter()
                .filter(|s| !has(&steps, &s.text))
                .map(|s| s.text.trim().to_string())
                .collect(),
            reordered: {
                let kept: Vec<&str> = steps
                    .iter()
                    .map(|s| s.text.trim())
                    .filter(|t| has(&previous, t))
                    .collect();
                let was: Vec<&str> = previous
                    .iter()
                    .map(|s| s.text.trim())
                    .filter(|t| has(&steps, t))
                    .collect();
                kept != was
            },
        };
        if edit.is_empty() {
            return Ok(None);
        }

        for text in &edit.added {
            board.removed.retain(|t| t != text);
            if !board.yours.contains(text) {
                board.yours.push(text.clone());
            }
        }
        for text in &edit.removed {
            board.yours.retain(|t| t != text);
            if !board.removed.contains(text) {
                board.removed.push(text.clone());
            }
        }
        board.edited = Some(match board.edited.take() {
            Some(earlier) => earlier.then(edit.clone()),
            None => edit.clone(),
        });
        board.steps = steps.clone();
        Ok(Some((steps, edit)))
    }

    /// Puts back a plan read from a transcript, into a board this process
    /// hasn't used yet.
    ///
    /// The board isn't saved, so without this a restart forgets it: the panel
    /// redraws the plan from the transcript while the model's reminder is
    /// empty, and the steps the person wrote lose their protection. Ignored if
    /// the board already holds something, which is newer than any transcript.
    ///
    /// An edit the model hadn't answered before the restart is still news to
    /// it, so it comes back as pending and the reminder says what changed. A
    /// plan that only lists the person's steps would read, to a model whose
    /// history holds its own older list, like a list to argue with.
    pub fn restore(&self, plan: RestoredPlan) {
        let mut board = self.board();
        if !board.steps.is_empty() || plan.steps.is_empty() {
            return;
        }
        let RestoredPlan {
            steps,
            yours,
            removed,
            pending,
        } = plan;
        board.yours = yours.into_iter().filter(|t| has(&steps, t)).collect();
        board.removed = removed.into_iter().filter(|t| !has(&steps, t)).collect();
        board.edited = pending.filter(|edit| !edit.is_empty());
        board.steps = steps;
        board.carried = true;
    }

    /// The board, as `update_plan` checks a list against it.
    pub(crate) fn rules(&self) -> Rules {
        let board = self.board();
        Rules {
            previous: board.steps.clone(),
            carried: board.carried,
            yours: board.yours.clone(),
            removed: board.removed.clone(),
            edited: board.edited.is_some(),
        }
    }

    pub fn steps(&self) -> Vec<Step> {
        self.board().steps.clone()
    }

    /// Whether the plan on the board was written before this turn began.
    ///
    /// Read by the identical-plan rule in [`crate::builtin::plan`], which is
    /// the one place the difference matters: re-sending a carried plan
    /// unchanged is how a model adopts it, and that *is* a change even though
    /// none of the steps moved — see [`PlanBoard::set`], which clears the flag.
    pub fn carried(&self) -> bool {
        self.board().carried
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

    /// The checks on steps marked done or in progress that none of `ran`
    /// covers, in plan order.
    ///
    /// A check is covered when every part of it — split on `&&`, `;` and new
    /// lines — appears in some command that ran, whitespace aside. Contained
    /// rather than equal, because `cargo test -p parser -- --nocapture` is
    /// that check run, and two parts may be two commands because a model runs
    /// `cargo build` and then `grep` rather than chaining them. A check that
    /// is prose rather than a command is never covered; the nudge that names
    /// it says how to answer that in one line.
    pub fn unrun_checks(&self, ran: &[String]) -> Vec<String> {
        let ran: Vec<String> = ran.iter().map(|command| squash(command)).collect();
        self.board()
            .steps
            .iter()
            .filter(|step| step.state != StepState::Todo)
            .filter_map(|step| step.check.clone())
            .filter(|check| {
                !check
                    .split(['\n', ';'])
                    .flat_map(|part| part.split("&&"))
                    .map(squash)
                    .filter(|part| !part.is_empty())
                    .all(|part| ran.iter().any(|command| command.contains(&part)))
            })
            .collect()
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
            *board = Board::default();
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
        // An edit leads, ahead of the list. A model's history holds its own
        // older list, and a note after the steps was read, by the small
        // models that most needed it, only once they had worked from that
        // older list instead.
        if let Some(edit) = &board.edited {
            out.push_str(&edit_note(edit));
        } else {
            out.push_str(if board.carried {
                "You wrote this with update_plan earlier in this conversation, before the message \
                 you are answering now. It is restated here every time because it is the record \
                 of where you are:\n\n"
            } else {
                "You wrote this with update_plan. It is restated here every time because it is \
                 the record of where you are:\n\n"
            });
        }
        for (index, step) in board.steps.iter().enumerate() {
            out.push_str(&format!(
                "{}. {} {}",
                index + 1,
                step.state.marker(),
                step.text
            ));
            if let Some(check) = &step.check {
                out.push_str(&format!(" (check: {check})"));
            }
            if board.yours.iter().any(|t| t == step.text.trim()) {
                out.push_str(" (added by the user)");
            }
            out.push('\n');
        }
        // Named on every request, not once: the older list naming them stays
        // in the history for as long as the conversation does.
        if !board.removed.is_empty() {
            out.push_str(&format!(
                "\nThe user took these off the plan. Don't do them, even where an earlier message \
                 lists them: {}.\n",
                quoted(&board.removed)
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
/// read the whole list at the end of the request anyway, and sending it twice
/// per update buys nothing but tokens.
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

/// What the model is told about an edit it hasn't written a list since.
///
/// Said as what to do, not only what happened: the edit is already on the list
/// above it, so the news is the two rules that now apply to the next list it
/// sends.
fn edit_note(edit: &PlanEdit) -> String {
    let mut changes = Vec::new();
    if !edit.added.is_empty() {
        changes.push(format!("added {}", quoted(&edit.added)));
    }
    if !edit.removed.is_empty() {
        changes.push(format!("removed {}", quoted(&edit.removed)));
    }
    if edit.reordered {
        changes.push("changed the order".to_string());
    }
    format!(
        "The user edited this plan since you last sent it: {}. This list replaces the one in \
         your earlier update_plan calls, so work from it, not from those. Steps marked (added by \
         the user) stay on it, worded as they are: send each one back in every list, and mark it \
         'done' once you have done it. Your next update_plan call sends this list back, with the \
         states as they stand.\n\n",
        changes.join("; ")
    )
}

fn quoted(texts: &[String]) -> String {
    texts
        .iter()
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `steps` has one whose text is `text`, whitespace at the ends aside.
fn has(steps: &[Step], text: &str) -> bool {
    steps.iter().any(|s| s.text.trim() == text.trim())
}

/// Whether two lists say the same thing about the same steps.
///
/// Trimmed, because the difference between "Add the token type" and the same
/// with a trailing space is not a plan that moved. Compared on text and state
/// alone: `active_form` is the same step said differently.
pub(crate) fn same_steps(steps: &[Step], previous: &[Step]) -> bool {
    steps.len() == previous.len()
        && steps
            .iter()
            .zip(previous)
            .all(|(a, b)| a.state == b.state && a.text.trim() == b.text.trim())
}

/// The person's list, if it is one the plan can hold.
///
/// Its own rules rather than `update_plan`'s, in the person's words rather
/// than the model's, and narrower: the panel lets you write steps and move
/// them, not finish them. So every finished step stays exactly where it was,
/// nothing new arrives finished, and the one step in progress is the one the
/// model said it was on, or its rewording.
fn valid_edit(previous: &[Step], steps: Vec<Step>) -> Result<Vec<Step>, String> {
    if steps.is_empty() {
        return Err("A plan needs at least one step.".into());
    }
    if steps.len() > MAX_STEPS {
        return Err(format!("A plan holds at most {MAX_STEPS} steps."));
    }
    let mut steps = steps;
    for step in &mut steps {
        let text = step.text.trim();
        if text.is_empty() {
            return Err("Every step needs some text.".into());
        }
        if text.chars().count() > MAX_STEP_CHARS {
            return Err(format!(
                "Keep each step under {MAX_STEP_CHARS} characters: '{}…' is longer.",
                text.chars().take(40).collect::<String>()
            ));
        }
        step.text = text.to_string();
    }
    for (n, step) in steps.iter().enumerate() {
        if steps[..n].iter().any(|s| s.text == step.text) {
            return Err(format!("'{}' is on the plan twice.", step.text));
        }
    }

    let done = |list: &[Step]| -> Vec<String> {
        list.iter()
            .filter(|s| s.state == StepState::Done)
            .map(|s| s.text.trim().to_string())
            .collect()
    };
    let finished = done(previous);
    if done(&steps) != finished
        || steps[..finished.len()]
            .iter()
            .any(|s| s.state != StepState::Done)
    {
        return Err("Finished steps can't be changed, removed or moved.".into());
    }
    if steps
        .iter()
        .filter(|s| s.state == StepState::Active)
        .count()
        > 1
    {
        return Err("Only one step can be in progress.".into());
    }
    Ok(steps)
}

/// Runs of whitespace as one space, ends trimmed.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(text: &str, state: StepState) -> Step {
        Step {
            text: text.into(),
            state,
            active_form: None,
            check: None,
        }
    }

    fn checked(text: &str, state: StepState, check: &str) -> Step {
        Step {
            check: Some(check.into()),
            ..step(text, state)
        }
    }

    #[test]
    fn a_step_names_its_check_in_the_reminder() {
        let board = PlanBoard::new();
        board.set(vec![checked(
            "Bump the version",
            StepState::Active,
            "cargo build",
        )]);
        assert!(board
            .reminder()
            .unwrap()
            .contains("1. [>] Bump the version (check: cargo build)"));
    }

    #[test]
    fn a_check_is_owed_until_every_part_of_it_has_run() {
        let board = PlanBoard::new();
        board.set(vec![
            checked("Parse it", StepState::Done, "cargo test -p parser"),
            checked(
                "Bump it",
                StepState::Active,
                "cargo build && grep version config.toml",
            ),
            // Not started, so nothing is owed for it yet.
            checked("Ship it", StepState::Todo, "cargo publish"),
        ]);
        let ran = |commands: &[&str]| {
            board.unrun_checks(&commands.iter().map(|c| c.to_string()).collect::<Vec<_>>())
        };

        assert_eq!(
            ran(&[]),
            [
                "cargo test -p parser",
                "cargo build && grep version config.toml"
            ]
        );
        // Contained, whitespace aside, and the parts of a chain may be two
        // separate commands.
        assert_eq!(
            ran(&[
                "cargo  test -p parser -- --nocapture",
                "cargo build",
                "grep version config.toml",
            ]),
            Vec::<String>::new()
        );
        assert_eq!(
            ran(&["cargo test -p parser", "cargo build"]),
            ["cargo build && grep version config.toml"]
        );
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

    #[test]
    fn an_edit_says_what_it_added_removed_and_moved() {
        let board = PlanBoard::new();
        let plan = vec![
            step("One", StepState::Done),
            step("Two", StepState::Active),
            step("Three", StepState::Todo),
        ];
        board.set(plan.clone());

        let edit = board
            .edit(
                &plan,
                vec![
                    step("One", StepState::Done),
                    step("Four", StepState::Todo),
                    step("Two", StepState::Active),
                ],
            )
            .expect("a valid edit")
            .expect("something changed")
            .1;
        assert_eq!(edit.added, ["Four"]);
        assert_eq!(edit.removed, ["Three"]);
        assert!(!edit.reordered, "the steps that stayed kept their order");

        let reminder = board.reminder().unwrap();
        assert!(
            reminder.contains("2. [ ] Four (added by the user)"),
            "{reminder}"
        );
        assert!(
            reminder.contains("added 'Four'; removed 'Three'"),
            "{reminder}"
        );
    }

    #[test]
    fn an_edit_drawn_from_a_plan_that_has_since_moved_is_refused() {
        // The model wrote a new list while the panel was open. Applying the
        // person's list would undo it without either of them knowing.
        let board = PlanBoard::new();
        let seen = vec![step("One", StepState::Active), step("Two", StepState::Todo)];
        board.set(seen.clone());
        board.set(vec![
            step("One", StepState::Done),
            step("Two", StepState::Active),
        ]);

        assert_eq!(
            board.edit(&seen, vec![step("One", StepState::Active)]),
            Err(EditRefused::Stale)
        );
    }

    #[test]
    fn an_edit_on_an_empty_board_takes_the_plan_on_screen_as_the_one_it_changed() {
        // A restart empties the board while the panel still draws the plan
        // from the transcript. The model's steps must not become the user's.
        let board = PlanBoard::new();
        let seen = vec![step("One", StepState::Active)];
        let edit = board
            .edit(
                &seen,
                vec![step("One", StepState::Active), step("Two", StepState::Todo)],
            )
            .unwrap()
            .unwrap()
            .1;
        assert_eq!(edit.added, ["Two"]);
        assert!(!board
            .reminder()
            .unwrap()
            .contains("One (added by the user)"));
    }

    #[test]
    fn a_finished_step_cannot_be_edited_away() {
        let board = PlanBoard::new();
        let plan = vec![step("One", StepState::Done), step("Two", StepState::Active)];
        board.set(plan.clone());

        for edited in [
            vec![step("Two", StepState::Active)],
            vec![
                step("One!", StepState::Done),
                step("Two", StepState::Active),
            ],
            vec![step("Two", StepState::Active), step("One", StepState::Done)],
            vec![step("One", StepState::Done), step("Two", StepState::Done)],
        ] {
            assert!(
                matches!(
                    board.edit(&plan, edited.clone()),
                    Err(EditRefused::Invalid(_))
                ),
                "{edited:?}"
            );
        }
        assert_eq!(board.steps(), plan, "a refused edit changes nothing");
    }

    #[test]
    fn an_edit_that_changes_nothing_is_not_news() {
        let board = PlanBoard::new();
        let plan = vec![step("One", StepState::Active)];
        board.set(plan.clone());
        assert_eq!(
            board.edit(&plan, vec![step(" One ", StepState::Active)]),
            Ok(None)
        );
        assert!(!board.reminder().unwrap().contains("The user edited"));
    }

    #[test]
    fn two_edits_before_the_model_reads_either_are_one_note() {
        // A step added and taken off again was never news to the model.
        let board = PlanBoard::new();
        let plan = vec![step("One", StepState::Active)];
        board.set(plan.clone());
        let with_two = vec![step("One", StepState::Active), step("Two", StepState::Todo)];
        board.edit(&plan, with_two.clone()).unwrap();
        board
            .edit(
                &with_two,
                vec![
                    step("One", StepState::Active),
                    step("Three", StepState::Todo),
                ],
            )
            .unwrap();

        let reminder = board.reminder().unwrap();
        assert!(reminder.contains("added 'Three'."), "{reminder}");
        assert!(!reminder.contains("added 'Two'"), "{reminder}");
        assert!(!reminder.contains("removed 'Two'"), "{reminder}");
    }

    #[test]
    fn a_restored_plan_is_carried_and_keeps_whose_steps_are_whose() {
        let board = PlanBoard::new();
        board.restore(RestoredPlan {
            steps: vec![step("One", StepState::Active), step("Two", StepState::Todo)],
            yours: vec!["Two".into(), "Gone".into()],
            removed: vec!["Three".into()],
            pending: None,
        });
        assert!(board.carried());
        let reminder = board.reminder().unwrap();
        assert!(reminder.contains("Two (added by the user)"), "{reminder}");
        assert!(!reminder.contains("edited this plan"), "{reminder}");

        // Something newer is already there.
        board.restore(RestoredPlan {
            steps: vec![step("Other", StepState::Todo)],
            ..RestoredPlan::default()
        });
        assert_eq!(board.steps().len(), 2);
    }

    #[test]
    fn an_edit_the_model_never_answered_is_still_news_after_a_restart() {
        let board = PlanBoard::new();
        board.restore(RestoredPlan {
            steps: vec![step("One", StepState::Done), step("Four", StepState::Todo)],
            yours: vec!["Four".into()],
            removed: vec!["Three".into()],
            pending: Some(PlanEdit {
                added: vec!["Four".into()],
                removed: vec!["Three".into()],
                reordered: false,
            }),
        });
        let reminder = board.reminder().unwrap();
        assert!(
            reminder.contains("added 'Four'; removed 'Three'"),
            "{reminder}"
        );

        assert!(
            reminder.find("edited this plan") < reminder.find("1. [x] One"),
            "the edit leads, ahead of the list: {reminder}"
        );

        // Re-sending the list is how the model says it read the note. The
        // step that came off is still named, since the older list is still in
        // the history.
        assert!(board.rules().edited);
        board.set(vec![
            step("One", StepState::Done),
            step("Four", StepState::Active),
        ]);
        let reminder = board.reminder().unwrap();
        assert!(!reminder.contains("edited this plan"), "{reminder}");
        assert!(
            reminder.contains("Don't do them, even where an earlier message lists them: 'Three'."),
            "{reminder}"
        );
    }

    #[test]
    fn a_finished_plan_takes_the_users_marks_with_it() {
        let board = PlanBoard::new();
        let plan = vec![step("One", StepState::Active)];
        board.set(plan.clone());
        board
            .edit(
                &plan,
                vec![step("One", StepState::Active), step("Two", StepState::Todo)],
            )
            .unwrap();
        board.set(vec![
            step("One", StepState::Done),
            step("Two", StepState::Done),
        ]);
        assert!(!board.start_turn());

        board.set(vec![step("Two", StepState::Active)]);
        assert!(!board.reminder().unwrap().contains("added by the user"));
    }
}
