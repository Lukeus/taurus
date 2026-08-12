//! Transcripts that outlive the process.
//!
//! Sessions live under `~/.taurus/sessions/<workspace-key>/<id>.jsonl`, in the
//! global config home rather than in the workspace they belong to. A transcript
//! holds file contents, shell output, and whatever MCP servers returned; kept
//! inside the project it would be committed by accident. Keying the directory
//! by workspace gives back the only thing the location cost — "show me this
//! project's sessions" — without putting any of it in the repository.
//!
//! The file is append-only JSONL: a header line, then one line per message as
//! it is produced. Nothing is rewritten, so a crash mid-turn costs the turn in
//! progress rather than the whole conversation, and a half-written final line
//! is dropped on load instead of poisoning the file.
//!
//! There is deliberately no index file. Everything a listing needs is in the
//! transcript itself — normally in its opening lines — and an index is a second
//! copy of the truth that can disagree with it.
//!
//! Reading and listing apply the same rule about what a valid transcript is. A
//! listing stricter than the loader would hide conversations that open fine
//! when asked for by id, and the listing is the only way anybody reaches them.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use taurus_core::Session;
use taurus_provider::{ContentBlock, Message, Role, TokenUsage};

/// Bumped when a record shape changes incompatibly. Transcripts written by a
/// newer version are skipped rather than misread.
const FORMAT_VERSION: u32 = 1;

const EXTENSION: &str = "jsonl";

/// How far into a transcript to look for the line that titles it. A session
/// whose first user message is somehow past this is listed by its id.
const TITLE_SCAN_LINES: usize = 64;

const TITLE_MAX_CHARS: usize = 80;

pub fn sessions_dir() -> PathBuf {
    crate::config::home_dir().join("sessions")
}

/// Where one workspace's checkpoint logs live.
///
/// Alongside transcripts and keyed the same way, for the same reason: a
/// checkpoint holds the contents of files in the project, so keeping it in the
/// project would commit them. It shares [`workspace_key`] so that both halves
/// of a conversation's record — what was said and what was changed — land in
/// directories a person can pair up by eye.
pub fn checkpoints_dir(workspace: &Path) -> PathBuf {
    crate::config::home_dir()
        .join("checkpoints")
        .join(workspace_key(workspace))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// A filesystem-safe directory name for a workspace.
///
/// The basename keeps the directory browsable; the hash is what actually
/// distinguishes two checkouts of the same project. FNV-1a rather than
/// `DefaultHasher` because the standard hasher's algorithm is explicitly
/// unspecified across Rust releases — a toolchain upgrade would silently
/// re-key every directory and orphan the sessions in them.
pub fn workspace_key(workspace: &Path) -> String {
    let path = workspace.to_string_lossy();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }

    let name: String = workspace
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "root".into())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(32)
        .collect();

    format!("{}-{hash:016x}", name.trim_matches('-'))
}

/// One line of a transcript.
///
/// Internally tagged so an unrecognized record from a future version can be
/// skipped by name, and so a line reads as what it is when someone opens the
/// file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record {
    Header(Header),
    Message(Message),
    /// A running total rather than a per-turn delta: the last one wins, so a
    /// dropped tail line costs accuracy in the token counter and nothing else.
    Usage(TokenUsage),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    id: String,
    workspace: String,
    model: String,
    started: u64,
}

/// What a listing shows, read from a transcript's own opening lines.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionMeta {
    pub id: String,
    pub workspace: String,
    pub model: String,
    /// Unix seconds. Formatting is the frontend's business.
    ///
    /// Declared to TypeScript as `number` rather than the `bigint` a `u64`
    /// would otherwise become: seconds since 1970 stay exact in a double until
    /// the year 285-million-odd, and `bigint` would cost every consumer a
    /// conversion for precision no timestamp will ever use.
    #[ts(type = "number")]
    pub started: u64,
    /// Unix seconds, from the file's own mtime — the last turn recorded.
    #[ts(type = "number")]
    pub updated: u64,
    /// The first thing asked, shortened. Empty for a session with no turns.
    pub title: String,
}

/// An open transcript, appended to as turns complete.
///
/// Writes never fail a turn: persistence is a side effect of work the user
/// asked for, and a full disk or a read-only home must cost them the record of
/// the conversation, not the conversation. A failure is reported once and then
/// retried quietly on the next turn — a disk that frees up must not leave the
/// rest of the conversation unrecorded because one early write missed.
pub struct SessionLog {
    path: PathBuf,
    /// The header this transcript must carry, held until the file is known to
    /// have one. Kept rather than written once and forgotten, so a log whose
    /// header write failed can still be repaired on a later turn.
    header: Option<Header>,
    /// How many of the session's messages are already on disk. Advanced per
    /// message that actually landed, never past one that did not.
    persisted: usize,
    /// A log that must never write anything. Distinct from a write that failed,
    /// which is retried.
    off: bool,
    /// Set after a failure, so one broken log does not narrate every turn.
    warned: bool,
}

impl SessionLog {
    /// Starts a transcript for a new session and writes its header.
    pub fn create(session: &Session, workspace: &Path) -> Self {
        let mut log = Self {
            path: transcript_path(session, workspace),
            header: Some(header_for(session, workspace)),
            persisted: 0,
            off: false,
            warned: false,
        };
        log.ensure_header();
        // Nothing is on disk yet, so every message the session already holds
        // still has to be written. Adopting an existing transcript is
        // `resume`'s job, not this one's.
        log
    }

    /// Reopens the transcript a session was loaded from, to append to it.
    pub fn resume(session: &Session, workspace: &Path) -> Self {
        Self {
            path: transcript_path(session, workspace),
            // Carried but almost never written: a transcript that loaded has a
            // header by definition, so this is only reached if the file lost
            // one. `started` is unrecoverable in that case, and the listing
            // does not show it.
            header: Some(header_for(session, workspace)),
            persisted: session.messages.len(),
            off: false,
            warned: false,
        }
    }

    /// A log that writes nowhere. For runs that must leave no trace, and tests.
    pub fn disabled() -> Self {
        Self {
            path: PathBuf::new(),
            header: None,
            persisted: 0,
            off: true,
            warned: false,
        }
    }

    /// Writes the header unless the transcript already carries one.
    ///
    /// Asks the file for the record rather than for its own existence:
    /// `try_write` creates the file before it writes to it, so a write that
    /// failed leaves an empty transcript behind, and "the file is there" would
    /// be true from then on. Asking for the record also repairs a transcript
    /// that is already missing one.
    fn ensure_header(&mut self) {
        if self.off || self.header.is_none() {
            return;
        }
        if has_header(&self.path) {
            self.header = None;
            return;
        }
        if let Some(header) = self.header.clone() {
            if self.write(&Record::Header(header)) {
                self.header = None;
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends whatever the session gained since the last call.
    pub fn record(&mut self, session: &Session) {
        if self.off {
            return;
        }

        // Retried every turn rather than attempted once at creation: a
        // transient failure must cost the turn that hit it, not the record of
        // the whole conversation.
        self.ensure_header();
        if self.header.is_some() {
            // Still no header on disk. Messages appended under one would make a
            // transcript `load` cannot identify, so leave them for the turn
            // that manages to write it.
            return;
        }

        // Guards a resumed or replaced session whose history is shorter than
        // what has been written; appending from a stale offset would duplicate.
        let start = self.persisted.min(session.messages.len());
        self.persisted = start;
        for message in &session.messages[start..] {
            if !self.write(&Record::Message(message.clone())) {
                // Left pointing at the message that did not land, so the next
                // turn writes it rather than skipping past it.
                return;
            }
            self.persisted += 1;
        }
        self.write(&Record::Usage(session.usage));
    }

    /// Appends one record. Returns whether it landed.
    fn write(&mut self, record: &Record) -> bool {
        if self.off {
            return false;
        }
        match self.try_write(record) {
            Ok(()) => true,
            Err(e) => {
                if !self.warned {
                    tracing::warn!(
                        path = %self.path.display(),
                        error = %e,
                        "could not write the session transcript; retrying on the next turn"
                    );
                    self.warned = true;
                }
                false
            }
        }
    }

    fn try_write(&self, record: &Record) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let line = serde_json::to_string(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")
    }
}

fn transcript_path(session: &Session, workspace: &Path) -> PathBuf {
    sessions_dir()
        .join(workspace_key(workspace))
        .join(format!("{}.{EXTENSION}", session.id))
}

fn header_for(session: &Session, workspace: &Path) -> Header {
    Header {
        version: FORMAT_VERSION,
        id: session.id.clone(),
        workspace: workspace.display().to_string(),
        model: session.model.clone(),
        started: now(),
    }
}

/// Whether a transcript already carries a header record.
///
/// Scans for the record rather than reading the first line, so that this agrees
/// with [`load`], which accepts a header wherever it appears. `any` stops at the
/// first match, so the ordinary case reads one line and no more.
fn has_header(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| matches!(serde_json::from_str::<Record>(&line), Ok(Record::Header(_))))
}

/// Rebuilds a session from its transcript.
///
/// A trailing line that will not parse is dropped rather than failing the load:
/// it is the turn that was in flight when the process died, and refusing to
/// open the file would lose the rest of the conversation over it.
pub fn load(id: &str) -> Result<(Session, PathBuf), String> {
    let path = find(id).ok_or_else(|| format!("no saved session '{id}'"))?;
    let file = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;

    let mut header: Option<Header> = None;
    let mut messages = Vec::new();
    let mut usage = TokenUsage::default();

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Record>(&line) {
            Ok(Record::Header(h)) => header = Some(h),
            Ok(Record::Message(m)) => messages.push(m),
            Ok(Record::Usage(u)) => usage = u,
            Err(e) => {
                tracing::debug!(error = %e, "skipping an unreadable transcript line");
            }
        }
    }

    let header = header.ok_or_else(|| format!("{} has no header", path.display()))?;
    if header.version > FORMAT_VERSION {
        return Err(format!(
            "session '{id}' was written by a newer version of Taurus (format {} > {FORMAT_VERSION})",
            header.version
        ));
    }

    Ok((
        Session {
            id: header.id,
            model: header.model,
            messages,
            usage,
        },
        path,
    ))
}

/// Sessions for one workspace, or for every workspace, newest first.
pub fn list(workspace: Option<&Path>) -> Vec<SessionMeta> {
    let dirs: Vec<PathBuf> = match workspace {
        Some(workspace) => vec![sessions_dir().join(workspace_key(workspace))],
        None => std::fs::read_dir(sessions_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
    };

    let mut sessions: Vec<SessionMeta> = dirs
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == EXTENSION))
        .filter_map(|path| read_meta(&path))
        .collect();

    // Descending, so the newest is first and `latest` is just the head.
    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated));
    sessions
}

/// The most recently updated session for a workspace.
pub fn latest(workspace: &Path) -> Option<SessionMeta> {
    list(Some(workspace)).into_iter().next()
}

/// Reads a listing entry without reading the whole transcript.
///
/// Bounded on purpose: a session that read a large file has a transcript to
/// match, and a listing that parses all of them for every entry would make
/// `taurus sessions` slower than the turns it is listing.
///
/// The header is looked for rather than demanded on line one, because [`load`]
/// accepts it anywhere. A listing stricter than the loader hides sessions that
/// open perfectly well when asked for by id, which is worse than either rule on
/// its own — the conversation is there, and only the one screen that would let
/// somebody reach it disagrees.
fn read_meta(path: &Path) -> Option<SessionMeta> {
    let updated = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_default();

    let file = std::fs::File::open(path).ok()?;

    let mut header: Option<Header> = None;
    let mut title: Option<String> = None;

    // One pass for both, stopping as soon as nothing worth finding is left. A
    // transcript normally opens with its header and reaches the first question
    // a line or two later, so this reads the top of the file and returns.
    for (index, line) in BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .enumerate()
    {
        match serde_json::from_str::<Record>(&line) {
            Ok(Record::Header(found)) => header = Some(found),
            // Tool results are skipped: a resumed turn can begin with one, and
            // titling a session with the contents of a file it happened to read
            // tells the user nothing about what they asked for. `first_text`
            // returns `None` for those, which leaves the search running.
            Ok(Record::Message(message)) if title.is_none() && message.role == Role::User => {
                title = first_text(&message);
            }
            _ => {}
        }
        if header.is_some() && (title.is_some() || index + 1 >= TITLE_SCAN_LINES) {
            break;
        }
    }

    let header = header?;
    if header.version > FORMAT_VERSION {
        return None;
    }

    Some(SessionMeta {
        id: header.id,
        workspace: header.workspace,
        model: header.model,
        started: header.started,
        updated,
        title: title.unwrap_or_default(),
    })
}

/// The first line of a message's text, shortened for a listing.
///
/// Tool results are skipped: a resumed turn can begin with one, and titling a
/// session with the contents of a file it happened to read tells the user
/// nothing about what they asked for.
fn first_text(message: &Message) -> Option<String> {
    let text = message.content.iter().find_map(|block| match block {
        ContentBlock::Text { text } => Some(text.trim()),
        _ => None,
    })?;

    let line = text.lines().find(|l| !l.trim().is_empty())?.trim();
    if line.chars().count() > TITLE_MAX_CHARS {
        Some(format!(
            "{}…",
            line.chars().take(TITLE_MAX_CHARS - 1).collect::<String>()
        ))
    } else {
        Some(line.to_string())
    }
}

/// Locates a transcript by id across every workspace.
fn find(id: &str) -> Option<PathBuf> {
    // Rejected before it reaches the filesystem: an id is used as a filename,
    // and one carrying separators would resolve outside the sessions tree.
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return None;
    }
    let filename = format!("{id}.{EXTENSION}");
    std::fs::read_dir(sessions_dir())
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&filename))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::isolated_home;

    fn session_with(id: &str, turns: &[&str]) -> Session {
        let mut session = Session::new("test-model");
        session.id = id.to_string();
        for text in turns {
            session.push(Message::user(*text));
            session.push(Message::assistant("ok"));
        }
        session
    }

    #[test]
    fn a_session_survives_the_process_that_wrote_it() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/project");

        let mut session = session_with("abc123", &["first question"]);
        session.add_usage(TokenUsage {
            input_tokens: 10,
            output_tokens: 4,
        });
        let mut log = SessionLog::create(&session, workspace);
        log.record(&session);

        let (loaded, _) = load("abc123").expect("the transcript should reload");
        assert_eq!(loaded.id, "abc123");
        assert_eq!(loaded.model, "test-model");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.usage.total(), 14);
    }

    #[test]
    fn recording_twice_appends_only_what_is_new() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/project");

        let mut session = session_with("append1", &["one"]);
        let mut log = SessionLog::create(&session, workspace);
        log.record(&session);

        session.push(Message::user("two"));
        session.push(Message::assistant("ok"));
        log.record(&session);

        let (loaded, _) = load("append1").unwrap();
        assert_eq!(loaded.messages.len(), 4, "a message was duplicated or lost");
    }

    #[test]
    fn a_resumed_session_does_not_rewrite_the_history_it_loaded() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/project");

        let session = session_with("resume1", &["one", "two"]);
        let mut log = SessionLog::create(&session, workspace);
        log.record(&session);

        let (mut loaded, _) = load("resume1").unwrap();
        let mut resumed = SessionLog::resume(&loaded, workspace);
        loaded.push(Message::user("three"));
        resumed.record(&loaded);

        let (again, _) = load("resume1").unwrap();
        assert_eq!(again.messages.len(), 5);
    }

    #[test]
    fn a_torn_final_line_costs_the_last_turn_and_nothing_else() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/project");

        let session = session_with("torn1", &["a question"]);
        let mut log = SessionLog::create(&session, workspace);
        log.record(&session);

        // What a process killed mid-write leaves behind.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(log.path())
            .unwrap();
        file.write_all(br#"{"type":"message","role":"user","cont"#)
            .unwrap();
        drop(file);

        let (loaded, _) = load("torn1").expect("a torn tail must not fail the load");
        assert_eq!(loaded.messages.len(), 2);
    }

    #[test]
    fn listing_is_scoped_to_a_workspace_and_titled_by_the_first_question() {
        let _home = isolated_home();
        let mine = Path::new("/tmp/mine");
        let theirs = Path::new("/tmp/theirs");

        for (id, workspace, question) in [
            ("s1", mine, "summarize the readme"),
            ("s2", theirs, "fix the build"),
        ] {
            let session = session_with(id, &[question]);
            let mut log = SessionLog::create(&session, workspace);
            log.record(&session);
        }

        let listed = list(Some(mine));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "s1");
        assert_eq!(listed[0].title, "summarize the readme");
        assert_eq!(listed[0].model, "test-model");

        assert_eq!(list(None).len(), 2, "every workspace, unscoped");
    }

    #[test]
    fn two_checkouts_of_the_same_project_do_not_share_a_directory() {
        let a = workspace_key(Path::new("/home/me/src/taurus"));
        let b = workspace_key(Path::new("/home/me/work/taurus"));
        assert_ne!(a, b);
        // Still recognizable to someone browsing the directory.
        assert!(a.starts_with("taurus-"), "{a}");
        assert_eq!(a, workspace_key(Path::new("/home/me/src/taurus")));
    }

    #[test]
    fn an_id_that_would_escape_the_sessions_tree_finds_nothing() {
        let _home = isolated_home();
        for hostile in ["../../etc/passwd", "a/b", "", "..'"] {
            assert!(load(hostile).is_err(), "{hostile} was accepted");
        }
    }

    #[test]
    fn a_session_with_no_turns_lists_without_a_title() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/empty");
        let session = session_with("empty1", &[]);
        SessionLog::create(&session, workspace);

        let listed = list(Some(workspace));
        assert_eq!(listed.len(), 1);
        assert!(listed[0].title.is_empty());
    }

    #[test]
    fn a_disabled_log_writes_nothing() {
        let _home = isolated_home();
        let mut log = SessionLog::disabled();
        log.record(&session_with("nope", &["secret"]));
        assert!(list(None).is_empty());
    }

    /// Puts a file where a workspace's transcript directory belongs, so
    /// `create_dir_all` fails and every write to it fails with it.
    ///
    /// Portable, unlike permission bits, and it models the case that matters:
    /// writes failing from the very first one, before any header exists.
    fn block_writes(workspace: &Path) -> PathBuf {
        let dir = sessions_dir().join(workspace_key(workspace));
        std::fs::create_dir_all(dir.parent().expect("sessions dir has a parent")).unwrap();
        std::fs::write(&dir, "in the way").unwrap();
        dir
    }

    #[test]
    fn a_failed_first_write_does_not_cost_the_whole_conversation() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/blocked");
        let obstruction = block_writes(workspace);

        let mut session = session_with("blocked1", &["first question"]);
        let mut log = SessionLog::create(&session, workspace);
        log.record(&session);
        assert!(
            load("blocked1").is_err(),
            "the write failed, so there is nothing to load yet"
        );

        // Whatever it was clears: a full disk with room again, a home that is
        // no longer read-only.
        std::fs::remove_file(&obstruction).unwrap();

        session.push(Message::user("second question"));
        session.push(Message::assistant("ok"));
        log.record(&session);

        let (loaded, _) = load("blocked1")
            .expect("the next turn must write the header it could not write before");
        assert_eq!(
            loaded.messages.len(),
            4,
            "the turn that failed must be written too, not skipped past"
        );
        assert_eq!(loaded.model, "test-model");
        assert_eq!(
            list(Some(workspace)).len(),
            1,
            "a repaired transcript has to reach the listing, not just the loader"
        );
    }

    #[test]
    fn a_transcript_carries_exactly_one_header_however_many_turns_land() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/onceonly");

        let mut session = session_with("once1", &["one"]);
        let mut log = SessionLog::create(&session, workspace);
        log.record(&session);
        session.push(Message::user("two"));
        session.push(Message::assistant("ok"));
        log.record(&session);

        let text = std::fs::read_to_string(log.path()).unwrap();
        let headers = text
            .lines()
            .filter(|line| matches!(serde_json::from_str::<Record>(line), Ok(Record::Header(_))))
            .count();
        assert_eq!(headers, 1, "the header must not be rewritten every turn");
    }

    #[test]
    fn a_listing_accepts_a_header_that_is_not_on_the_first_line() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/outoforder");

        let dir = sessions_dir().join(workspace_key(workspace));
        std::fs::create_dir_all(&dir).unwrap();

        let message =
            serde_json::to_string(&Record::Message(Message::user("the question"))).unwrap();
        let header = serde_json::to_string(&Record::Header(Header {
            version: FORMAT_VERSION,
            id: "ooo1".into(),
            workspace: workspace.display().to_string(),
            model: "test-model".into(),
            started: 1,
        }))
        .unwrap();
        std::fs::write(dir.join("ooo1.jsonl"), format!("{message}\n{header}\n")).unwrap();

        // `load` reads a header wherever it sits, so the listing must agree. A
        // listing that is stricter hides a conversation that opens fine.
        let (loaded, _) = load("ooo1").expect("load accepts a header anywhere");
        assert_eq!(loaded.model, "test-model");

        let listed = list(Some(workspace));
        assert_eq!(
            listed.len(),
            1,
            "a session `load` can open must be listable"
        );
        assert_eq!(listed[0].title, "the question");
    }
}
