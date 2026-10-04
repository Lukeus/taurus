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
//! It does get what the turn *claimed*: the message it ended on, and what its
//! commands and delegates actually returned, as recorded. Those are claims to
//! check, not context to reason from. "The tests pass" beside a recorded test
//! run that failed is the review's most useful finding, and it can't be made
//! from the diff alone. See [`Claims`].
//!
//! # Asking twice
//!
//! A review is fingerprinted by everything it would be sent, and kept. Asking
//! again about the same diff, claims, and model returns the review already
//! made instead of paying for a second model call, and says when it was made.
//! Asking again on purpose runs it again.
//!
//! # Where the answer goes
//!
//! Back to the caller, and from there onto the drawer beside the diff — not
//! into the transcript. A review in the conversation is a review in the context
//! window of every later request in that conversation, which is exactly the
//! cost this design exists to avoid paying.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
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
use taurus_provider::{ContentBlock, Message, Provider};
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
    /// Whether it was shown what the turn claimed as well as its diff. `false`
    /// for a turn from before turns were linked to their transcript.
    #[serde(default)]
    pub read_claims: bool,
    /// Everything the review was sent, hashed. Two reviews with the same one
    /// were asked the same question.
    #[serde(default)]
    pub fingerprint: String,
    /// When it was made, in Unix seconds.
    #[serde(default)]
    #[ts(type = "number")]
    pub at: u64,
    /// Returned from an earlier request, not made just now.
    #[serde(default)]
    pub cached: bool,
}

/// Bumped when [`BRIEF`] or the shape of what a review is sent changes, so a
/// review kept under the old brief isn't returned for the new one.
const REVIEW_VERSION: u32 = 2;

/// The most command results a review is shown: the last ones, which are the
/// ones a turn's closing claims are about.
const MAX_EVIDENCE: usize = 12;

/// How much of one result's end is shown. A test runner's verdict is on its
/// last lines.
const EVIDENCE_TAIL_LINES: usize = 3;
const EVIDENCE_LINE_CHARS: usize = 240;

/// What a turn said about itself, and what it can be checked against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Claims {
    /// The message the turn ended on.
    pub said: String,
    /// What its commands and delegates returned, one line each, from the
    /// recorded results rather than the turn's account of them.
    pub evidence: Vec<String>,
}

impl Claims {
    /// Reads a turn's claims out of its messages. `None` when it ended on
    /// nothing said.
    pub fn from_turn(messages: &[Message]) -> Option<Self> {
        let said = messages
            .iter()
            .rev()
            .find(|m| m.role == taurus_provider::Role::Assistant && !m.text().trim().is_empty())
            .map(|m| m.text().trim().to_string())?;

        let calls: std::collections::HashMap<&str, (&str, &serde_json::Value)> = messages
            .iter()
            .flat_map(|m| m.tool_uses())
            .map(|(id, name, input)| (id, (name, input)))
            .collect();
        let mut evidence = Vec::new();
        for block in messages.iter().flat_map(|m| m.content.iter()) {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = block
            else {
                continue;
            };
            let Some((name, input)) = calls.get(tool_use_id.as_str()) else {
                continue;
            };
            let text = content.to_text();
            let line = match *name {
                "run_command" => format!(
                    "`{}` {}: {}",
                    input["command"].as_str().unwrap_or("?"),
                    if *is_error { "failed" } else { "returned" },
                    tail(&text)
                ),
                "check_command" => format!(
                    "checked a background command, {}: {}",
                    if *is_error { "failed" } else { "returned" },
                    tail(&text)
                ),
                "spawn_subagent" => format!(
                    "delegated to {}: {}",
                    input["agent_type"].as_str().unwrap_or("?"),
                    text.lines().next().unwrap_or("").trim()
                ),
                _ => continue,
            };
            evidence.push(line);
        }
        let skip = evidence.len().saturating_sub(MAX_EVIDENCE);
        Some(Self {
            said,
            evidence: evidence.split_off(skip),
        })
    }
}

/// The last few non-empty lines of a result, on one line.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let start = lines.len().saturating_sub(EVIDENCE_TAIL_LINES);
    let joined = lines[start..].join(" ⏎ ");
    if joined.is_empty() {
        return "(no output)".into();
    }
    let mut cut: String = joined.chars().take(EVIDENCE_LINE_CHARS).collect();
    if cut.len() < joined.len() {
        cut.push('…');
    }
    cut
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
                     passes or fails beyond what the recorded results below show.\n\n\
                     When you are shown the message the turn ended on, treat everything it \
                     says it did as a claim, not a fact: \"the tests pass\", \"fixed\", \"I \
                     could not, because…\". Check each against the diff, the files, and the \
                     command results recorded beside it, which are what actually ran rather \
                     than the turn's account of it. After your findings, list each claim as \
                     supported, contradicted, or can't tell, with the evidence for it. A claim \
                     the recorded results contradict is the most important thing you can \
                     report.";

/// Everything one review will be sent, ready to fingerprint before it's sent.
pub struct Prepared {
    message: String,
    files: u32,
    omitted: Vec<String>,
    read_claims: bool,
    /// See [`ReviewReport::fingerprint`].
    pub fingerprint: String,
}

/// Renders what a review of `turn` would be sent. Refuses a turn with nothing
/// a reviewer could be shown.
pub fn prepare(
    changes: Vec<TurnChange>,
    claims: Option<&Claims>,
    turn: u32,
    model: &str,
) -> Result<Prepared, String> {
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

    let message = match claims {
        None => diff,
        Some(claims) => {
            let mut text = format!(
                "The turn ended with this message:\n\n<<<\n{}\n>>>\n\n",
                claims.said
            );
            if claims.evidence.is_empty() {
                text.push_str("It ran no commands and delegated nothing.\n\n");
            } else {
                text.push_str(
                    "What its commands and delegates returned, as recorded (not as it \
                     described them):\n",
                );
                for line in &claims.evidence {
                    text.push_str(&format!("- {line}\n"));
                }
                text.push('\n');
            }
            text.push_str("Its diff:\n\n");
            text.push_str(&diff);
            text
        }
    };
    let fingerprint = fingerprint_of(&[&REVIEW_VERSION.to_string(), BRIEF, model, &message]);
    Ok(Prepared {
        message,
        files,
        omitted,
        read_claims: claims.is_some(),
        fingerprint,
    })
}

/// FNV-1a over the parts, each ended by a zero byte so no two splits of the
/// same text hash alike. Stable across Rust releases, unlike `DefaultHasher`,
/// which matters for something kept on disk. A collision costs a review
/// returned for the wrong question once in 2^64, which is not worth a crypto
/// hash's dependency.
fn fingerprint_of(parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.as_bytes().iter().chain(std::iter::once(&0)) {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// Runs one review of a turn's diff alone. See [`prepare`] and [`run`] for
/// the two halves, which a caller that keeps reviews uses directly.
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
    let prepared = prepare(changes, None, turn, model)?;
    run(provider, model, registry, context, prepared, turn, cancel).await
}

/// Runs a prepared review, and returns what it found.
///
/// `provider` and `model` are the caller's, matching
/// [`crate::Host::build_agent`]: the host does not decide what a session is on,
/// and a review that quietly ran somewhere else would be a bill nobody
/// authorised.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    provider: Arc<dyn Provider>,
    model: &str,
    registry: ToolRegistry,
    context: ToolContext,
    prepared: Prepared,
    turn: u32,
    cancel: CancellationToken,
) -> Result<ReviewReport, String> {
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
            // A review is not a turn of the conversation it reads. Its tool
            // calls still meet the tool hooks.
            turn_hooks: false,
            ..Default::default()
        },
    );

    let mut session = Session::new(model);

    // Drained rather than forwarded. The reviewer's tool calls belong to the
    // review and not to any conversation, and there is no card here for them
    // to land on — this runs outside a turn by construction.
    let (tx, mut rx) = mpsc::channel::<UiEvent>(256);
    let pump = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let outcome = agent
        .run_turn(&mut session, Message::user(prepared.message), tx)
        .await;
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
        files: prepared.files,
        model: model.to_string(),
        text,
        omitted: prepared.omitted,
        read_claims: prepared.read_claims,
        fingerprint: prepared.fingerprint,
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
        cached: false,
    })
}

/// Where one conversation's reviews are kept: beside its transcript's
/// directory, keyed the same way, one line per review.
fn reviews_path(workspace: &Path, session_id: &str) -> Option<PathBuf> {
    // The same rule every other per-session file keeps: an id that isn't a
    // plain name is not turned into a path.
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(
        crate::config::home_dir()
            .join("reviews")
            .join(crate::sessions::workspace_key(workspace))
            .join(format!("{session_id}.jsonl")),
    )
}

/// The review already made with this fingerprint, if there is one. The most
/// recent, when there are several.
pub fn stored(workspace: &Path, session_id: &str, fingerprint: &str) -> Option<ReviewReport> {
    let file = std::fs::File::open(reviews_path(workspace, session_id)?).ok()?;
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<ReviewReport>(&line).ok())
        .filter(|report| report.fingerprint == fingerprint)
        .last()
        .map(|report| ReviewReport {
            cached: true,
            ..report
        })
}

/// Keeps a review. A failure is logged and otherwise ignored: losing the copy
/// costs a second model call later, not the review in hand.
pub fn store(workspace: &Path, session_id: &str, report: &ReviewReport) {
    let Some(path) = reviews_path(workspace, session_id) else {
        return;
    };
    let written = (|| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut line = serde_json::to_vec(report)?;
        line.push(b'\n');
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(&line)
    })();
    if let Err(e) = written {
        tracing::warn!(path = %path.display(), error = %e, "could not keep a review");
    }
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

    fn turn_with_claims() -> Vec<Message> {
        let call = |id: &str, name: &str, input: serde_json::Value| ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
            signature: None,
        };
        let result = |id: &str, text: &str, is_error: bool| ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: text.into(),
            is_error,
        };
        vec![
            Message::user("fix the parser and make sure the tests pass"),
            Message::new(
                taurus_provider::Role::Assistant,
                vec![
                    call("c1", "run_command", serde_json::json!({ "command": "cargo test" })),
                    call(
                        "c2",
                        "spawn_subagent",
                        serde_json::json!({ "agent_type": "explorer", "prompt": "…" }),
                    ),
                    call("c3", "read_file", serde_json::json!({ "path": "a.rs" })),
                ],
            ),
            Message::new(
                taurus_provider::Role::User,
                vec![
                    result(
                        "c1",
                        "running 3 tests\ntest a ... ok\ntest b ... FAILED\n\ntest result: FAILED. 2 passed; 1 failed",
                        true,
                    ),
                    result("c2", "Status: done.\n\nThe parser is in src/parse.rs.", false),
                    result("c3", "fn main() {}", false),
                ],
            ),
            Message::assistant("Fixed the parser. All tests pass."),
        ]
    }

    #[test]
    fn claims_are_what_the_turn_said_and_evidence_is_what_it_ran() {
        let claims = Claims::from_turn(&turn_with_claims()).unwrap();
        assert_eq!(claims.said, "Fixed the parser. All tests pass.");
        assert_eq!(
            claims.evidence.len(),
            2,
            "reads aren't evidence: {:?}",
            claims.evidence
        );
        assert!(
            claims.evidence[0].starts_with("`cargo test` failed:")
                && claims.evidence[0].contains("1 failed"),
            "the verdict is on the last lines: {}",
            claims.evidence[0]
        );
        assert_eq!(claims.evidence[1], "delegated to explorer: Status: done.");
    }

    #[test]
    fn a_turn_that_said_nothing_has_no_claims() {
        assert!(Claims::from_turn(&[Message::user("hi")]).is_none());
    }

    fn one_change() -> Vec<TurnChange> {
        vec![TurnChange::Diff {
            diff: diff(
                "src/parse.rs",
                vec![line(DiffLineKind::Added, "fn parse() {}", None, Some(1))],
            ),
        }]
    }

    #[test]
    fn a_review_with_claims_is_shown_them_beside_the_diff() {
        let claims = Claims::from_turn(&turn_with_claims()).unwrap();
        let prepared = prepare(one_change(), Some(&claims), 1, "m").unwrap();
        assert!(prepared.read_claims);
        let sent = &prepared.message;
        let said = sent.find("All tests pass.").unwrap();
        let ran = sent.find("`cargo test` failed").unwrap();
        let diff = sent.find("fn parse()").unwrap();
        assert!(
            said < ran && ran < diff,
            "claims, then evidence, then the diff: {sent}"
        );

        let alone = prepare(one_change(), None, 1, "m").unwrap();
        assert!(!alone.read_claims);
        assert!(!alone.message.contains("The turn ended with"));
    }

    #[test]
    fn the_fingerprint_changes_with_anything_the_review_is_sent() {
        let claims = Claims::from_turn(&turn_with_claims()).unwrap();
        let fp = |claims: Option<&Claims>, model: &str| {
            prepare(one_change(), claims, 1, model).unwrap().fingerprint
        };
        assert_eq!(fp(Some(&claims), "m"), fp(Some(&claims), "m"), "stable");
        assert_ne!(fp(Some(&claims), "m"), fp(None, "m"));
        assert_ne!(fp(Some(&claims), "m"), fp(Some(&claims), "other"));
        let mut said_otherwise = claims.clone();
        said_otherwise.said.push_str(" Also refactored.");
        assert_ne!(fp(Some(&claims), "m"), fp(Some(&said_otherwise), "m"));
    }

    #[test]
    fn a_kept_review_comes_back_for_the_same_question_only() {
        let _home = crate::testing::isolated_home();
        let workspace = Path::new("/tmp/project");
        let report = ReviewReport {
            turn: 1,
            files: 1,
            model: "m".into(),
            text: "Looks right.".into(),
            omitted: vec![],
            read_claims: true,
            fingerprint: "abc".into(),
            at: 1_700_000_000,
            cached: false,
        };
        assert!(stored(workspace, "s1", "abc").is_none());
        store(workspace, "s1", &report);

        let kept = stored(workspace, "s1", "abc").expect("kept");
        assert!(kept.cached, "marked as the earlier answer");
        assert_eq!(kept.text, "Looks right.");
        assert!(stored(workspace, "s1", "different").is_none());
        assert!(stored(workspace, "s2", "abc").is_none(), "per conversation");
        assert!(stored(workspace, "../escape", "abc").is_none());
    }
}
