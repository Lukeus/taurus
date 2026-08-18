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
//! A conversation's delegates are written the same way, one level down:
//!
//! ```text
//! <workspace-key>/<id>.jsonl                        the conversation
//! <workspace-key>/<id>/subagents/agent-<id>.jsonl   what it delegated
//! ```
//!
//! Which keeps the parent's transcript the record of one delegation — a call
//! and the answer it returned — while the child's own reading, dead ends and
//! reasoning stay somewhere they can still be found afterwards.
//!
//! Reading and listing apply the same rule about what a valid transcript is. A
//! listing stricter than the loader would hide conversations that open fine
//! when asked for by id, and the listing is the only way anybody reaches them.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use ts_rs::TS;

use taurus_core::{Session, SubagentRecorder, TurnRecorder};
use taurus_provider::{ContentBlock, Message, Role, TokenUsage};

/// Bumped when a record shape changes incompatibly. Transcripts written by a
/// newer version are skipped rather than misread.
const FORMAT_VERSION: u32 = 1;

const EXTENSION: &str = "jsonl";

/// Where a conversation's delegates live: a directory beside its transcript,
/// named for it.
///
/// Nested rather than flat because [`list`] reads one level down and keeps only
/// `.jsonl` files, so a directory can never turn up in a listing as a session
/// of its own. A delegate is part of the conversation that spawned it, not a
/// conversation somebody had.
const SUBAGENT_DIR: &str = "subagents";

/// Prefixed so a delegate's file is recognizable as one from its name alone,
/// wherever it is opened — a listing, an editor, `ls`.
const SUBAGENT_PREFIX: &str = "agent-";

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
    /// The branch checked out when the conversation began.
    ///
    /// Recorded for the same reason `workspace` is: it is part of what the
    /// conversation is *about*. Every file path in the transcript, and every
    /// pre-image in the matching checkpoint log, describes the tree as it stood
    /// on this branch, so resuming somewhere else is a thing worth being told
    /// about rather than discovering through a rewind.
    ///
    /// `None` for a workspace with no repository, a detached HEAD, and every
    /// transcript written before this field existed — which is why it defaults
    /// rather than being required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    /// The conversation this one was delegated from, for a sub-agent's
    /// transcript. `None` for a conversation somebody had themselves.
    ///
    /// Recorded even though the file's own directory already says it: a
    /// transcript that is moved, copied out for a bug report, or read on its
    /// own has to still be able to say what it was a delegate *of*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    /// Which kind of sub-agent this was, by the name in its definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
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
    /// The branch this conversation was started on, when there was one. See
    /// [`Header::branch`].
    #[ts(optional)]
    pub branch: Option<String>,
    /// The kind of sub-agent this transcript belongs to, for one listed by
    /// [`list_subagents`]. `None` for an ordinary conversation.
    #[ts(optional)]
    pub agent: Option<String>,
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
    /// Starts a transcript for a new session.
    ///
    /// Nothing is written until there is something to record. A session is
    /// created by every model switch and every press of **New conversation**,
    /// and writing the header here made a file for each — which listed, so the
    /// rail filled with untitled rows for conversations that never happened,
    /// and reopening "the most recent" reopened one of them. The header is
    /// written by the first [`Self::record`], which had to be able to write it
    /// anyway.
    ///
    /// `branch` is passed in rather than looked up, because asking git is an
    /// async round trip and this crate's transcript layer has no business
    /// knowing that git exists. `None` covers a workspace with no repository, a
    /// detached HEAD, and every caller that does not care.
    pub fn create(session: &Session, workspace: &Path, branch: Option<String>) -> Self {
        // `persisted` is zero because nothing is on disk yet, so every message
        // the session already holds still has to be written. Adopting an
        // existing transcript is `resume`'s job, not this one's.
        Self {
            path: transcript_path(session, workspace),
            header: Some(header_for(session, workspace, branch)),
            persisted: 0,
            off: false,
            warned: false,
        }
    }

    /// Reopens the transcript a session was loaded from, to append to it.
    ///
    /// Takes the whole [`Loaded`] rather than a session and a workspace,
    /// because the file to append to is the one it was read from — not one
    /// derived from wherever the app happens to be pointing now. Derived, it
    /// forked the conversation: [`load`] finds a transcript by id across every
    /// workspace, so resuming one from anywhere but its own folder wrote the
    /// rest of it into a second file under a second key, and both halves then
    /// listed as separate conversations with the same id.
    pub fn resume(loaded: &Loaded) -> Self {
        Self {
            path: loaded.path.clone(),
            // Carried but almost never written: a transcript that loaded has a
            // header by definition, so this is only reached if the file lost
            // one. `started` and `branch` are unrecoverable in that case — the
            // branch this is resuming on is not the one it began on, and
            // writing today's would be a worse record than none.
            header: Some(header_for(&loaded.session, &loaded.workspace, None)),
            persisted: loaded.session.messages.len(),
            off: false,
            warned: false,
        }
    }

    /// Starts a delegate's transcript, in its parent's own directory.
    ///
    /// Takes the parent's *id* rather than its log, because a delegation
    /// starts while the parent's turn is still running and its log is held by
    /// whoever is driving that turn. Nothing about writing this file needs to
    /// wait for that lock, and taking it would mean a child could stall a
    /// parent's turn to write a line.
    ///
    /// No branch: a delegate runs inside its parent's turn, on whatever the
    /// parent recorded, and a second copy of that answer is one more thing that
    /// can disagree.
    pub fn for_subagent(child: &Session, workspace: &Path, parent: &str, agent: &str) -> Self {
        let mut header = header_for(child, workspace, None);
        header.parent = Some(parent.to_string());
        header.agent = Some(agent.to_string());
        Self {
            path: subagent_path(workspace, parent, &child.id),
            header: Some(header),
            persisted: 0,
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

/// Where one conversation's delegates are written.
fn subagents_dir_in(workspace: &Path, parent: &str) -> PathBuf {
    sessions_dir()
        .join(workspace_key(workspace))
        .join(parent)
        .join(SUBAGENT_DIR)
}

fn subagent_path(workspace: &Path, parent: &str, child: &str) -> PathBuf {
    subagents_dir_in(workspace, parent).join(format!("{SUBAGENT_PREFIX}{child}.{EXTENSION}"))
}

fn header_for(session: &Session, workspace: &Path, branch: Option<String>) -> Header {
    Header {
        version: FORMAT_VERSION,
        id: session.id.clone(),
        workspace: workspace.display().to_string(),
        model: session.model.clone(),
        started: now(),
        branch,
        parent: None,
        agent: None,
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

/// A conversation read back off disk, with the two facts about *where* it lives
/// that the session alone cannot answer.
///
/// Both matter because [`load`] finds a transcript by id across every
/// workspace, so what comes back is not necessarily from the folder that is
/// open. `path` is the file to go on appending to, and `workspace` is the
/// project the conversation is about — the tree its file paths describe, and
/// the one its checkpoints are keyed by.
pub struct Loaded {
    pub session: Session,
    /// The transcript this was read from.
    pub path: PathBuf,
    /// The workspace it was started in, from its own header.
    pub workspace: PathBuf,
}

/// Rebuilds a session from its transcript.
///
/// A trailing line that will not parse is dropped rather than failing the load:
/// it is the turn that was in flight when the process died, and refusing to
/// open the file would lose the rest of the conversation over it.
pub fn load(id: &str) -> Result<Loaded, String> {
    read_transcript(find(id).ok_or_else(|| format!("no saved session '{id}'"))?)
}

/// Reads one transcript file, whoever it belongs to.
///
/// Shared by [`load`] and [`load_subagent`] rather than copied, so a delegate's
/// transcript is as tolerant of a half-written tail as its parent's — which
/// matters more for a delegate, not less: a turn interrupted mid-delegation is
/// exactly the case its transcript exists for.
fn read_transcript(path: PathBuf) -> Result<Loaded, String> {
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
            "{} was written by a newer version of Taurus (format {} > {FORMAT_VERSION})",
            path.display(),
            header.version
        ));
    }

    Ok(Loaded {
        workspace: PathBuf::from(header.workspace),
        session: Session {
            id: header.id,
            model: header.model,
            messages,
            usage,
        },
        path,
    })
}

/// The workspace a saved conversation belongs to, read from its header alone.
///
/// A cheap answer to the one question [`load`] is otherwise the only way to
/// ask. Anything reading a conversation's checkpoints has to know which project
/// they are keyed by, and reading a whole transcript — which holds every file
/// this conversation ever read — to learn one path would make listing the
/// changed files of an open conversation cost more than the turn that changed
/// them.
pub fn workspace_of(id: &str) -> Option<PathBuf> {
    read_meta(&find(id)?).map(|meta| PathBuf::from(meta.workspace))
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

/// One conversation's delegates, newest first.
///
/// Empty for a conversation that delegated nothing, and for one whose delegates
/// were recorded by a build that did not keep them — both of which are the same
/// answer to the caller: there is nothing to show.
pub fn list_subagents(parent: &str) -> Vec<SessionMeta> {
    let Some(dir) = subagents_dir(parent) else {
        return Vec::new();
    };
    let mut listed: Vec<SessionMeta> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == EXTENSION))
        .filter_map(|path| read_meta(&path))
        .collect();
    listed.sort_by_key(|s| std::cmp::Reverse(s.updated));
    listed
}

/// Reads one delegate's conversation.
///
/// Scoped to its parent rather than found by id alone, because that is what a
/// delegate's id *is*: unique inside the conversation that spawned it, and not
/// something the rest of the tree indexes.
pub fn load_subagent(parent: &str, child: &str) -> Result<Loaded, String> {
    if !usable_as_filename(child) {
        return Err(format!("'{child}' is not a sub-agent id"));
    }
    let dir = subagents_dir(parent)
        .ok_or_else(|| format!("session '{parent}' has no recorded sub-agents"))?;
    let path = dir.join(format!("{SUBAGENT_PREFIX}{child}.{EXTENSION}"));
    if !path.is_file() {
        return Err(format!("no sub-agent '{child}' under session '{parent}'"));
    }
    read_transcript(path)
}

/// Locates a conversation's delegates across every workspace.
///
/// Looks for the directory rather than deriving it from the parent's
/// transcript, so delegates are still readable from a conversation whose own
/// first turn never made it to disk — the delegation runs *during* that turn,
/// and writes before it.
///
/// Public because pointing somebody at the files is a useful answer on its own:
/// there is no viewer for one of these yet, and `taurus sessions --agents`
/// prints this so the transcripts can be read with whatever is to hand.
pub fn subagents_dir(parent: &str) -> Option<PathBuf> {
    if !usable_as_filename(parent) {
        return None;
    }
    std::fs::read_dir(sessions_dir())
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(parent).join(SUBAGENT_DIR))
        .find(|path| path.is_dir())
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
        branch: header.branch,
        agent: header.agent,
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

/// Erases a saved conversation.
///
/// The transcript is the conversation — there is no index to keep in step and
/// nothing else on disk refers to it — so removing the file is the whole
/// operation. Its checkpoint log is a separate tree with a separate owner, and
/// deleting that is the caller's to decide.
///
/// A session that is not there is an error rather than a quiet success: the
/// only way to ask for one is from a listing that just showed it, so a missing
/// transcript means something else removed it and the user should be told
/// rather than shown a confirmation for work that did not happen.
pub fn delete(id: &str) -> Result<(), String> {
    let path = find(id).ok_or_else(|| format!("no saved session '{id}'"))?;

    // The delegates go with it. They are part of this conversation — recorded
    // under its id, unreachable once it is gone — and leaving them would turn
    // every deleted session into a directory nothing lists and nothing can
    // open. Best effort, and before the transcript rather than after: a
    // conversation whose delegates could not be removed is still deleted, but
    // one that reported success while its own transcript survived would not be.
    if let Some(dir) = path.parent().map(|parent| parent.join(id)) {
        if dir.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                tracing::warn!(
                    path = %dir.display(),
                    error = %e,
                    "could not remove a session's sub-agent transcripts"
                );
            }
        }
    }

    std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether an id can be put in a path at all.
///
/// Checked before it reaches the filesystem: an id is used as a filename, and
/// one carrying separators or a `..` would resolve outside the sessions tree.
/// Applied to a delegate's id as well as a conversation's — a sub-agent id
/// comes from the same generator, but it now also arrives from a frontend
/// asking to read one back.
fn usable_as_filename(id: &str) -> bool {
    !id.is_empty() && !id.contains(['/', '\\', '.'])
}

/// Locates a transcript by id across every workspace.
fn find(id: &str) -> Option<PathBuf> {
    if !usable_as_filename(id) {
        return None;
    }
    let filename = format!("{id}.{EXTENSION}");
    std::fs::read_dir(sessions_dir())
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&filename))
        .find(|path| path.is_file())
}

/// Keeps a conversation's delegates, under the conversation that spawned them.
///
/// Built per turn by [`crate::Host::build_agent`], so the CLI and the desktop
/// app cannot disagree about whether delegation is recorded — the same reason
/// the rest of a turn's wiring is built there.
///
/// A no-trace mode, when there is one, turns this off with [`Self::disabled`]:
/// a session the user asked not to record must not leave its delegates behind,
/// which is precisely the kind of thing that gets missed when two layers each
/// decide separately what to write.
pub struct SubagentLogs {
    workspace: PathBuf,
    parent: String,
    off: bool,
}

impl SubagentLogs {
    pub fn new(workspace: PathBuf, parent: impl Into<String>) -> Self {
        Self {
            workspace,
            parent: parent.into(),
            off: false,
        }
    }

    /// A recorder that writes nowhere.
    pub fn disabled() -> Self {
        Self {
            workspace: PathBuf::new(),
            parent: String::new(),
            off: true,
        }
    }
}

#[async_trait]
impl SubagentRecorder for SubagentLogs {
    async fn open(&self, agent_type: &str, child: &Session) -> Option<Arc<dyn TurnRecorder>> {
        if self.off {
            return None;
        }
        Some(Arc::new(SubagentLog {
            log: Mutex::new(SessionLog::for_subagent(
                child,
                &self.workspace,
                &self.parent,
                agent_type,
            )),
        }))
    }
}

/// One delegate's transcript.
///
/// A lock per child rather than one across all of them: delegates run
/// concurrently, they write to separate files, and a shared lock would make
/// each one wait on the others' disks for nothing.
struct SubagentLog {
    log: Mutex<SessionLog>,
}

#[async_trait]
impl TurnRecorder for SubagentLog {
    async fn record(&self, session: &Session) {
        self.log.lock().await.record(session);
    }
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
        let mut log = SessionLog::create(&session, workspace, None);
        log.record(&session);

        let loaded = load("abc123")
            .expect("the transcript should reload")
            .session;
        assert_eq!(loaded.id, "abc123");
        assert_eq!(loaded.model, "test-model");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.usage.total(), 14);
    }

    #[test]
    fn a_delegate_keeps_its_own_transcript_under_its_parent() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/delegating");

        let parent = session_with("parent1", &["find the bug"]);
        SessionLog::create(&parent, workspace, None).record(&parent);

        let child = session_with("child1", &["search everything"]);
        let mut log = SessionLog::for_subagent(&child, workspace, "parent1", "explorer");
        log.record(&child);

        let loaded = load_subagent("parent1", "child1").expect("the delegate should reload");
        assert_eq!(loaded.session.id, "child1");
        assert_eq!(loaded.session.messages.len(), 2);
        // Its own file, in its parent's directory, named for what it was.
        assert!(loaded
            .path
            .ends_with("parent1/subagents/agent-child1.jsonl"));
    }

    #[test]
    fn a_delegate_does_not_list_as_a_conversation() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/notaconversation");

        let parent = session_with("parent2", &["do the thing"]);
        SessionLog::create(&parent, workspace, None).record(&parent);

        let child = session_with("child2", &["the delegated part"]);
        SessionLog::for_subagent(&child, workspace, "parent2", "explorer").record(&child);

        // The rail shows conversations somebody had. A delegate is part of one.
        let listed = list(Some(workspace));
        assert_eq!(listed.len(), 1, "{listed:#?}");
        assert_eq!(listed[0].id, "parent2");

        let delegates = list_subagents("parent2");
        assert_eq!(delegates.len(), 1);
        assert_eq!(delegates[0].id, "child2");
        assert_eq!(delegates[0].agent.as_deref(), Some("explorer"));
        assert_eq!(delegates[0].title, "the delegated part");
    }

    #[test]
    fn deleting_a_conversation_takes_its_delegates_with_it() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/deleteme");

        let parent = session_with("parent3", &["delegate this"]);
        SessionLog::create(&parent, workspace, None).record(&parent);
        let child = session_with("child3", &["work"]);
        SessionLog::for_subagent(&child, workspace, "parent3", "explorer").record(&child);

        delete("parent3").expect("the conversation should delete");

        assert!(load("parent3").is_err());
        assert!(load_subagent("parent3", "child3").is_err());
        // Not merely unreadable — gone. A directory nothing lists and nothing
        // can open is the thing this test exists to prevent.
        assert!(subagents_dir("parent3").is_none());
    }

    #[test]
    fn a_delegate_is_recorded_as_it_runs_not_at_the_end() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/midflight");

        // What a child looks like part-way through: one round done, more to
        // come. The process dying here must still leave the round that landed.
        let mut child = session_with("child4", &["first round"]);
        let mut log = SessionLog::for_subagent(&child, workspace, "parent4", "explorer");
        log.record(&child);

        let midflight = load_subagent("parent4", "child4").expect("the first round is on disk");
        assert_eq!(midflight.session.messages.len(), 2);

        child.push(Message::user("second round"));
        child.push(Message::assistant("done"));
        log.record(&child);

        let finished = load_subagent("parent4", "child4").unwrap();
        assert_eq!(
            finished.session.messages.len(),
            4,
            "appended, not rewritten"
        );
    }

    #[tokio::test]
    async fn a_disabled_recorder_declines_to_open_anything() {
        let _home = isolated_home();
        let child = session_with("child5", &["work"]);

        let off = SubagentLogs::disabled();
        assert!(off.open("explorer", &child).await.is_none());

        let on = SubagentLogs::new(PathBuf::from("/tmp/recording"), "parent5");
        assert!(on.open("explorer", &child).await.is_some());
    }

    #[test]
    fn a_delegate_id_that_would_escape_the_sessions_tree_is_refused() {
        let _home = isolated_home();

        // Both halves are used as path segments, and one of them now arrives
        // from a frontend asking to read a transcript back.
        assert!(load_subagent("../../etc", "child").is_err());
        assert!(load_subagent("parent", "../../../etc/passwd").is_err());
        assert!(subagents_dir("..").is_none());
    }

    #[test]
    fn a_listing_remembers_the_branch_the_conversation_started_on() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/branchy");

        let session = session_with("onbranch", &["do a thing"]);
        let mut log = SessionLog::create(&session, workspace, Some("feat/widgets".into()));
        log.record(&session);

        let listed = list(Some(workspace));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].branch.as_deref(), Some("feat/widgets"));
    }

    #[test]
    fn a_transcript_written_before_branches_were_recorded_still_lists() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/nobranch");

        // The literal shape of an older header: no `branch` key at all. A
        // required field here would have made every existing conversation
        // unlistable on upgrade, which is why it defaults.
        let dir = sessions_dir().join(workspace_key(workspace));
        std::fs::create_dir_all(&dir).unwrap();
        let header = serde_json::json!({
            "type": "header",
            "version": FORMAT_VERSION,
            "id": "old1",
            "workspace": workspace.display().to_string(),
            "model": "test-model",
            "started": 1,
        });
        std::fs::write(dir.join("old1.jsonl"), format!("{header}\n")).unwrap();

        let listed = list(Some(workspace));
        assert_eq!(listed.len(), 1, "an older transcript must still be listed");
        assert_eq!(listed[0].branch, None);
        load("old1").expect("and must still open");
    }

    #[test]
    fn a_session_with_no_branch_writes_no_branch_key() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/plainrepo");

        // A workspace with no repository is the common case, and its header
        // should read the same as one written before the field existed rather
        // than carrying `"branch": null` on every line.
        let session = session_with("plain1", &["hello"]);
        SessionLog::create(&session, workspace, None).record(&session);

        let path = sessions_dir()
            .join(workspace_key(workspace))
            .join("plain1.jsonl");
        let written = std::fs::read_to_string(path).unwrap();
        assert!(!written.contains("branch"), "{written}");
    }

    #[test]
    fn recording_twice_appends_only_what_is_new() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/project");

        let mut session = session_with("append1", &["one"]);
        let mut log = SessionLog::create(&session, workspace, None);
        log.record(&session);

        session.push(Message::user("two"));
        session.push(Message::assistant("ok"));
        log.record(&session);

        let loaded = load("append1").unwrap().session;
        assert_eq!(loaded.messages.len(), 4, "a message was duplicated or lost");
    }

    #[test]
    fn a_resumed_session_does_not_rewrite_the_history_it_loaded() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/project");

        let session = session_with("resume1", &["one", "two"]);
        let mut log = SessionLog::create(&session, workspace, None);
        log.record(&session);

        let loaded = load("resume1").unwrap();
        let mut resumed = SessionLog::resume(&loaded);
        let mut session = loaded.session;
        session.push(Message::user("three"));
        resumed.record(&session);

        assert_eq!(load("resume1").unwrap().session.messages.len(), 5);
    }

    #[test]
    fn a_deleted_session_is_gone_from_the_listing_and_from_disk() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/project");

        for id in ["keep1", "drop1"] {
            let session = session_with(id, &["a question"]);
            let mut log = SessionLog::create(&session, workspace, None);
            log.record(&session);
        }

        delete("drop1").expect("a listed session should delete");

        assert!(load("drop1").is_err(), "the transcript is still readable");
        let ids: Vec<String> = list(Some(workspace)).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["keep1"], "the listing still offers a deleted session");
    }

    #[test]
    fn deleting_a_session_that_is_not_there_says_so() {
        // The only way to ask is from a listing that just showed it, so a
        // missing transcript means something else took it — and reporting
        // success would confirm work that did not happen.
        let _home = isolated_home();
        assert!(delete("never-existed").is_err());
    }

    #[test]
    fn an_id_that_would_escape_the_sessions_tree_deletes_nothing() {
        let _home = isolated_home();
        for id in ["../../etc/passwd", "..", "", "a/b"] {
            assert!(delete(id).is_err(), "'{id}' was accepted as a session id");
        }
    }

    #[test]
    fn a_torn_final_line_costs_the_last_turn_and_nothing_else() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/project");

        let session = session_with("torn1", &["a question"]);
        let mut log = SessionLog::create(&session, workspace, None);
        log.record(&session);

        // What a process killed mid-write leaves behind.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(log.path())
            .unwrap();
        file.write_all(br#"{"type":"message","role":"user","cont"#)
            .unwrap();
        drop(file);

        let loaded = load("torn1")
            .expect("a torn tail must not fail the load")
            .session;
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
            let mut log = SessionLog::create(&session, workspace, None);
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
    fn a_session_with_no_turns_writes_nothing_and_lists_as_nothing() {
        // Every model switch and every press of New conversation makes one of
        // these. Written eagerly, each left a file that listed, so the rail
        // filled with untitled rows for conversations that never happened and
        // "reopen the most recent" reopened one of them.
        let _home = isolated_home();
        let workspace = Path::new("/tmp/empty");
        let session = session_with("empty1", &[]);
        let log = SessionLog::create(&session, workspace, None);

        assert!(
            !log.path().exists(),
            "an unused session is not a transcript"
        );
        assert!(list(Some(workspace)).is_empty());
    }

    #[test]
    fn a_session_starts_being_recorded_the_moment_it_has_a_turn() {
        // The other half of the rule above: deferring the header must not cost
        // the conversation its record, only the empty file.
        let _home = isolated_home();
        let workspace = Path::new("/tmp/firstturn");
        let session = session_with("first1", &["a real question"]);
        SessionLog::create(&session, workspace, None).record(&session);

        let listed = list(Some(workspace));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "a real question");
        assert_eq!(load("first1").unwrap().session.messages.len(), 2);
    }

    #[test]
    fn a_resumed_conversation_appends_to_its_own_transcript_from_anywhere() {
        // `load` finds a transcript by id across every workspace, so a resume
        // that derived the path from the open folder wrote the rest of the
        // conversation into a second file under a second key — two halves
        // listing as two conversations sharing one id.
        let _home = isolated_home();
        let home_workspace = Path::new("/tmp/its-own-folder");

        let mut session = session_with("travelled1", &["started here"]);
        SessionLog::create(&session, home_workspace, None).record(&session);

        let loaded = load("travelled1").unwrap();
        assert_eq!(loaded.workspace, home_workspace);

        let mut log = SessionLog::resume(&loaded);
        session.push(Message::user("continued from elsewhere"));
        session.push(Message::assistant("ok"));
        log.record(&session);

        assert_eq!(
            list(Some(home_workspace)).len(),
            1,
            "the conversation must not have been forked into a second file"
        );
        assert_eq!(load("travelled1").unwrap().session.messages.len(), 4);
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
        let mut log = SessionLog::create(&session, workspace, None);
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

        let loaded = load("blocked1")
            .expect("the next turn must write the header it could not write before")
            .session;
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
        let mut log = SessionLog::create(&session, workspace, None);
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
            branch: None,
            parent: None,
            agent: None,
        }))
        .unwrap();
        std::fs::write(dir.join("ooo1.jsonl"), format!("{message}\n{header}\n")).unwrap();

        // `load` reads a header wherever it sits, so the listing must agree. A
        // listing that is stricter hides a conversation that opens fine.
        let loaded = load("ooo1")
            .expect("load accepts a header anywhere")
            .session;
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
