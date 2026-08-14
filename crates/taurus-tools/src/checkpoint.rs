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
//! **A turn owns the records that follow it.** There is no turn number in the
//! file — order is the association, and numbers are assigned when the log is
//! read. That is what lets a sub-agent's writes land in the turn that spawned
//! it without anyone passing an identifier down.
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

/// Bumped when a record shape changes incompatibly. A log written by a newer
/// Taurus is refused rather than half-understood — a partial rewind is the one
/// outcome worse than no rewind.
const FORMAT_VERSION: u32 = 1;

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
    },
    Before {
        path: String,
        state: State,
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
        Ok(read_log(&path)?
            .into_iter()
            .enumerate()
            .map(|(index, turn)| Checkpoint {
                turn: index as u32 + 1,
                prompt: turn.prompt,
                at: turn.at,
                files: turn.changes.into_iter().map(|(path, _)| path).collect(),
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
    pub fn rewind(
        &self,
        session_id: &str,
        workspace: &Path,
        turn: u32,
        dry_run: bool,
    ) -> Result<Vec<Restored>, String> {
        let Some(path) = self.log_path(session_id) else {
            return Err(format!("'{session_id}' is not a usable session id"));
        };
        let turns = read_log(&path)?;

        if turn == 0 || turn as usize > turns.len() {
            return Err(format!(
                "session '{session_id}' has {} checkpointed turn{}; {turn} is not one of them",
                turns.len(),
                if turns.len() == 1 { "" } else { "s" }
            ));
        }

        // First writer wins, so iterating forward from the target turn leaves
        // each path holding the oldest pre-image at or after it.
        let mut earliest: Vec<(String, State)> = Vec::new();
        for entry in &turns[turn as usize - 1..] {
            for (file, state) in &entry.changes {
                if !earliest.iter().any(|(seen, _)| seen == file) {
                    earliest.push((file.clone(), state.clone()));
                }
            }
        }
        earliest.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(earliest
            .into_iter()
            .map(|(file, state)| restore(workspace, &file, &state, dry_run))
            .collect())
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
}

/// Rebuilds a log's turns, assigning numbers from their order.
///
/// A trailing line that will not parse is dropped rather than failing the
/// read: it is the turn that was in flight when the process died, and refusing
/// the file would lose every earlier checkpoint over it.
fn read_log(path: &Path) -> Result<Vec<ReadTurn>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        // No log is not an error: it is a session that never changed a file.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };

    let mut turns: Vec<ReadTurn> = Vec::new();
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
            Ok(Record::Turn { prompt, at }) => turns.push(ReadTurn {
                prompt,
                at,
                changes: Vec::new(),
            }),
            Ok(Record::Before { path, state }) => {
                // A `before` with no open turn is a torn log; there is nothing
                // to attach it to, and inventing a turn for it would produce a
                // checkpoint the user never made.
                if let Some(turn) = turns.last_mut() {
                    turn.changes.push((path, state));
                }
            }
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
    Ok(turns)
}

#[derive(Default)]
struct TurnState {
    /// Paths already captured this turn. The first capture is the pre-image;
    /// later ones would record what an earlier call in the same turn wrote.
    seen: HashSet<PathBuf>,
    opened: bool,
    /// Set after a write fails, so one broken log does not narrate every call.
    disabled: bool,
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
            // Asks the file whether it has a header rather than whether it
            // exists. `append` creates the file before it writes, so a write
            // that fails leaves an empty one behind — and "it exists" would
            // then be true forever, so every later turn skipped the header and
            // left a log that `read_log` could not accept. Checking for the
            // record itself also repairs a log already in that state, on its
            // next turn.
            let header = (!has_header(&log)).then(|| {
                Record::Header(Header {
                    version: FORMAT_VERSION,
                    session: self.session.clone(),
                    workspace: self.workspace.display().to_string(),
                })
            });
            for record in header.into_iter().chain([Record::Turn {
                prompt: self.prompt.clone(),
                at: now(),
            }]) {
                if !append(&log, &record) {
                    state.disabled = true;
                    return;
                }
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

/// Whether the log already carries a header record.
///
/// Scans for one anywhere rather than checking the first line, because a log
/// repaired after a failed first write carries its header after the turns it
/// was missing from. A log is one short line per changed file, and the normal
/// case stops on line one.
fn has_header(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| matches!(serde_json::from_str::<Record>(&line), Ok(Record::Header(_))))
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

        let restored = f.store.rewind("s1", &f.root, 1, false).unwrap();
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

        let restored = f.store.rewind("s1", &f.root, 1, false).unwrap();
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

        let planned = f.store.rewind("s1", &f.root, 1, true).unwrap();
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

        let restored = f.store.rewind("s1", &f.root, 1, false).unwrap();
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

        assert!(has_header(&f.log("s1")), "the next turn must repair it");
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

        let restored = f.store.rewind("s1", &f.root, 1, false).unwrap();
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
