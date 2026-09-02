//! Reading a turn back from a context that did not write it.
//!
//! A turn is already recoverable as a diff — see
//! [`taurus_tools::checkpoint::CheckpointStore::changes`] — and a delegate
//! already shares none of its parent's context. This is the two of them put
//! together, which is the only arrangement in which "check this over" means
//! anything: an agent asked to review its own turn is asked to find a mistake
//! using the reasoning that made it, and it will confidently report that the
//! code is fine. Handing the same diff to a context with no memory of writing
//! it is not a trick, it is the whole of the idea.
//!
//! # Why it is a verb and not an agent
//!
//! There is no `reviewer` in [`taurus_agents::builtin`], deliberately. That
//! roster is a line each in the spawn tool's description, paid on every request
//! of every turn whether or not anybody delegates — and a review is something a
//! person asks for after looking at a diff, perhaps once an hour. A roster line
//! would charge every conversation for a button. So this is reached from the
//! Changes drawer and from `taurus review`, and the model is never told it
//! exists.
//!
//! That also settles what the reviewer may do. It is not choosing its own
//! scope from a description; the scope is fixed here, and it is `explorer`'s —
//! read, search, and nothing else. Its tool list is taken from that definition
//! rather than written out again, so "it can only read" has one source.
//!
//! # What it is given, and what it is not
//!
//! It gets the diff and the workspace. It does not get the conversation: not
//! the request, not the plan, not what the user said when they rejected the
//! first attempt. That is the point and it is also the cost, and the cost is
//! real — it cannot know that a function was left unused on purpose, so it will
//! say so. A reviewer given the request back would be a reviewer reasoning from
//! the context that wrote the code, which is the thing there was no point
//! running.
//!
//! # Where the answer goes
//!
//! Back to the caller, and from there onto the drawer beside the diff — not
//! into the transcript. A review in the conversation is a review in the context
//! window of every later request in that conversation, which is exactly the
//! cost this design exists to avoid paying.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use taurus_agents::builtin;
use taurus_core::agent::{Agent, AgentConfig};
use taurus_core::event::UiEvent;
use taurus_core::subagent::SPAWN_TOOL;
use taurus_core::Session;
use taurus_provider::{Message, Provider};
use taurus_tools::checkpoint::TurnChange;
use taurus_tools::diff::{DiffLineKind, FileDiff};
use taurus_tools::{ToolContext, ToolRegistry};

/// How much diff the reviewer is given.
///
/// A turn that rewrote half the repository is not a turn anyone reviews in one
/// pass, and a model on an 8k window would spend the whole of it on the diff
/// with nothing left to read the surrounding code with. Files past this are
/// named in [`ReviewReport::omitted`] rather than silently dropped.
const MAX_DIFF_BYTES: usize = 24 * 1024;

/// Ceiling on the reviewer's own tool round trips.
///
/// It reads around the diff and answers; it does not build anything. The same
/// budget `explorer` is given, for the same work.
const MAX_ITERATIONS: u32 = 20;

/// What a review found, and what it was not shown.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewReport {
    pub turn: u32,
    /// Files whose diff reached the reviewer.
    pub files: u32,
    /// What it ran on, so a report read later says what produced it.
    pub model: String,
    /// The reviewer's reply, as Markdown.
    pub text: String,
    /// Files the turn changed that the reviewer did not see: the ones with no
    /// readable diff, and any dropped to fit [`MAX_DIFF_BYTES`].
    ///
    /// Named rather than quietly absent. A review that covered four of a
    /// turn's six files and did not say so is worse than no review, because it
    /// reads as a clean bill of health for all six.
    pub omitted: Vec<String>,
}

/// The brief the reviewer works from.
///
/// It says what the reviewer cannot know as plainly as what it should do,
/// because the failure mode of this feature is not a missed bug — it is a
/// confident paragraph about a deliberate decision. A reviewer that has been
/// told it cannot see the request writes "if this was intended, ignore me",
/// and one that has not been told writes "this is wrong".
const BRIEF: &str = "You are reviewing a change you did not write, in a codebase you have not \
                     read. You are given the diff of one turn and nothing else — not the request \
                     that produced it, not the conversation around it, and not the plan it was \
                     part of. That is deliberate: an agent reviewing its own work reasons from \
                     the same context that made the mistake, and you are here because you do \
                     not.\n\n\
                     Read the surrounding code before judging any of it. A diff shows what \
                     moved and not what it has to fit, and most of what looks wrong in a hunk \
                     is answered by the file it is in.\n\n\
                     Report what would actually break, hardest first: a case the change does \
                     not handle, an invariant it drops, an error path it swallows, a caller it \
                     leaves inconsistent. Say where, by file and line. If the change looks \
                     correct, say so in a sentence rather than finding something to fill the \
                     space.\n\n\
                     You cannot see why this was done. Where something looks wrong but would be \
                     reasonable under an intent you were not told, say that rather than \
                     asserting it is a defect. You also cannot run anything: do not claim a test \
                     passes or fails.";

/// Runs one review, and returns what it found.
///
/// `provider` and `model` are the caller's, matching
/// [`crate::Host::build_agent`]: the host does not decide what a session is on,
/// and a review that quietly ran somewhere else would be a bill nobody
/// authorised.
#[allow(clippy::too_many_arguments)]
pub async fn review(
    provider: Arc<dyn Provider>,
    model: &str,
    registry: ToolRegistry,
    // Carries the workspace the reviewer reads in, which is why one is not
    // passed beside it: two sources for the same path is one that can be wrong.
    context: ToolContext,
    changes: Vec<TurnChange>,
    turn: u32,
    cancel: CancellationToken,
) -> Result<ReviewReport, String> {
    if changes.is_empty() {
        return Err(format!(
            "Turn {turn} changed no files, so there is nothing to review."
        ));
    }

    let Rendered {
        text: diff,
        files,
        omitted,
    } = render(changes);

    if files == 0 {
        return Err(format!(
            "Turn {turn} changed {} file{}, and none of them has a diff that can be read — \
             they are binary, or their earlier contents are no longer recorded. There is \
             nothing here a reviewer could be shown.",
            omitted.len(),
            if omitted.len() == 1 { "" } else { "s" }
        ));
    }

    // The depth cap the delegate path relies on, applied here for the same
    // reason: a reviewer that could spawn is a reviewer that could spend a
    // conversation's budget on work nobody asked for.
    let registry = registry.without(SPAWN_TOOL);

    // `explorer`'s scope rather than a second list, so the claim in this
    // module's docs and the scope in the registry cannot drift apart. A tool
    // it names that is not registered here is dropped rather than fatal — the
    // list is all built-ins, so an empty result would mean a registry with no
    // built-in tools at all, which is refused below rather than run wide open.
    let allowed: Vec<String> = builtin::definitions()
        .into_iter()
        .find(|agent| agent.name() == builtin::EXPLORER)
        .and_then(|agent| agent.frontmatter.tools)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| registry.get(name).is_some())
        .collect();
    if allowed.is_empty() {
        // Empty `allowed_tools` means "everything registered", so falling
        // through here would hand a reviewer the shell. The same refusal
        // `SpawnSubagent` makes at the same point, and for the same reason.
        return Err(
            "None of the tools a review reads with are available in this session, so it \
             would run unrestricted. It has been refused instead."
                .into(),
        );
    }

    let agent = Agent::new(
        provider,
        registry,
        context,
        AgentConfig {
            system_prompt: BRIEF.into(),
            max_iterations: MAX_ITERATIONS,
            allowed_tools: allowed,
            // Nothing was changed, so there is nothing to check afterwards.
            // Left on, the nudge asks a read-only agent to go and run its work.
            verify_changes: false,
            ..Default::default()
        },
    );

    let mut session = Session::new(model);

    // Drained rather than forwarded. The reviewer's tool calls belong to the
    // review and not to any conversation, and there is no card here for them
    // to land on — this runs outside a turn by construction.
    let (tx, mut rx) = mpsc::channel::<UiEvent>(256);
    let pump = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let outcome = agent.run_turn(&mut session, Message::user(diff), tx).await;
    let _ = pump.await;

    if cancel.is_cancelled() {
        return Err("Review stopped.".into());
    }
    outcome.map_err(|e| e.to_string())?;

    let text = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == taurus_provider::Role::Assistant && !m.text().trim().is_empty())
        .map(|m| m.text())
        .ok_or_else(|| {
            "The reviewer finished without saying anything. Nothing has been changed.".to_string()
        })?;

    Ok(ReviewReport {
        turn,
        files,
        model: model.to_string(),
        text,
        omitted,
    })
}

/// The diff as the reviewer receives it.
struct Rendered {
    text: String,
    files: u32,
    omitted: Vec<String>,
}

/// Renders a turn's changes as one unified diff, capped.
///
/// Ordinary unified format rather than anything of this project's own: a model
/// has read a great deal more of that than of any format invented here, and the
/// bytes spent on `@@` markers buy comprehension the same bytes of prose would
/// not.
fn render(changes: Vec<TurnChange>) -> Rendered {
    let mut text = String::new();
    let mut files = 0u32;
    let mut omitted = Vec::new();

    for change in changes {
        match change {
            // A file with no readable before or after. Named, and that is all
            // that can be done with it.
            TurnChange::Opaque { path, .. } => omitted.push(path),
            TurnChange::Diff { diff } => {
                let rendered = one_file(&diff);
                // Checked before appending rather than truncating mid-hunk: a
                // diff cut in the middle reads as a change that ends where it
                // does, and a reviewer would report on the half it was given.
                if text.len() + rendered.len() > MAX_DIFF_BYTES && files > 0 {
                    omitted.push(diff.path);
                    continue;
                }
                text.push_str(&rendered);
                files += 1;
            }
        }
    }

    Rendered {
        text,
        files,
        omitted,
    }
}

/// One file's diff, in unified form.
///
/// The line numbers come from the hunk's own lines rather than a header this
/// would have to compute, which is what lets a reviewer say "line 91" and mean
/// the file rather than the diff.
fn one_file(diff: &FileDiff) -> String {
    let mut out = String::new();
    let verb = if diff.created {
        " (new file)"
    } else if diff.deleted {
        " (deleted)"
    } else {
        ""
    };
    out.push_str(&format!("--- {}{verb}\n", diff.path));

    for hunk in &diff.hunks {
        let first = hunk
            .lines
            .iter()
            .find_map(|line| line.new_line.or(line.old_line))
            .unwrap_or(1);
        out.push_str(&format!("@@ line {first} @@\n"));
        for line in &hunk.lines {
            let mark = match line.kind {
                DiffLineKind::Added => '+',
                DiffLineKind::Removed => '-',
                DiffLineKind::Context => ' ',
            };
            out.push(mark);
            out.push_str(&line.text);
            out.push('\n');
        }
    }

    // The per-file cap the diff itself applied, before this one. Said in the
    // text the reviewer reads rather than only in the report the user reads:
    // it is the reviewer that would otherwise conclude a function has no
    // caller, from a hunk list that simply stopped.
    if diff.elided > 0 {
        out.push_str(&format!(
            "... {} further changed line{} in this file are not shown\n",
            diff.elided,
            if diff.elided == 1 { "" } else { "s" }
        ));
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use taurus_tools::diff::{DiffHunk, DiffLine};

    fn line(kind: DiffLineKind, text: &str, old: Option<usize>, new: Option<usize>) -> DiffLine {
        DiffLine {
            kind,
            text: text.into(),
            old_line: old,
            new_line: new,
        }
    }

    fn diff(path: &str, lines: Vec<DiffLine>) -> FileDiff {
        FileDiff {
            path: path.into(),
            created: false,
            deleted: false,
            added: 1,
            removed: 1,
            hunks: vec![DiffHunk { lines }],
            elided: 0,
        }
    }

    #[test]
    fn a_hunk_carries_the_line_the_file_would_show() {
        // A reviewer that says "line 91" has to mean the file, not the ninety
        // first line of the diff it was handed.
        let rendered = one_file(&diff(
            "src/lib.rs",
            vec![
                line(DiffLineKind::Context, "fn main() {", Some(90), Some(90)),
                line(DiffLineKind::Removed, "    old();", Some(91), None),
                line(DiffLineKind::Added, "    new();", None, Some(91)),
            ],
        ));
        assert!(rendered.contains("@@ line 90 @@"), "{rendered}");
        assert!(rendered.contains("-    old();"), "{rendered}");
        assert!(rendered.contains("+    new();"), "{rendered}");
    }

    #[test]
    fn a_file_with_no_readable_diff_is_named_rather_than_dropped() {
        let rendered = render(vec![
            TurnChange::Diff {
                diff: diff(
                    "src/lib.rs",
                    vec![line(DiffLineKind::Added, "x", None, Some(1))],
                ),
            },
            TurnChange::Opaque {
                path: "logo.png".into(),
                reason: "is not text".into(),
            },
        ]);
        assert_eq!(rendered.files, 1);
        assert_eq!(rendered.omitted, vec!["logo.png".to_string()]);
    }

    #[test]
    fn a_turn_past_the_cap_keeps_whole_files_and_names_the_rest() {
        // Whole files or none. A diff cut mid-hunk reads as a change that ends
        // where it does, and the reviewer would report on the half it saw.
        let big = |path: &str| TurnChange::Diff {
            diff: diff(
                path,
                (0..2_000)
                    .map(|i| line(DiffLineKind::Added, "some added line", None, Some(i)))
                    .collect(),
            ),
        };
        let rendered = render(vec![big("a.rs"), big("b.rs"), big("c.rs")]);

        assert!(rendered.files >= 1, "the first file is always kept");
        assert!(!rendered.omitted.is_empty(), "the rest are named");
        assert_eq!(rendered.files as usize + rendered.omitted.len(), 3);
        // Every kept file ends where a file ends, never inside a hunk.
        assert!(rendered.text.ends_with("\n\n"), "cut mid-file");
    }

    #[test]
    fn a_single_file_larger_than_the_cap_is_still_reviewed() {
        // The alternative is a review that refuses the turn most worth
        // reviewing. One file over the cap is kept whole; it is the second that
        // starts being dropped.
        let rendered = render(vec![TurnChange::Diff {
            diff: diff(
                "huge.rs",
                (0..4_000)
                    .map(|i| line(DiffLineKind::Added, "some added line", None, Some(i)))
                    .collect(),
            ),
        }]);
        assert_eq!(rendered.files, 1);
        assert!(rendered.omitted.is_empty());
        assert!(rendered.text.len() > MAX_DIFF_BYTES);
    }

    #[test]
    fn a_diff_that_was_already_elided_says_so_to_the_reviewer() {
        // `FileDiff` caps itself at 160 lines long before this sees it. A
        // reviewer not told that concludes a function has no caller from a
        // hunk list that merely stopped.
        let mut d = diff(
            "src/lib.rs",
            vec![line(DiffLineKind::Added, "x", None, Some(1))],
        );
        d.elided = 40;
        let rendered = one_file(&d);
        assert!(rendered.contains("40 further changed lines"), "{rendered}");
    }

    #[test]
    fn a_new_file_and_a_deleted_one_say_which_they_are() {
        let mut created = diff(
            "new.rs",
            vec![line(DiffLineKind::Added, "x", None, Some(1))],
        );
        created.created = true;
        assert!(one_file(&created).contains("(new file)"));

        let mut deleted = diff(
            "old.rs",
            vec![line(DiffLineKind::Removed, "x", Some(1), None)],
        );
        deleted.deleted = true;
        assert!(one_file(&deleted).contains("(deleted)"));
    }
}
