//! Pre-images of files the agent is about to change, so a turn can be undone.
//!
//! Taurus has no other undo. A model that rewrites the wrong file, or gets an
//! `edit_file` subtly wrong across a dozen call sites, has destroyed work that
//! nothing else in the harness records — the transcript remembers the *call*,
//! not the bytes that were there first.
//!
//! So the bytes are kept. Before a tool changes a file, its current contents go
//! into an append-only log under `~/.taurus/checkpoints/<workspace>/<id>.jsonl`,
//! the same shape and for the same reasons as a session transcript: one JSON
//! object per line, nothing rewritten, a torn final line dropped on load rather
//! than poisoning the file.
//!
//! **A turn owns the records that follow it.** Numbers are assigned when the
//! log is read, and order is what associates a pre-image with the turn that
//! took it. That is what lets a sub-agent's writes land in the turn that
//! spawned it without anyone passing an identifier down. The single exception
//! is [`Record::Committed`], which is written long after its turn closed and
//! so has to name it; see there for why a number is safe to write down.
//!
//! **A rewind says what it cannot put back.** Restoring files is the whole of
//! what this can do, and three things routinely make that not enough: the turn
//! moved `HEAD`, the turn was kept as a commit, or the workspace has since
//! moved to another branch. None are recoverable here and all are knowable, so
//! the log records them and [`CheckpointStore::rewind`] reports them beside the
//! plan — before the button, not after it.
//!
//! Pre-images arrive two ways. A tool that can name what it will change
//! declares it through [`crate::Tool::touches`], and [`TurnRecorder::capture`]
//! reads the file just before the call — that is `write_file` and `edit_file`.
//! A tool that cannot name anything is covered by [`crate::sweep`], which
//! indexes the workspace around the call and hands back the pre-images it
//! already holds through [`TurnRecorder::capture_state`] — that is
//! `run_command`, whose reach is only knowable by looking afterwards.
//!
//! Both land in the same log, and a rewind cannot tell them apart. What it can
//! tell is when a pre-image is missing: a file too large to hold, or one that
//! was never text, is recorded as [`State::Opaque`] and reported as skipped
//! rather than quietly left out. A checkpoint that covered less than it
//! appeared to would be worse than none.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use ts_rs::TS;

use crate::diff::FileDiff;

/// Bumped when a record shape changes incompatibly. A log written by a newer
/// Taurus is refused rather than half-understood — a partial rewind is the one
/// outcome worse than no rewind.
///
/// Format 2 added the three facts a rewind needs in order to warn: the branch a
/// turn ran on, whether it moved git's own state, and whether it was later
/// committed. An older build skips records it does not know, which here would
/// mean rewinding past a committed turn without a word — precisely the
/// half-understanding this guard exists for. So a log that gains a format-2
/// record gains a format-2 header with it, and older builds refuse it rather
/// than under-report it. See [`ensure_header`].
const FORMAT_VERSION: u32 = 2;

const EXTENSION: &str = "jsonl";

/// How much of the prompt is kept to label a turn in a listing.
const LABEL_MAX_CHARS: usize = 80;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// One line of a checkpoint log.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record {
    Header(Header),
    /// Opens a turn. Every `Before` after it belongs to it.
    Turn {
        prompt: String,
        at: u64,
        /// The branch checked out when the turn began, so a rewind can say
        /// that these pre-images came out of a tree that is no longer checked
        /// out. `None` outside a repository, on a detached `HEAD`, and in
        /// every format-1 log — all three of which mean the same thing here,
        /// which is that there is nothing to compare and so nothing to say.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    Before {
        path: String,
        state: State,
    },
    /// The turn this follows also moved `HEAD` or the ref it names.
    ///
    /// Written once per turn by [`crate::sweep`], which is the only thing that
    /// looks. Position is the association, the same as `Before`: the sweep
    /// notices while the turn is open, so there is nothing to name.
    MovedGit,
    /// A turn was kept as a commit.
    ///
    /// The one record carrying a turn number rather than relying on position.
    /// A commit is made from the drawer long after the turn closed, so it
    /// lands at the end of the log instead of inside the turn it describes.
    /// The number is safe to write down because the log is append-only and
    /// turns only ever arrive at the end: turn 3 is turn 3 for the life of the
    /// conversation, and a rewind does not renumber what it undoes.
    Committed {
        turn: u32,
        sha: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    session: String,
    workspace: String,
}

/// What a file looked like before the turn touched it.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum State {
    /// It did not exist. Undoing means deleting it again.
    Absent,
    /// Its contents. Every path the file tools take is UTF-8 — `write_file`
    /// receives a string and `edit_file` reads one — so the text goes inline
    /// rather than into a blob store that would need its own garbage
    /// collection.
    Text { content: String },
    /// It existed, but its contents are not here to put back — it was not
    /// text, could not be read, or was too large for [`crate::sweep`] to hold.
    /// Recorded anyway: a rewind has to be able to say which files it could not
    /// restore, rather than leaving the user to notice.
    ///
    /// `reason` is a complete phrase following the file's name, because that is
    /// how a rewind reports it: "config.db was not text when it was recorded".
    Opaque { reason: String },
}

/// Reads what a file holds right now, as a pre-image.
///
/// Shared with [`crate::sweep`], so a file that is missing, unreadable, or not
/// text is classified the same way whichever side captured it.
pub(crate) fn read_state(path: &Path) -> State {
    match std::fs::read_to_string(path) {
        Ok(content) => State::Text { content },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::Absent,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => State::Opaque {
            reason: "was not text when it was recorded".into(),
        },
        Err(e) => State::Opaque {
            reason: format!("could not be read when it was recorded ({e})"),
        },
    }
}

/// A turn that changed files, as a listing shows it.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Checkpoint {
    /// Position in the log, oldest first, starting at 1.
    pub turn: u32,
    /// The request that led to these changes, shortened.
    pub prompt: String,
    /// Unix seconds. Formatting is the frontend's business.
    #[ts(type = "number")]
    pub at: u64,
    /// Workspace-relative paths this turn was the first to touch.
    pub files: Vec<String>,
    /// The branch it ran on. `None` outside a repository, on a detached
    /// `HEAD`, and for any turn recorded before format 2.
    pub branch: Option<String>,
    /// It moved `HEAD` or a branch ref as well as files, so undoing it puts
    /// back less than it appears to.
    pub moved_git: bool,
    /// The commit it was kept as, if it was kept. What makes the drawer able
    /// to say which turns are already in `HEAD` — and so which ones a commit
    /// made now would sit on top of.
    pub commit: Option<String>,
}

/// What one turn did to one file.
///
/// The log already holds both halves of this. A turn records what a file held
/// before it touched it, so the pre-image the *next* turn to touch that file
/// recorded is, by construction, what this turn left behind — and for the last
/// turn to touch it, what it left behind is what is on disk now. No second
/// store, no post-images, and nothing extra written per turn.
///
/// The seam is a hand edit between two turns, which lands in the later turn's
/// diff rather than being attributed to nobody. That is the same assumption
/// [`CheckpointStore::rewind`] already makes when it says a rewind will
/// overwrite "anything you changed by hand since", and it fails the same way:
/// visibly, in a direction the user can reason about.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum TurnChange {
    /// The before and after, ready to draw.
    Diff { diff: FileDiff },
    /// The file changed, and one side of it is not here to show. A rewind
    /// reports the same files as skipped, for the same reason.
    ///
    /// `reason` is a complete phrase following the file's name.
    Opaque { path: String, reason: String },
}

impl TurnChange {
    pub fn path(&self) -> &str {
        match self {
            Self::Diff { diff } => &diff.path,
            Self::Opaque { path, .. } => path,
        }
    }
}

/// What a rewind did to one file.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case")]
#[ts(export)]
pub enum Restored {
    /// Put back to its earlier contents.
    Reverted { path: String },
    /// Removed, because the turn created it.
    Deleted { path: String },
    /// Left alone, and why.
    Skipped { path: String, reason: String },
}

impl Restored {
    pub fn path(&self) -> &str {
        match self {
            Self::Reverted { path } | Self::Deleted { path } | Self::Skipped { path, .. } => path,
        }
    }
}

/// What a rewind did, and what it could not do.
///
/// The warnings are the reason this is a struct rather than the list of files
/// it used to be. Every one of them names something a rewind is structurally
/// unable to put back — git's own state, a commit already made, a branch that
/// has since changed underneath the pre-images — and each was previously either
/// said at the wrong moment or not said at all. They are produced identically
/// for a dry run and a real one, because the moment they are worth reading is
/// the moment before the button.
#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Rewind {
    /// One entry per file the rewind touched or declined to touch.
    pub restored: Vec<Restored>,
    /// Complete sentences, in the order the turns they describe ran. Empty is
    /// the ordinary case: a rewind of turns that only changed files has
    /// nothing to add.
    pub warnings: Vec<String>,
}

/// The checkpoint logs for one workspace.
///
/// Holds a directory and nothing else: which session is being written is
/// decided per turn by [`Self::begin_turn`], because one workspace outlives
/// many conversations.
pub struct CheckpointStore {
    dir: PathBuf,
}

impl CheckpointStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn log_path(&self, session_id: &str) -> Option<PathBuf> {
        // An id becomes a filename, so one carrying separators would resolve
        // outside the checkpoint tree.
        if session_id.is_empty() || session_id.contains(['/', '\\', '.']) {
            return None;
        }
        Some(self.dir.join(format!("{session_id}.{EXTENSION}")))
    }

    /// Opens a turn and returns the recorder the tools write through.
    ///
    /// Nothing is written here. The turn's header goes down with the first file
    /// captured, so a log holds only turns there is something to undo — which
    /// is what makes a listing worth reading.
    pub fn begin_turn(
        &self,
        session_id: &str,
        workspace: &Path,
        prompt: &str,
    ) -> Arc<TurnRecorder> {
        Arc::new(TurnRecorder {
            path: self.log_path(session_id),
            session: session_id.to_string(),
            // Read now rather than when the first file lands, because a turn
            // that checks out another branch and then edits a file would
            // otherwise record the branch it moved to as the one it started on
            // — and that is the exact case the warning exists for.
            branch: crate::sweep::branch(workspace),
            workspace: workspace.to_path_buf(),
            prompt: shorten(prompt),
            state: Mutex::new(TurnState::default()),
        })
    }

    /// Every turn in a session that changed a file, oldest first.
    pub fn turns(&self, session_id: &str) -> Result<Vec<Checkpoint>, String> {
        let Some(path) = self.log_path(session_id) else {
            return Err(format!("'{session_id}' is not a usable session id"));
        };
        let mut log = read_log(&path)?;
        // Taken out so the turns can be consumed while the commits they are
        // matched against stay borrowable.
        let turns = std::mem::take(&mut log.turns);
        Ok(turns
            .into_iter()
            .enumerate()
            .map(|(index, turn)| {
                let number = index as u32 + 1;
                Checkpoint {
                    turn: number,
                    prompt: turn.prompt,
                    at: turn.at,
                    files: turn.changes.into_iter().map(|(path, _)| path).collect(),
                    branch: turn.branch,
                    moved_git: turn.moved_git,
                    commit: log.commit_for(number),
                }
            })
            .collect())
    }

    /// What one turn changed, file by file, as a diff.
    ///
    /// The turn numbers are the ones [`Self::turns`] hands out, and the files
    /// are that turn's own — the ones it was the first to touch. See
    /// [`TurnChange`] for where the after-image comes from.
    pub fn changes(
        &self,
        session_id: &str,
        workspace: &Path,
        turn: u32,
    ) -> Result<Vec<TurnChange>, String> {
        let Some(path) = self.log_path(session_id) else {
            return Err(format!("'{session_id}' is not a usable session id"));
        };
        let turns = read_log(&path)?.turns;
        let index = check_turn(session_id, turn, turns.len())?;

        // Split rather than indexed twice, so the "what came after" search
        // cannot accidentally include the turn being described.
        let (through, later) = turns.split_at(index + 1);
        let entry = through.last().expect("check_turn bounded the index");

        Ok(entry
            .changes
            .iter()
            .map(|(file, before)| {
                // The earliest later pre-image of this same file is what this
                // turn left behind. Iterating turns in order and taking the
                // first match is exactly that; nothing later can be closer.
                let after = later
                    .iter()
                    .flat_map(|turn| turn.changes.iter())
                    .find(|(seen, _)| seen == file)
                    .map(|(_, state)| state.clone())
                    .unwrap_or_else(|| state_now(workspace, file));
                describe(file, before, &after)
            })
            .collect())
    }

    /// Discards a session's log, and with it every turn it could have undone.
    ///
    /// For a conversation being deleted: the log is keyed by session id and
    /// reachable only through it, so one left behind is a copy of the user's
    /// files that nothing can read, list, or ever restore.
    ///
    /// A missing log is success, unlike everything else that takes an id here.
    /// A conversation that only read files never writes one, so "no log" is the
    /// ordinary outcome rather than a sign that something went wrong.
    pub fn forget(&self, session_id: &str) -> Result<(), String> {
        let Some(path) = self.log_path(session_id) else {
            return Err(format!("'{session_id}' is not a usable session id"));
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Puts `workspace` back the way it was before `turn` ran.
    ///
    /// Every turn from `turn` onward is undone, not just that one: the log
    /// records what a file looked like before a turn, and skipping a later turn
    /// would restore a file to a state that never coexisted with the rest of
    /// the tree.
    ///
    /// Where two turns touched the same file, the earliest pre-image wins —
    /// that is the one that predates all of them.
    ///
    /// A dry run produces the same warnings as a real one, which is the point
    /// of having them: the drawer and the CLI both show the plan first, and a
    /// warning that only arrived afterwards would be a report rather than a
    /// chance to stop.
    pub fn rewind(
        &self,
        session_id: &str,
        workspace: &Path,
        turn: u32,
        dry_run: bool,
    ) -> Result<Rewind, String> {
        let Some(path) = self.log_path(session_id) else {
            return Err(format!("'{session_id}' is not a usable session id"));
        };
        let log = read_log(&path)?;
        let index = check_turn(session_id, turn, log.turns.len())?;
        let undoing = &log.turns[index..];

        // First writer wins, so iterating forward from the target turn leaves
        // each path holding the oldest pre-image at or after it.
        let mut earliest: Vec<(String, State)> = Vec::new();
        for entry in undoing {
            for (file, state) in &entry.changes {
                if !earliest.iter().any(|(seen, _)| seen == file) {
                    earliest.push((file.clone(), state.clone()));
                }
            }
        }
        earliest.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(Rewind {
            restored: earliest
                .into_iter()
                .map(|(file, state)| restore(workspace, &file, &state, dry_run))
                .collect(),
            warnings: warnings(&log, workspace, turn, undoing),
        })
    }

    /// Records that `turn` was kept as `sha`.
    ///
    /// Written after the commit rather than before it: a commit that failed
    /// halfway is not one a rewind should warn about, and the sha does not
    /// exist to write down until git has made it.
    ///
    /// Best-effort, like every other write here. Losing it costs the warning
    /// and the drawer's knowledge that this turn is already in `HEAD`; it
    /// cannot cost the commit, which is already made, or the undo, which does
    /// not depend on it.
    pub fn record_commit(&self, session_id: &str, workspace: &Path, turn: u32, sha: &str) {
        let Some(log) = self.log_path(session_id) else {
            return;
        };
        // A log being committed from has always been written to, so the header
        // is there. Asked for anyway, because what this appends is a format-2
        // record and a log still carrying a format-1 header would let an older
        // build skip it silently.
        if !ensure_header(&log, session_id, workspace) {
            return;
        }
        let _ = append(
            &log,
            &Record::Committed {
                turn,
                sha: sha.to_string(),
            },
        );
    }
}

/// What this rewind cannot put back, as complete sentences.
///
/// Ordered the way the turns ran, because that is how the two per-turn
/// warnings read as a sequence of events. The branch warning is one line for
/// the whole rewind rather than one per turn: it is a fact about where the
/// files are being written, not about any turn, and repeating it per turn
/// would bury the two that are.
fn warnings(log: &ReadLog, workspace: &Path, first: u32, undoing: &[ReadTurn]) -> Vec<String> {
    let mut warnings = Vec::new();

    for (offset, entry) in undoing.iter().enumerate() {
        let number = first + offset as u32;
        if entry.moved_git {
            warnings.push(format!(
                "Turn {number} moved git's own state. Its files come back; HEAD and the \
                 index stay where the command left them, so the result will match neither \
                 commit. `git reflog` is the way back to where HEAD was."
            ));
        }
        if let Some(sha) = log.commit_for(number) {
            warnings.push(format!(
                "Turn {number} was committed as {sha}. Undoing it leaves that commit in \
                 place, so the tree will no longer match it — `git revert {sha}` undoes it \
                 as a new commit, `git reset` moves the branch off it."
            ));
        }
    }

    // Distinct, and only the ones that disagree with the workspace: a
    // conversation that never left its branch has nothing to warn about, and
    // one that moved back and forth should say each name once.
    let now = crate::sweep::branch(workspace);
    let mut elsewhere: Vec<&str> = Vec::new();
    for entry in undoing {
        if let Some(branch) = entry.branch.as_deref() {
            if Some(branch) != now.as_deref() && !elsewhere.contains(&branch) {
                elsewhere.push(branch);
            }
        }
    }
    if !elsewhere.is_empty() {
        let ran_on = joined(&elsewhere);
        warnings.push(match now.as_deref() {
            Some(now) => format!(
                "These turns ran on {ran_on}, and the workspace is on {now}. Their \
                 pre-images came out of a tree that is no longer checked out, so a rewind \
                 writes them over {now} as it stands."
            ),
            // Detached, or the repository is gone. Naming what it is not is
            // more useful than naming what it is, which is a bare sha at best.
            None => format!(
                "These turns ran on {ran_on}, and that branch is not checked out now. \
                 Their pre-images came out of a different tree than the one a rewind \
                 would write them into."
            ),
        });
    }

    warnings
}

/// `a`, `a and b`, `a, b, and c` — for a warning that is read as a sentence.
fn joined(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [first, last] => format!("{first} and {last}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// Turns a 1-based turn number into an index, or says why it is not one.
///
/// Shared so that asking to *see* turn 9 of a five-turn session and asking to
/// *rewind* to it fail with the same sentence. Two spellings of one refusal is
/// how a user concludes the two features disagree about what exists.
fn check_turn(session_id: &str, turn: u32, len: usize) -> Result<usize, String> {
    if turn == 0 || turn as usize > len {
        return Err(format!(
            "session '{session_id}' has {len} checkpointed turn{}; {turn} is not one of them",
            if len == 1 { "" } else { "s" }
        ));
    }
    Ok(turn as usize - 1)
}

/// What a file holds right now, for the after-side of the last turn to touch it.
///
/// Deliberately not [`read_state`], whose `Opaque` reasons are written in the
/// past tense because it always describes a capture. Here the read is happening
/// now, and a reason that says "when it was recorded" would point the user at
/// the wrong moment to go looking.
fn state_now(workspace: &Path, file: &str) -> State {
    let path = match crate::path_guard::resolve(workspace, file) {
        Ok(path) => path,
        Err(e) => {
            return State::Opaque {
                reason: e.to_string(),
            }
        }
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => State::Text { content },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::Absent,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => State::Opaque {
            reason: "is not text, so there is nothing to diff against".into(),
        },
        Err(e) => State::Opaque {
            reason: format!("cannot be read now ({e})"),
        },
    }
}

/// Pairs two states into something the drawer can draw.
fn describe(file: &str, before: &State, after: &State) -> TurnChange {
    /// `Ok(None)` for a file that was not there, which is a side of the diff
    /// rather than a failure to produce one.
    fn text(state: &State) -> Result<Option<&str>, String> {
        match state {
            State::Absent => Ok(None),
            State::Text { content } => Ok(Some(content.as_str())),
            State::Opaque { reason } => Err(reason.clone()),
        }
    }

    match (text(before), text(after)) {
        (Ok(before), Ok(after)) => TurnChange::Diff {
            diff: crate::diff::of_change(file.to_string(), before, after),
        },
        // Either side missing costs the whole diff — half of one would show a
        // file being created out of nothing or emptied to nothing, neither of
        // which happened.
        (Err(reason), _) | (Ok(_), Err(reason)) => TurnChange::Opaque {
            path: file.to_string(),
            reason,
        },
    }
}

/// Applies one pre-image, re-validating the path against the workspace.
///
/// The log is a file on disk like any other, and a rewind writes wherever it
/// says. Resolving through the same guard the tools use means a hand-edited or
/// corrupted log cannot reach outside the workspace it belongs to.
fn restore(workspace: &Path, file: &str, state: &State, dry_run: bool) -> Restored {
    let path = match crate::path_guard::resolve(workspace, file) {
        Ok(path) => path,
        Err(e) => {
            return Restored::Skipped {
                path: file.to_string(),
                reason: e.to_string(),
            }
        }
    };

    let outcome = match state {
        // The reason is already a complete phrase; the file it belongs to is
        // carried alongside it, so wrapping it here would say the name twice.
        State::Opaque { reason } => {
            return Restored::Skipped {
                path: file.to_string(),
                reason: reason.clone(),
            }
        }
        State::Absent => {
            if dry_run || !path.exists() {
                Ok(())
            } else {
                std::fs::remove_file(&path)
            }
        }
        State::Text { content } => {
            if dry_run {
                Ok(())
            } else {
                // A turn can have created the directory too, so a plain write
                // to a path whose parent is gone would fail.
                match path.parent() {
                    Some(parent) => std::fs::create_dir_all(parent)
                        .and_then(|()| std::fs::write(&path, content)),
                    None => std::fs::write(&path, content),
                }
            }
        }
    };

    match outcome {
        Err(e) => Restored::Skipped {
            path: file.to_string(),
            reason: e.to_string(),
        },
        Ok(()) => match state {
            State::Absent => Restored::Deleted {
                path: file.to_string(),
            },
            _ => Restored::Reverted {
                path: file.to_string(),
            },
        },
    }
}

/// One turn as read back off disk.
struct ReadTurn {
    prompt: String,
    at: u64,
    changes: Vec<(String, State)>,
    branch: Option<String>,
    moved_git: bool,
}

/// A whole log as read back off disk.
///
/// Commits sit beside the turns rather than inside them because that is how
/// they are written: a commit record lands at the end of the log naming a turn
/// that closed long before it. Resolving it into the turn happens here, once,
/// so nothing downstream has to know that.
struct ReadLog {
    turns: Vec<ReadTurn>,
    commits: Vec<(u32, String)>,
}

impl ReadLog {
    /// The commit a turn was kept as.
    ///
    /// Last writer wins: a turn committed, rewound, and committed again has two
    /// records, and the live one is the one written most recently.
    fn commit_for(&self, turn: u32) -> Option<String> {
        self.commits
            .iter()
            .rev()
            .find(|(number, _)| *number == turn)
            .map(|(_, sha)| sha.clone())
    }
}

/// Rebuilds a log's turns, assigning numbers from their order.
///
/// A trailing line that will not parse is dropped rather than failing the
/// read: it is the turn that was in flight when the process died, and refusing
/// the file would lose every earlier checkpoint over it.
fn read_log(path: &Path) -> Result<ReadLog, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        // No log is not an error: it is a session that never changed a file.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ReadLog {
                turns: Vec::new(),
                commits: Vec::new(),
            })
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };

    let mut turns: Vec<ReadTurn> = Vec::new();
    let mut commits: Vec<(u32, String)> = Vec::new();
    let mut header_seen = false;

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Record>(&line) {
            Ok(Record::Header(header)) => {
                if header.version > FORMAT_VERSION {
                    return Err(format!(
                        "{} was written by a newer version of Taurus (format {} > \
                         {FORMAT_VERSION}); refusing to restore from it",
                        path.display(),
                        header.version
                    ));
                }
                header_seen = true;
            }
            Ok(Record::Turn { prompt, at, branch }) => turns.push(ReadTurn {
                prompt,
                at,
                changes: Vec::new(),
                branch,
                moved_git: false,
            }),
            Ok(Record::Before { path, state }) => {
                // A `before` with no open turn is a torn log; there is nothing
                // to attach it to, and inventing a turn for it would produce a
                // checkpoint the user never made.
                if let Some(turn) = turns.last_mut() {
                    turn.changes.push((path, state));
                }
            }
            // Same rule, same reason: it belongs to the turn it follows, and a
            // torn log with no turn to follow has nothing to say it about.
            Ok(Record::MovedGit) => {
                if let Some(turn) = turns.last_mut() {
                    turn.moved_git = true;
                }
            }
            // Position-independent, so it is collected wherever it lands and
            // matched to its turn by number afterwards.
            Ok(Record::Committed { turn, sha }) => commits.push((turn, sha)),
            Err(e) => {
                tracing::debug!(error = %e, "skipping an unreadable checkpoint line");
            }
        }
    }

    // A missing header is a damaged log, not a suspicious one, so it costs a
    // warning rather than the file. The version guard above only ever applied
    // to logs that *have* a header — a newer Taurus writes one too — so
    // refusing a header-less file protects nothing, while every turn in it is
    // sitting right there, already parsed as the current format.
    if !header_seen && !turns.is_empty() {
        tracing::warn!(
            path = %path.display(),
            "checkpoint log has no header; reading it as the current format"
        );
    }
    Ok(ReadLog { turns, commits })
}

#[derive(Default)]
struct TurnState {
    /// Paths already captured this turn. The first capture is the pre-image;
    /// later ones would record what an earlier call in the same turn wrote.
    seen: HashSet<PathBuf>,
    opened: bool,
    /// Set after a write fails, so one broken log does not narrate every call.
    disabled: bool,
    /// Whether this turn has already said it moved git. Several commands in
    /// one turn can each move it, and the warning reads the same once as
    /// three times.
    moved_git: bool,
}

/// The open turn that tools record into.
///
/// Shared by clone through [`crate::ToolContext`], so a sub-agent records into
/// the turn that spawned it rather than one of its own.
pub struct TurnRecorder {
    /// `None` for a session id that cannot be a filename.
    path: Option<PathBuf>,
    session: String,
    workspace: PathBuf,
    prompt: String,
    /// The branch the turn began on, written into its `Turn` record.
    branch: Option<String>,
    state: Mutex<TurnState>,
}

impl TurnRecorder {
    /// How many distinct files this turn has recorded a pre-image for.
    ///
    /// The authority on "did this turn change anything", because it counts what
    /// was actually captured rather than what a tool was asked to do — a denied
    /// call, a failed write, and a command that touched nothing all leave it
    /// where it was.
    pub async fn changed_count(&self) -> usize {
        self.state.lock().await.seen.len()
    }

    /// The same set, as the workspace-relative strings a listing shows.
    ///
    /// Sorted rather than in capture order: this is read repeatedly during a
    /// turn to redraw a list, and ordering it by when each file happened to be
    /// touched would reshuffle the rows under whoever is reading them.
    ///
    /// Rendered through [`crate::path_guard::display`], the same call
    /// [`Self::record`] writes the log with, so a path reported here and the
    /// one a rewind later names are the same string.
    pub async fn changed_paths(&self) -> Vec<String> {
        let state = self.state.lock().await;
        let mut paths: Vec<String> = state
            .seen
            .iter()
            .map(|path| crate::path_guard::display(&self.workspace, path))
            .collect();
        paths.sort();
        paths
    }

    /// Records that this turn moved `HEAD` or the ref it names.
    ///
    /// Only ever called by [`crate::sweep`], and only when the same pass also
    /// recorded a file — a `git commit` that left the working tree alone gives
    /// a rewind nothing to be wrong about. That ordering is what lets this skip
    /// a turn that is not open: with no pre-image recorded there is nothing to
    /// undo, and a record written here would attach itself to whichever turn
    /// last opened the log.
    pub async fn moved_git(&self) {
        let Some(log) = self.path.clone() else {
            return;
        };
        let mut state = self.state.lock().await;
        if state.disabled || !state.opened || state.moved_git {
            return;
        }
        state.moved_git = true;
        if !append(&log, &Record::MovedGit) {
            state.disabled = true;
        }
    }

    /// Records what `path` holds right now, the first time this turn asks.
    ///
    /// For a tool that named the file before touching it, which is the only
    /// moment its previous contents are still on disk to be read.
    pub async fn capture(&self, path: &Path) {
        self.record(path, None).await;
    }

    /// Records a pre-image the caller is already holding.
    ///
    /// For [`crate::sweep`], which learns that a file changed only after the
    /// command that changed it has finished — by which time reading the path
    /// would capture the new contents as though they were the old ones. The
    /// bytes it passes here were read before the command ran.
    pub async fn capture_state(&self, path: &Path, before: State) {
        self.record(path, Some(before)).await;
    }

    /// The one path into the log.
    ///
    /// Failing to write a checkpoint never fails the tool call. Persistence is
    /// a side effect of work the user asked for, and a full disk has to cost
    /// them the undo, not the edit — the same bargain the session transcript
    /// makes. It is logged once and then goes quiet.
    async fn record(&self, path: &Path, held: Option<State>) {
        let Some(log) = self.path.clone() else {
            return;
        };

        let mut state = self.state.lock().await;
        if state.disabled || !state.seen.insert(path.to_path_buf()) {
            return;
        }

        let relative = crate::path_guard::display(&self.workspace, path);
        // First capture wins, and a held pre-image is by definition older than
        // anything a read here could produce, so it is preferred when present.
        let before = held.unwrap_or_else(|| read_state(path));

        // The turn's header goes down with its first file, so a log holds only
        // turns that changed something.
        if !state.opened {
            // Asks the file what header it has rather than whether it exists.
            // `append` creates the file before it writes, so a write that fails
            // leaves an empty one behind — and "it exists" would then be true
            // forever, so every later turn skipped the header and left a log
            // that `read_log` could not accept. Checking for the record itself
            // also repairs a log already in that state, on its next turn.
            if !ensure_header(&log, &self.session, &self.workspace) {
                state.disabled = true;
                return;
            }
            if !append(
                &log,
                &Record::Turn {
                    prompt: self.prompt.clone(),
                    at: now(),
                    branch: self.branch.clone(),
                },
            ) {
                state.disabled = true;
                return;
            }
            state.opened = true;
        }

        if !append(
            &log,
            &Record::Before {
                path: relative,
                state: before,
            },
        ) {
            state.disabled = true;
        }
    }
}

/// The newest format a log's headers claim, or `None` for a log with none.
///
/// Scans the whole file rather than checking the first line, because a log
/// repaired after a failed first write carries its header after the turns it
/// was missing from — and because a log upgraded in place carries two. A log
/// is one short line per changed file, and the common case stops on line one.
fn header_version(path: &Path) -> Option<u32> {
    let file = std::fs::File::open(path).ok()?;
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| match serde_json::from_str::<Record>(&line) {
            Ok(Record::Header(header)) => Some(header.version),
            _ => None,
        })
        .max()
}

/// Gives a log a header at the current format if it does not have one.
///
/// Covers two cases with one check. A log that has never been written needs its
/// first header; a log written by an older Taurus needs a second one, because
/// it is about to gain a record that older build would skip rather than
/// understand, and [`read_log`]'s version guard is the only thing that can stop
/// it. Appending rather than rewriting keeps the file append-only, and
/// [`header_version`] takes the newest of what it finds.
///
/// Returns whether the log is safe to append to.
fn ensure_header(path: &Path, session: &str, workspace: &Path) -> bool {
    if header_version(path) == Some(FORMAT_VERSION) {
        return true;
    }
    append(
        path,
        &Record::Header(Header {
            version: FORMAT_VERSION,
            session: session.to_string(),
            workspace: workspace.display().to_string(),
        }),
    )
}

/// Appends one record. Returns whether it landed.
fn append(path: &Path, record: &Record) -> bool {
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let line = serde_json::to_string(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        restrict(&file);
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")
    };

    match write() {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "could not write a checkpoint; this turn will not be rewindable"
            );
            false
        }
    }
}

/// Narrows a log to its owner, if it is readable by anyone else.
///
/// These files hold the contents of whatever a turn changed, and since
/// [`crate::sweep`] began covering files an ignore rule excludes by name, that
/// includes `.env`. A file kept out of version control on purpose must not
/// become world-readable by being made recoverable.
///
/// Applied on every append rather than only at creation, so a log written
/// before this existed is narrowed the next time it is touched. The check is
/// what keeps that to one `chmod` per file rather than one per record.
#[cfg(unix)]
fn restrict(file: &std::fs::File) {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = file.metadata() else {
        return;
    };
    let mut permissions = metadata.permissions();
    if permissions.mode() & 0o077 == 0 {
        return;
    }
    permissions.set_mode(0o600);
    // Best-effort: a log that could not be narrowed is still a log worth
    // writing, and failing the append would cost the undo as well.
    let _ = file.set_permissions(permissions);
}

/// Windows has no comparable mode to set; the file inherits the directory's
/// ACL, and `~/.taurus` is under the user's own profile.
#[cfg(not(unix))]
fn restrict(_file: &std::fs::File) {}

fn shorten(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > LABEL_MAX_CHARS {
        format!(
            "{}…",
            line.chars().take(LABEL_MAX_CHARS - 1).collect::<String>()
        )
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A store and a workspace, both temporary.
    ///
    /// The root is canonicalized because that is what a real capture receives:
    /// tools resolve through [`crate::path_guard`] first. On macOS the temp
    /// directory lives behind `/var -> /private/var`, so skipping this would
    /// record absolute paths and quietly stop testing the relative ones.
    struct Fixture {
        store: CheckpointStore,
        root: PathBuf,
        logs: TempDir,
        _workspace: TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let logs = TempDir::new().unwrap();
            let workspace = TempDir::new().unwrap();
            Self {
                store: CheckpointStore::new(logs.path()),
                root: workspace.path().canonicalize().unwrap(),
                logs,
                _workspace: workspace,
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }

        fn log(&self, session: &str) -> PathBuf {
            self.logs.path().join(format!("{session}.jsonl"))
        }
    }

    impl Fixture {
        /// A repository, as far as anything here reads one: `HEAD` naming a
        /// branch. The same shape the sweep's fixture builds, and for the same
        /// reason — nothing in this crate shells out to git, so a branch is a
        /// line in a file.
        fn on_branch(&self, name: &str) {
            let head = self.root.join(".git/HEAD");
            std::fs::create_dir_all(head.parent().unwrap()).unwrap();
            std::fs::write(head, format!("ref: refs/heads/{name}\n")).unwrap();
        }

        /// A turn that changes one file, which is enough to be rewindable.
        async fn turn(&self, prompt: &str, content: &str) -> Arc<TurnRecorder> {
            let file = self.path("a.txt");
            let recorder = self.store.begin_turn("s1", &self.root, prompt);
            recorder.capture(&file).await;
            std::fs::write(&file, content).unwrap();
            recorder
        }

        fn rewind_plan(&self, turn: u32) -> Rewind {
            self.store.rewind("s1", &self.root, turn, true).unwrap()
        }
    }

    #[tokio::test]
    async fn a_turn_that_only_changed_files_warns_about_nothing() {
        // The common case, and the one that decides whether the warnings are
        // worth having: a list that fires on every rewind is one nobody reads.
        let f = Fixture::new();
        f.turn("edit it", "after\n").await;

        assert!(f.rewind_plan(1).warnings.is_empty());
    }

    #[tokio::test]
    async fn a_turn_that_moved_git_says_so_at_the_moment_of_the_rewind() {
        // The sweep already says this when the command runs. The person who
        // needs it is reading a rewind plan, which can be days later.
        let f = Fixture::new();
        let recorder = f.turn("check out another branch", "after\n").await;
        recorder.moved_git().await;

        let warnings = f.rewind_plan(1).warnings;
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("Turn 1"), "{warnings:?}");
        assert!(warnings[0].contains("git reflog"), "{warnings:?}");
    }

    #[tokio::test]
    async fn a_turn_that_moved_git_twice_says_it_once() {
        // Several commands in one turn can each move it. The warning reads the
        // same once as three times, and three copies would bury the others.
        let f = Fixture::new();
        let recorder = f.turn("move it about", "after\n").await;
        recorder.moved_git().await;
        recorder.moved_git().await;

        assert_eq!(f.rewind_plan(1).warnings.len(), 1);
    }

    #[tokio::test]
    async fn a_turn_that_recorded_nothing_cannot_claim_to_have_moved_git() {
        // A `git commit` that left the working tree alone gives a rewind
        // nothing to be wrong about. Written down anyway, the record would
        // attach itself to whichever turn last opened the log — turn 1's
        // warning appearing on turn 2's plan.
        let f = Fixture::new();
        f.turn("edit it", "after\n").await;

        let empty = f.store.begin_turn("s1", &f.root, "git commit");
        empty.moved_git().await;

        assert!(f.rewind_plan(1).warnings.is_empty());
        assert_eq!(f.store.turns("s1").unwrap().len(), 1, "no turn was opened");
    }

    #[tokio::test]
    async fn a_committed_turn_says_the_commit_will_be_left_behind() {
        // The two features did not know about each other: rewinding past a
        // committed turn restored the files and left the commit pointing at a
        // tree that no longer exists.
        let f = Fixture::new();
        f.turn("get it right", "after\n").await;
        f.store.record_commit("s1", &f.root, 1, "abc1234");

        let warnings = f.rewind_plan(1).warnings;
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("abc1234"), "{warnings:?}");
        assert!(warnings[0].contains("git revert"), "{warnings:?}");
    }

    #[tokio::test]
    async fn only_the_turns_being_undone_are_warned_about() {
        // Rewinding to turn 2 undoes 2 and 3. Turn 1's commit stays exactly
        // where it is, so naming it would be describing something that is not
        // happening.
        let f = Fixture::new();
        f.turn("one", "one\n").await;
        f.turn("two", "two\n").await;
        f.turn("three", "three\n").await;
        f.store.record_commit("s1", &f.root, 1, "aaaaaaa");
        f.store.record_commit("s1", &f.root, 3, "ccccccc");

        let warnings = f.rewind_plan(2).warnings;
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("ccccccc"), "{warnings:?}");
    }

    #[tokio::test]
    async fn a_turn_carries_the_commit_it_was_kept_as_into_the_listing() {
        // What lets the drawer say which turns are already in HEAD, and so
        // which ones a commit made now would sit on top of.
        let f = Fixture::new();
        f.turn("one", "one\n").await;
        f.turn("two", "two\n").await;
        f.store.record_commit("s1", &f.root, 1, "aaaaaaa");

        let turns = f.store.turns("s1").unwrap();
        assert_eq!(turns[0].commit.as_deref(), Some("aaaaaaa"));
        assert_eq!(turns[1].commit, None, "turn 2 is not in HEAD");
    }

    #[tokio::test]
    async fn a_turn_committed_twice_reports_the_commit_it_is_actually_in() {
        // Committed, rewound, committed again. The log keeps both records
        // because it is append-only; the live one is the newer.
        let f = Fixture::new();
        f.turn("one", "one\n").await;
        f.store.record_commit("s1", &f.root, 1, "aaaaaaa");
        f.store.record_commit("s1", &f.root, 1, "bbbbbbb");

        assert_eq!(
            f.store.turns("s1").unwrap()[0].commit.as_deref(),
            Some("bbbbbbb")
        );
    }

    #[tokio::test]
    async fn a_rewind_onto_a_branch_the_turn_did_not_run_on_says_so() {
        // The gap this closes: the branch was recorded in the transcript header
        // and nothing acted on it, so a conversation started on feat/x and
        // resumed on main restored feat/x's files over main without a word.
        let f = Fixture::new();
        f.on_branch("feat-x");
        f.turn("work on the feature", "after\n").await;
        f.on_branch("main");

        let warnings = f.rewind_plan(1).warnings;
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("feat-x"), "{warnings:?}");
        assert!(warnings[0].contains("main"), "{warnings:?}");
    }

    #[tokio::test]
    async fn a_conversation_that_stayed_on_its_branch_says_nothing_about_it() {
        let f = Fixture::new();
        f.on_branch("main");
        f.turn("work", "after\n").await;

        assert!(f.rewind_plan(1).warnings.is_empty());
        assert_eq!(
            f.store.turns("s1").unwrap()[0].branch.as_deref(),
            Some("main")
        );
    }

    #[tokio::test]
    async fn a_turn_records_the_branch_it_began_on_not_the_one_it_moved_to() {
        // A turn that checks out another branch and then edits a file is
        // exactly the case the warning exists for, and recording the branch
        // lazily — when the first file lands — would record the destination.
        let f = Fixture::new();
        f.on_branch("main");
        let file = f.path("a.txt");
        let recorder = f.store.begin_turn("s1", &f.root, "switch and edit");
        f.on_branch("elsewhere");
        recorder.capture(&file).await;
        std::fs::write(&file, "after\n").unwrap();

        assert_eq!(
            f.store.turns("s1").unwrap()[0].branch.as_deref(),
            Some("main")
        );
    }

    #[tokio::test]
    async fn a_detached_head_is_no_branch_rather_than_a_branch_called_head() {
        let f = Fixture::new();
        std::fs::create_dir_all(f.path(".git")).unwrap();
        std::fs::write(f.path(".git/HEAD"), "aaaaaaabbbbbbbccccccc\n").unwrap();
        f.turn("work", "after\n").await;

        assert_eq!(f.store.turns("s1").unwrap()[0].branch, None);
        assert!(f.rewind_plan(1).warnings.is_empty());
    }

    #[tokio::test]
    async fn a_format_1_log_gains_a_current_header_before_it_gains_a_format_2_record() {
        // An older build skips records it does not know, which here would mean
        // rewinding past a committed turn without a word. The version guard is
        // the only thing that can stop it, and it only fires on a header.
        let f = Fixture::new();
        std::fs::create_dir_all(f.logs.path()).unwrap();
        std::fs::write(
            f.log("s1"),
            "{\"type\":\"header\",\"version\":1,\"session\":\"s1\",\"workspace\":\"/w\"}\n\
             {\"type\":\"turn\",\"prompt\":\"an older turn\",\"at\":1}\n\
             {\"type\":\"before\",\"path\":\"old.txt\",\"state\":\"absent\"}\n",
        )
        .unwrap();

        f.turn("a newer turn", "after\n").await;

        assert_eq!(header_version(&f.log("s1")), Some(FORMAT_VERSION));
        let turns = f.store.turns("s1").unwrap();
        assert_eq!(turns.len(), 2, "the format-1 turn is still readable");
        assert_eq!(turns[0].prompt, "an older turn");
        assert_eq!(turns[0].branch, None, "format 1 recorded no branch");
    }

    #[test]
    fn a_list_of_branches_reads_as_a_sentence() {
        assert_eq!(joined(&["a"]), "a");
        assert_eq!(joined(&["a", "b"]), "a and b");
        assert_eq!(joined(&["a", "b", "c"]), "a, b, and c");
    }

    /// The diff for one file of one turn, or a panic naming what came back
    /// instead. Every `changes` test wants this and nothing else.
    fn only_diff(changes: Vec<TurnChange>) -> FileDiff {
        match changes.as_slice() {
            [TurnChange::Diff { diff }] => diff.clone(),
            other => panic!("expected one diff, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_turns_diff_ends_where_the_next_turn_to_touch_the_file_begins() {
        // The point of the whole design: no post-images are written, because
        // the next turn's pre-image already is one. A turn in the middle of a
        // session must show what *it* did, not everything since.
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "one\n").unwrap();

        let first = f.store.begin_turn("s1", &f.root, "make it two");
        first.capture(&file).await;
        std::fs::write(&file, "two\n").unwrap();

        let second = f.store.begin_turn("s1", &f.root, "make it three");
        second.capture(&file).await;
        std::fs::write(&file, "three\n").unwrap();

        let diff = only_diff(f.store.changes("s1", &f.root, 1).unwrap());
        let lines: Vec<_> = diff.hunks.iter().flat_map(|h| h.lines.iter()).collect();
        assert!(
            lines.iter().any(|l| l.text == "two"),
            "turn 1 ended at 'two', not at what is on disk now: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.text == "three"),
            "turn 2's work leaked into turn 1: {lines:?}"
        );
    }

    #[tokio::test]
    async fn the_last_turn_to_touch_a_file_is_diffed_against_the_disk() {
        // Nothing recorded an after-image for it, and there is no need: what it
        // left behind is still sitting there.
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "before\n").unwrap();

        let recorder = f.store.begin_turn("s1", &f.root, "rewrite it");
        recorder.capture(&file).await;
        std::fs::write(&file, "after\n").unwrap();

        let diff = only_diff(f.store.changes("s1", &f.root, 1).unwrap());
        assert_eq!((diff.added, diff.removed), (1, 1));
        assert!(!diff.created && !diff.deleted);
        assert_eq!(diff.path, "a.txt");
    }

    #[tokio::test]
    async fn a_file_the_turn_created_reads_as_a_creation() {
        let f = Fixture::new();
        let file = f.path("new.txt");

        let recorder = f.store.begin_turn("s1", &f.root, "add a file");
        // Captured before it exists, which is how `write_file` records one.
        recorder.capture(&file).await;
        std::fs::write(&file, "hello\n").unwrap();

        let diff = only_diff(f.store.changes("s1", &f.root, 1).unwrap());
        assert!(diff.created, "{diff:?}");
        assert_eq!((diff.added, diff.removed), (1, 0));
    }

    #[tokio::test]
    async fn a_file_the_turn_deleted_reads_as_a_deletion() {
        // A `run_command` that removed it. Rendering this as a file truncated
        // to nothing would describe a different act.
        let f = Fixture::new();
        let file = f.path("doomed.txt");
        std::fs::write(&file, "a\nb\n").unwrap();

        let recorder = f.store.begin_turn("s1", &f.root, "remove it");
        recorder.capture(&file).await;
        std::fs::remove_file(&file).unwrap();

        let diff = only_diff(f.store.changes("s1", &f.root, 1).unwrap());
        assert!(diff.deleted && !diff.created, "{diff:?}");
        assert_eq!((diff.added, diff.removed), (0, 2));
    }

    #[tokio::test]
    async fn a_file_with_no_pre_image_is_named_rather_than_shown() {
        // The same files a rewind reports as skipped. Leaving them out of the
        // list entirely would make the turn look smaller than it was.
        let f = Fixture::new();
        let file = f.path("blob.bin");

        let recorder = f.store.begin_turn("s1", &f.root, "write a blob");
        recorder
            .capture_state(
                &file,
                State::Opaque {
                    reason: "was too large to copy when it was recorded".into(),
                },
            )
            .await;

        match f.store.changes("s1", &f.root, 1).unwrap().as_slice() {
            [TurnChange::Opaque { path, reason }] => {
                assert_eq!(path, "blob.bin");
                assert!(reason.contains("too large"), "{reason}");
            }
            other => panic!("expected an opaque change, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn asking_for_a_turn_that_is_not_there_fails_the_way_a_rewind_does() {
        // Two spellings of one refusal is how a user concludes the two
        // features disagree about what exists.
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "x").unwrap();
        f.store
            .begin_turn("s1", &f.root, "touch it")
            .capture(&file)
            .await;

        let shown = f.store.changes("s1", &f.root, 9).unwrap_err();
        let rewound = f.store.rewind("s1", &f.root, 9, true).unwrap_err();
        assert_eq!(shown, rewound);
        assert!(shown.contains("has 1 checkpointed turn;"), "{shown}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_log_is_readable_by_its_owner_and_nobody_else() {
        // These hold the contents of whatever a turn changed, and since
        // `sweep` began covering files an ignore rule excludes by name, that
        // includes `.env`. Default permissions would publish it to every
        // account on the machine.
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "before").unwrap();

        let recorder = f.store.begin_turn("s1", &f.root, "change a.txt");
        recorder.capture(&file).await;

        let mode = std::fs::metadata(f.log("s1")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "log mode was {:o}", mode & 0o777);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_log_written_before_this_existed_is_narrowed_when_it_is_next_used() {
        // The mode is set on append rather than on creation, so an existing
        // log does not stay wide open for the rest of its life.
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let log = f.log("s1");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "").unwrap();
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();

        let file = f.path("a.txt");
        std::fs::write(&file, "before").unwrap();
        let recorder = f.store.begin_turn("s1", &f.root, "change a.txt");
        recorder.capture(&file).await;

        let mode = std::fs::metadata(&log).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "log mode was {:o}", mode & 0o777);
    }

    #[tokio::test]
    async fn a_turn_that_changed_nothing_is_not_listed() {
        let f = Fixture::new();
        f.store.begin_turn("s1", &f.root, "just have a look around");
        assert!(f.store.turns("s1").unwrap().is_empty());
    }

    #[tokio::test]
    async fn forgetting_a_session_takes_its_log_and_leaves_the_others() {
        let f = Fixture::new();
        for session in ["s1", "s2"] {
            let file = f.path(&format!("{session}.txt"));
            let recorder = f.store.begin_turn(session, &f.root, "write a file");
            recorder.capture(&file).await;
            std::fs::write(&file, "content").unwrap();
        }

        f.store.forget("s1").unwrap();

        assert!(!f.log("s1").exists(), "the log outlived its conversation");
        assert!(f.store.turns("s1").unwrap().is_empty());
        assert_eq!(f.store.turns("s2").unwrap().len(), 1, "the wrong log went");
    }

    #[tokio::test]
    async fn forgetting_a_session_that_changed_nothing_is_not_a_failure() {
        // A read-only conversation never writes a log, and that is the ordinary
        // case rather than a sign that something went wrong.
        let f = Fixture::new();
        f.store.forget("never-wrote-anything").unwrap();
    }

    #[tokio::test]
    async fn forgetting_will_not_take_an_id_that_escapes_the_checkpoint_tree() {
        let f = Fixture::new();
        for id in ["../secrets", "..", "", "a/b"] {
            assert!(f.store.forget(id).is_err(), "'{id}' was accepted");
        }
    }

    #[tokio::test]
    async fn an_edited_file_goes_back_to_what_it_was() {
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "original").unwrap();

        let recorder = f.store.begin_turn("s1", &f.root, "change a.txt");
        recorder.capture(&file).await;
        std::fs::write(&file, "the model's version").unwrap();

        let restored = f.store.rewind("s1", &f.root, 1, false).unwrap().restored;
        assert!(matches!(restored[0], Restored::Reverted { .. }));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
    }

    #[tokio::test]
    async fn a_file_the_turn_created_is_deleted_again() {
        let f = Fixture::new();
        let file = f.path("new.txt");

        let recorder = f.store.begin_turn("s1", &f.root, "add new.txt");
        recorder.capture(&file).await;
        std::fs::write(&file, "brand new").unwrap();

        let restored = f.store.rewind("s1", &f.root, 1, false).unwrap().restored;
        assert!(matches!(restored[0], Restored::Deleted { .. }));
        assert!(!file.exists(), "a created file must not survive the rewind");
    }

    #[tokio::test]
    async fn only_the_first_capture_in_a_turn_is_the_pre_image() {
        // Two edits to one file in a single turn: undoing has to reach past
        // the intermediate state to what the user last saw.
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "original").unwrap();

        let recorder = f.store.begin_turn("s1", &f.root, "edit twice");
        recorder.capture(&file).await;
        std::fs::write(&file, "halfway").unwrap();
        recorder.capture(&file).await;
        std::fs::write(&file, "finished").unwrap();

        f.store.rewind("s1", &f.root, 1, false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
    }

    #[tokio::test]
    async fn rewinding_to_a_turn_undoes_every_turn_after_it_too() {
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "v1").unwrap();

        for (turn, content) in [("first change", "v2"), ("second change", "v3")] {
            let recorder = f.store.begin_turn("s1", &f.root, turn);
            recorder.capture(&file).await;
            std::fs::write(&file, content).unwrap();
        }

        assert_eq!(f.store.turns("s1").unwrap().len(), 2);
        f.store.rewind("s1", &f.root, 1, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "v1",
            "the oldest pre-image is the one that predates both turns"
        );
    }

    #[tokio::test]
    async fn rewinding_to_the_later_of_two_turns_keeps_the_earlier_one() {
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "v1").unwrap();

        for (turn, content) in [("first change", "v2"), ("second change", "v3")] {
            let recorder = f.store.begin_turn("s1", &f.root, turn);
            recorder.capture(&file).await;
            std::fs::write(&file, content).unwrap();
        }

        f.store.rewind("s1", &f.root, 2, false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");
    }

    #[tokio::test]
    async fn a_dry_run_reports_without_touching_anything() {
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "original").unwrap();

        let recorder = f.store.begin_turn("s1", &f.root, "change it");
        recorder.capture(&file).await;
        std::fs::write(&file, "changed").unwrap();

        let planned = f.store.rewind("s1", &f.root, 1, true).unwrap().restored;
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].path(), "a.txt");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "changed",
            "a dry run must not write"
        );
    }

    #[tokio::test]
    async fn a_listing_names_the_turn_and_the_files_it_changed() {
        let f = Fixture::new();
        let recorder = f
            .store
            .begin_turn("s1", &f.root, "rename the widget\nand tidy up");
        recorder.capture(&f.path("a.txt")).await;
        recorder.capture(&f.path("b.txt")).await;

        let turns = f.store.turns("s1").unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn, 1);
        assert_eq!(turns[0].prompt, "rename the widget");
        assert_eq!(turns[0].files, vec!["a.txt", "b.txt"]);
        assert!(turns[0].at > 0);
    }

    #[tokio::test]
    async fn a_sub_agents_writes_land_in_the_turn_that_spawned_it() {
        // The recorder is shared by clone, so the parent's open turn is the one
        // a delegated write records into.
        let f = Fixture::new();
        let parent_file = f.path("a.txt");
        let child_file = f.path("b.txt");
        std::fs::write(&parent_file, "a").unwrap();
        std::fs::write(&child_file, "b").unwrap();

        let recorder = f.store.begin_turn("s1", &f.root, "delegate some work");
        recorder.capture(&parent_file).await;
        let child = recorder.clone();
        child.capture(&child_file).await;

        let turns = f.store.turns("s1").unwrap();
        assert_eq!(turns.len(), 1, "the child must not open a turn of its own");
        assert_eq!(turns[0].files, vec!["a.txt", "b.txt"]);
    }

    #[tokio::test]
    async fn a_file_that_is_not_text_is_reported_rather_than_silently_lost() {
        let f = Fixture::new();
        let file = f.path("blob.bin");
        std::fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();

        let recorder = f.store.begin_turn("s1", &f.root, "overwrite a binary");
        recorder.capture(&file).await;
        std::fs::write(&file, "clobbered").unwrap();

        let restored = f.store.rewind("s1", &f.root, 1, false).unwrap().restored;
        assert!(matches!(restored[0], Restored::Skipped { .. }));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "clobbered",
            "there was nothing to put back, and pretending otherwise would be worse"
        );
    }

    #[tokio::test]
    async fn a_turn_number_nobody_recorded_is_refused() {
        let f = Fixture::new();
        let recorder = f.store.begin_turn("s1", &f.root, "one turn");
        recorder.capture(&f.path("a.txt")).await;

        for bad in [0, 2, 99] {
            let err = f.store.rewind("s1", &f.root, bad, false).unwrap_err();
            assert!(err.contains("checkpointed turn"), "{err}");
        }
    }

    #[tokio::test]
    async fn a_torn_final_line_costs_the_last_capture_and_nothing_else() {
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "original").unwrap();
        let recorder = f.store.begin_turn("s1", &f.root, "change it");
        recorder.capture(&file).await;

        let log = f.log("s1");
        let mut handle = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        handle
            .write_all(br#"{"type":"before","path":"b.txt","sta"#)
            .unwrap();
        drop(handle);

        let turns = f
            .store
            .turns("s1")
            .expect("a torn tail must not fail the read");
        assert_eq!(turns[0].files, vec!["a.txt"]);
    }

    #[tokio::test]
    async fn an_empty_log_left_by_a_failed_write_still_gets_its_header() {
        // `append` creates the file before it writes, so a write that fails
        // leaves a zero-byte log. Keying the header off "does the file exist"
        // meant every later turn skipped it, and the session's checkpoints
        // became permanently unreadable while the pre-images sat on disk.
        let f = Fixture::new();
        std::fs::create_dir_all(f.log("s1").parent().unwrap()).unwrap();
        std::fs::write(f.log("s1"), "").unwrap();

        let file = f.path("a.txt");
        std::fs::write(&file, "original").unwrap();
        let recorder = f.store.begin_turn("s1", &f.root, "change a.txt");
        recorder.capture(&file).await;
        std::fs::write(&file, "changed").unwrap();

        let turns = f
            .store
            .turns("s1")
            .expect("a log that started empty must still be readable");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].files, vec!["a.txt"]);
    }

    #[tokio::test]
    async fn a_log_that_lost_its_header_is_repaired_by_the_next_turn() {
        let f = Fixture::new();
        let file = f.path("a.txt");
        std::fs::write(&file, "original").unwrap();

        // A turn's worth of records with no header, which is what the old
        // condition produced once a zero-byte log existed.
        std::fs::create_dir_all(f.log("s1").parent().unwrap()).unwrap();
        let headerless = [
            Record::Turn {
                branch: None,
                prompt: "earlier turn".into(),
                at: 1,
            },
            Record::Before {
                path: "a.txt".into(),
                state: State::Text {
                    content: "original".into(),
                },
            },
        ]
        .iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
        std::fs::write(f.log("s1"), format!("{headerless}\n")).unwrap();

        let recorder = f.store.begin_turn("s1", &f.root, "a later turn");
        recorder.capture(&file).await;

        assert_eq!(
            header_version(&f.log("s1")),
            Some(FORMAT_VERSION),
            "the next turn must repair it"
        );
        // And repairing it once is enough — a second turn must not stack
        // another header on top.
        let before = std::fs::read_to_string(f.log("s1")).unwrap();
        let again = f.store.begin_turn("s1", &f.root, "a third turn");
        std::fs::write(&file, "changed twice").unwrap();
        again.capture(&file).await;
        let after = std::fs::read_to_string(f.log("s1")).unwrap();
        assert_eq!(
            before.matches("\"header\"").count(),
            after.matches("\"header\"").count(),
        );
    }

    #[tokio::test]
    async fn a_header_less_log_is_read_rather_than_refused() {
        // Every record in it parsed as the current format, and a newer Taurus
        // would have written a header of its own — so refusing this file
        // guards nothing and loses turns that are sitting right there.
        let f = Fixture::new();
        std::fs::create_dir_all(f.log("s1").parent().unwrap()).unwrap();
        let records = [
            Record::Turn {
                branch: None,
                prompt: "a turn".into(),
                at: 1,
            },
            Record::Before {
                path: "a.txt".into(),
                state: State::Text {
                    content: "original".into(),
                },
            },
        ]
        .iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
        std::fs::write(f.log("s1"), format!("{records}\n")).unwrap();

        let turns = f
            .store
            .turns("s1")
            .expect("a header-less log must still be readable");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].files, vec!["a.txt"]);

        // And it can actually be rewound, which is the point of reading it.
        let file = f.path("a.txt");
        std::fs::write(&file, "the model's version").unwrap();
        f.store.rewind("s1", &f.root, 1, false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
    }

    #[tokio::test]
    async fn a_log_from_a_newer_taurus_is_refused_rather_than_half_applied() {
        let f = Fixture::new();
        std::fs::write(
            f.log("s1"),
            "{\"type\":\"header\",\"version\":99,\"session\":\"s1\",\"workspace\":\"/w\"}\n",
        )
        .unwrap();

        let err = f.store.rewind("s1", &f.root, 1, false).unwrap_err();
        assert!(err.contains("newer version"), "{err}");
    }

    #[tokio::test]
    async fn a_log_naming_a_path_outside_the_workspace_cannot_write_there() {
        // The log is an ordinary file. A rewind that trusted it could be
        // pointed anywhere on the machine.
        let f = Fixture::new();
        // Written through the real serializer rather than by hand, so the test
        // exercises the path guard instead of accidentally testing that
        // malformed JSON is skipped.
        let hostile = [
            Record::Header(Header {
                version: FORMAT_VERSION,
                session: "s1".into(),
                workspace: f.root.display().to_string(),
            }),
            Record::Turn {
                branch: None,
                prompt: "hostile".into(),
                at: 1,
            },
            Record::Before {
                path: "../escaped.txt".into(),
                state: State::Text {
                    content: "owned".into(),
                },
            },
        ];
        for record in &hostile {
            assert!(append(&f.log("s1"), record));
        }

        let restored = f.store.rewind("s1", &f.root, 1, false).unwrap().restored;
        assert!(matches!(restored[0], Restored::Skipped { .. }));
        assert!(!&f.root.parent().unwrap().join("escaped.txt").exists());
    }

    #[tokio::test]
    async fn a_session_id_that_would_escape_the_checkpoint_tree_records_nothing() {
        let f = Fixture::new();
        for hostile in ["../../etc/passwd", "a/b", ""] {
            let recorder = f.store.begin_turn(hostile, &f.root, "hostile");
            recorder.capture(&f.path("a.txt")).await;
            assert!(f.store.turns(hostile).is_err(), "{hostile} was accepted");
        }
    }
}
