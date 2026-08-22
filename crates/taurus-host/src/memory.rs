//! What the last conversation in this workspace left unfinished.
//!
//! A session ends where it ends. The transcript is on disk and can be reopened,
//! but the next conversation starts with none of it — so the first thing a
//! person does the following morning is explain, again, what was being done and
//! how far it got. The model has no way to know that the auth refactor is half
//! applied, or that the flaky test was tracked to a clock and not to the code.
//!
//! This is the small, deliberate exception. A note the model writes when it has
//! a conclusion worth carrying, kept per workspace, and read back into the
//! system prompt of every later conversation in that workspace.
//!
//! # Why it is written rather than derived
//!
//! Every turn is already recorded, so the previous conversation could in
//! principle be summarized on demand instead. That was the other option and it
//! is worse in the way that matters: summarizing costs a model round trip
//! before the first question can be answered, and it can only recover what the
//! transcript happens to say. A note is written at the moment the conclusion is
//! reached, by the thing that reached it, and says the part that was worth
//! keeping rather than the part that was said.
//!
//! # Why it is not in the workspace
//!
//! Same reason transcripts and checkpoint logs are not: a note is prose about
//! the contents of somebody's project, and a file in the project is a file that
//! gets committed. It lives in the config home, keyed by workspace exactly like
//! those two, so the three halves of a workspace's record sit side by side.
//!
//! # What bounds it
//!
//! Notes are paid for in context on every request of every turn, which is the
//! same bargain [`crate::instructions`] makes and the same reason it is capped.
//! A note is a sentence or two; a model that tries to file a whole document is
//! refused rather than truncated, because a note cut off mid-sentence is worse
//! than one that was never written. The prompt takes the newest few under a
//! byte cap, and the file keeps the newest [`MAX_KEPT`] so a year of daily work
//! does not become a log nobody prunes.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::sessions::workspace_key;

/// Longest note that will be accepted.
///
/// Generous for what this is for — "the migration is applied to staging but not
/// production; `schema.sql` is the source of truth" is a hundred bytes — and
/// firm enough that a model cannot file a design document as a note and spend
/// every later conversation's context on it.
pub const MAX_NOTE_BYTES: usize = 2_000;

/// How many notes reach the prompt, newest first.
const MAX_IN_PROMPT: usize = 12;

/// Ceiling on the whole section, whatever the count.
const MAX_SECTION_BYTES: usize = 4 * 1024;

/// How many notes the file keeps. Older ones are dropped when it is next
/// written, oldest first.
pub const MAX_KEPT: usize = 200;

/// One thing worth carrying to the next conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Note {
    /// Stable across reads, so a drawer can delete one note without naming it
    /// by position in a list that a running turn may have appended to.
    ///
    /// Absent in a line somebody typed themselves — these files are meant to be
    /// readable and editable by hand, and dropping such a line for missing a
    /// field they had no reason to know about would make that a lie. [`load`]
    /// fills one in, derived from the note itself rather than generated, so the
    /// id `taurus notes list` prints is the same one `taurus notes forget`
    /// accepts a moment later.
    #[serde(default)]
    pub id: String,
    /// Unix seconds. Formatting is the frontend's business, as it is for a
    /// session's timestamps.
    #[ts(type = "number")]
    pub at: u64,
    /// The conversation that wrote it, so a reader can open the one this came
    /// from, and so a session is not handed its own notes back as though they
    /// came from somewhere else.
    pub session: String,
    pub text: String,
}

/// Where one workspace's notes live.
pub fn memory_dir(workspace: &Path) -> PathBuf {
    crate::config::home_dir()
        .join("memory")
        .join(workspace_key(workspace))
}

fn notes_path(workspace: &Path) -> PathBuf {
    memory_dir(workspace).join("notes.jsonl")
}

/// Every note for this workspace, oldest first.
///
/// A line that will not parse is skipped rather than failing the read. This is
/// a record of work, not a configuration file: one torn line costs the note it
/// held, and refusing to load the rest would cost the lot.
pub fn load(workspace: &Path) -> Vec<Note> {
    let Ok(text) = std::fs::read_to_string(notes_path(workspace)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Note>(line).ok())
        .map(|mut note| {
            if note.id.is_empty() {
                note.id = derived_id(&note);
            }
            note
        })
        .collect()
}

/// An id for a note that arrived without one, from what the note says.
///
/// Derived rather than generated, because a random one would differ between two
/// reads of the same unchanged file — and the two reads that matter are a
/// listing and the command that acts on what the listing showed. FNV-1a for the
/// reason [`crate::sessions::workspace_key`] uses it: the standard hasher's
/// algorithm is unspecified across releases, and this has to survive a
/// toolchain upgrade.
fn derived_id(note: &Note) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in note
        .at
        .to_le_bytes()
        .iter()
        .chain(note.session.as_bytes())
        .chain(note.text.as_bytes())
    {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Adds a note, and returns everything now held.
pub fn append(workspace: &Path, session: &str, text: &str) -> Result<Vec<Note>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("A note needs something in it.".into());
    }
    if text.len() > MAX_NOTE_BYTES {
        return Err(format!(
            "That note is {} bytes and the limit is {MAX_NOTE_BYTES}. A note is a sentence or \
             two about where the work stands, not a document — the detail belongs in the files \
             or in a skill.",
            text.len()
        ));
    }

    let mut notes = load(workspace);
    notes.push(Note {
        id: fresh_id(),
        at: now(),
        session: session.to_string(),
        text: text.to_string(),
    });
    // Oldest first, so what is dropped is what has been superseded by the most
    // work since.
    if notes.len() > MAX_KEPT {
        notes.drain(..notes.len() - MAX_KEPT);
    }

    write(workspace, &notes)?;
    Ok(notes)
}

/// Replaces the whole set, which is how the drawer edits and deletes.
pub fn replace(workspace: &Path, notes: &[Note]) -> Result<(), String> {
    write(workspace, notes)
}

/// Written to a temporary file and renamed over the old one, so a crash or a
/// full disk leaves the previous notes intact rather than a half-written file
/// that loads as a plausible subset of them.
fn write(workspace: &Path, notes: &[Note]) -> Result<(), String> {
    let dir = memory_dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not open {}: {e}", dir.display()))?;

    let mut body = String::new();
    for note in notes {
        let line = serde_json::to_string(note).map_err(|e| e.to_string())?;
        body.push_str(&line);
        body.push('\n');
    }

    let temporary = dir.join("notes.jsonl.tmp");
    std::fs::write(&temporary, body).map_err(|e| format!("could not write a note: {e}"))?;
    std::fs::rename(&temporary, notes_path(workspace))
        .map_err(|e| format!("could not save a note: {e}"))
}

/// The prompt section, or `None` when there is nothing to carry.
///
/// `current` is the session being built for, and its own notes are left out. A
/// conversation that wrote something down this morning still has it in its own
/// transcript; repeating it back under a heading that says it came from an
/// earlier conversation would be the harness telling the model something it
/// knows, in a way that is not quite true.
pub fn section(notes: &[Note], current: &str) -> Option<String> {
    let carried: Vec<&Note> = notes
        .iter()
        .rev()
        .filter(|note| note.session != current)
        .take(MAX_IN_PROMPT)
        .collect();

    if carried.is_empty() {
        return None;
    }

    let mut section = String::from(
        "# Where this workspace was left\n\nNotes an earlier conversation in this workspace \
         wrote down, newest first. They are what was true when they were written, not \
         necessarily what is true now — check before relying on one, and write a new note when \
         you learn something that supersedes it.\n\n",
    );

    for note in carried {
        let line = format!("- {}\n", note.text.replace('\n', " "));
        if section.len() + line.len() > MAX_SECTION_BYTES {
            break;
        }
        section.push_str(&line);
    }

    Some(section)
}

/// Removes one note, and returns everything still held.
///
/// An id that names nothing is not an error. The drawer and a running turn read
/// the same file, so a note can be gone by the time somebody clicks — and the
/// state they wanted is the state they get.
pub fn forget(workspace: &Path, id: &str) -> Result<Vec<Note>, String> {
    let notes: Vec<Note> = load(workspace)
        .into_iter()
        .filter(|note| note.id != id)
        .collect();
    write(workspace, &notes)?;
    Ok(notes)
}

fn fresh_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ------------------------------------------------------------------ the tool

/// The tool a model calls to leave a note for the next conversation.
///
/// Built per turn rather than registered once, because it has to know which
/// conversation is writing — the same reason `ask_user` and `update_plan` are
/// per-turn tools. A sub-agent gets it too: a delegate that works something out
/// is exactly the case where the conclusion would otherwise be summarized into
/// a tool result and lost.
pub struct Remember {
    workspace: PathBuf,
    session: String,
}

pub const REMEMBER_TOOL: &str = "remember";

impl Remember {
    pub fn new(workspace: impl Into<PathBuf>, session: impl Into<String>) -> Self {
        Self {
            workspace: workspace.into(),
            session: session.into(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RememberInput {
    /// The note itself.
    pub note: String,
}

#[async_trait::async_trait]
impl taurus_tools::Tool for Remember {
    fn name(&self) -> &str {
        REMEMBER_TOOL
    }

    fn description(&self) -> &str {
        "Leave a note for the next conversation in this workspace. Reach for it when you learn          something that outlives this conversation and is not written anywhere else: work left          half-done and where it stopped, a decision and the reason for it, a dead end worth not          repeating, a fact about this project that cost you several steps to establish. One or          two sentences, written for someone who was not here. Do not use it for what the files          already say, for a reusable procedure — propose a skill instead — or to log what you          just did in a conversation the user watched."
    }

    fn input_schema(&self) -> serde_json::Value {
        taurus_tools::tool::schema_for::<RememberInput>()
    }

    /// Read, not Write, and the distinction is the workspace rather than the
    /// disk. This writes nothing the user is protecting: the note lands in the
    /// harness's own config home, beside the session transcript and the
    /// checkpoint log, neither of which asks permission to record a turn
    /// either. Gating it would put a dialog in front of the one thing here
    /// meant to be written often, and the note is shown as it happens and
    /// editable afterwards instead.
    fn effect(&self) -> taurus_tools::Effect {
        taurus_tools::Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let note = input.get("note").and_then(|n| n.as_str()).unwrap_or("");
        format!("Remember: {}", first_line(note))
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &taurus_tools::ToolContext,
    ) -> taurus_tools::ToolResult {
        let input: RememberInput = taurus_tools::tool::parse_input(input)?;

        // Rejections come back as tool errors rather than as a silent trim, so
        // a model that wrote too much can write less instead of believing a
        // truncated note was kept whole.
        let notes = append(&self.workspace, &self.session, &input.note)
            .map_err(taurus_tools::ToolError::InvalidInput)?;

        tracing::info!(kept = notes.len(), "note written");
        Ok(format!(
            "Noted. The next conversation in this workspace will be told this before it starts. \
             {} kept for this workspace.",
            crate::memory::count(notes.len())
        )
        .into())
    }
}

/// The first line of a note, shortened, for the row in the transcript.
fn first_line(note: &str) -> String {
    const MAX: usize = 80;
    let line = note.lines().next().unwrap_or("").trim();
    match line.char_indices().nth(MAX) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}

fn count(n: usize) -> String {
    if n == 1 {
        "1 note".into()
    } else {
        format!("{n} notes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::isolated_home;

    fn workspace() -> PathBuf {
        PathBuf::from("/projects/widget")
    }

    fn write(session: &str, text: &str) -> Vec<Note> {
        append(&workspace(), session, text).expect("a valid note")
    }

    #[test]
    fn a_note_survives_to_be_read_back() {
        let _home = isolated_home();
        write("s1", "the auth refactor is half applied");

        let notes = load(&workspace());
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "the auth refactor is half applied");
        assert_eq!(notes[0].session, "s1");
    }

    #[test]
    fn a_workspace_with_nothing_written_reads_as_empty_rather_than_failing() {
        let _home = isolated_home();
        assert!(load(&workspace()).is_empty());
        assert_eq!(section(&[], "s1"), None);
    }

    #[test]
    fn notes_from_two_workspaces_do_not_mix() {
        // The whole point of keying by workspace. A note about one project
        // surfacing in another would be worse than no note at all.
        let _home = isolated_home();
        append(Path::new("/projects/a"), "s1", "about a").unwrap();
        append(Path::new("/projects/b"), "s2", "about b").unwrap();

        let a = load(Path::new("/projects/a"));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].text, "about a");
    }

    #[test]
    fn a_note_with_nothing_in_it_is_refused() {
        let _home = isolated_home();
        assert!(append(&workspace(), "s1", "   ").is_err());
        assert!(load(&workspace()).is_empty());
    }

    #[test]
    fn a_note_past_the_cap_is_refused_rather_than_cut_short() {
        // Truncating would leave a note that ends mid-sentence and reads as
        // fact. Refusing lets the model write a shorter one instead.
        let _home = isolated_home();
        let sprawling = "x".repeat(MAX_NOTE_BYTES + 1);
        let refused = append(&workspace(), "s1", &sprawling).expect_err("too long");

        assert!(refused.contains("sentence or two"), "{refused}");
        assert!(load(&workspace()).is_empty());
    }

    #[test]
    fn the_file_keeps_the_newest_and_drops_the_rest() {
        let _home = isolated_home();
        for i in 0..MAX_KEPT + 5 {
            write("s1", &format!("note {i}"));
        }

        let notes = load(&workspace());
        assert_eq!(notes.len(), MAX_KEPT);
        assert_eq!(notes[0].text, "note 5", "the oldest are what goes");
        assert_eq!(notes[MAX_KEPT - 1].text, format!("note {}", MAX_KEPT + 4));
    }

    #[test]
    fn the_section_reads_newest_first() {
        let _home = isolated_home();
        write("s1", "first thing");
        write("s1", "second thing");

        let section = section(&load(&workspace()), "s2").expect("a section");
        let second = section.find("second thing").expect("the newer note");
        let first = section.find("first thing").expect("the older note");
        assert!(
            second < first,
            "the newest note is the one most worth reading"
        );
    }

    #[test]
    fn a_conversation_is_not_handed_its_own_notes() {
        // It has them in its own transcript already, and repeating them under a
        // heading that says they came from an earlier conversation would be the
        // harness telling the model something untrue about where they are from.
        let _home = isolated_home();
        write("s1", "written by this very conversation");

        assert_eq!(section(&load(&workspace()), "s1"), None);
        assert!(section(&load(&workspace()), "s2").is_some());
    }

    #[test]
    fn the_section_stays_within_its_share_of_the_context() {
        let _home = isolated_home();
        for i in 0..MAX_IN_PROMPT * 4 {
            write("s1", &format!("{i} {}", "padding ".repeat(80)));
        }

        let section = section(&load(&workspace()), "s2").expect("a section");
        assert!(
            section.len() <= MAX_SECTION_BYTES,
            "{} bytes is past the cap",
            section.len()
        );
    }

    #[test]
    fn a_note_spanning_lines_stays_on_one_bullet() {
        // The section is a list. A note with a newline in it would otherwise
        // break out of its bullet and read as prose of the harness's own.
        let _home = isolated_home();
        write("s1", "line one\nline two");

        let section = section(&load(&workspace()), "s2").expect("a section");
        assert!(section.contains("- line one line two"), "{section}");
    }

    #[test]
    fn forgetting_one_note_leaves_the_others() {
        let _home = isolated_home();
        write("s1", "keep this");
        let doomed = write("s1", "drop this")[1].id.clone();

        let left = forget(&workspace(), &doomed).unwrap();

        assert_eq!(left.len(), 1);
        assert_eq!(left[0].text, "keep this");
        assert_eq!(
            load(&workspace()).len(),
            1,
            "and it survives the round trip"
        );
    }

    #[test]
    fn forgetting_a_note_that_is_already_gone_is_not_an_error() {
        // The drawer and a running turn read the same file. A note can be gone
        // by the time somebody clicks, and the state they wanted is the state
        // they get.
        let _home = isolated_home();
        write("s1", "the only one");

        let left = forget(&workspace(), "no-such-note").expect("not an error");
        assert_eq!(left.len(), 1);
    }

    #[test]
    fn a_note_typed_in_by_hand_loads_without_an_id() {
        // These files are meant to be readable and editable. Dropping a line
        // for missing a field a person had no reason to know about would make
        // that a lie.
        let _home = isolated_home();
        let dir = memory_dir(&workspace());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("notes.jsonl"),
            "{\"at\":1,\"session\":\"by hand\",\"text\":\"written by a person\"}\n",
        )
        .unwrap();

        let notes = load(&workspace());
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "written by a person");
        assert!(
            !notes[0].id.is_empty(),
            "it still gets something to name it by"
        );
        assert_eq!(
            notes[0].id,
            load(&workspace())[0].id,
            "and the same one twice — a listing and the command acting on it are two reads"
        );
    }

    #[test]
    fn replacing_is_how_the_drawer_deletes() {
        let _home = isolated_home();
        write("s1", "keep this");
        write("s1", "drop this");

        let kept: Vec<Note> = load(&workspace())
            .into_iter()
            .filter(|n| n.text == "keep this")
            .collect();
        replace(&workspace(), &kept).unwrap();

        let notes = load(&workspace());
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "keep this");
    }

    /// The tool the model actually calls, rather than the store under it.
    mod the_tool {
        use super::*;
        use std::sync::Arc;
        use taurus_tools::{AllowAll, PermissionEngine, Tool, ToolContext};
        use tokio_util::sync::CancellationToken;

        fn ctx() -> (ToolContext, tempfile::TempDir) {
            let dir = tempfile::TempDir::new().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let engine = Arc::new(PermissionEngine::new(
                &root,
                root.join(".taurus"),
                Box::new(AllowAll),
            ));
            (
                ToolContext::new(root, engine, CancellationToken::new()),
                dir,
            )
        }

        #[tokio::test]
        async fn a_call_leaves_a_note_behind() {
            let _home = isolated_home();
            let (context, _dir) = ctx();
            let tool = Remember::new(workspace(), "s1");

            let said = tool
                .execute(
                    serde_json::json!({ "note": "the migration is staged, not live" }),
                    &context,
                )
                .await
                .expect("the note lands");

            assert!(said.to_text().contains("next conversation"), "{said}");
            assert_eq!(
                load(&workspace())[0].text,
                "the migration is staged, not live"
            );
        }

        #[tokio::test]
        async fn a_note_too_long_comes_back_as_an_error_the_model_can_act_on() {
            let _home = isolated_home();
            let (context, _dir) = ctx();
            let tool = Remember::new(workspace(), "s1");

            let refused = tool
                .execute(
                    serde_json::json!({ "note": "x".repeat(MAX_NOTE_BYTES + 1) }),
                    &context,
                )
                .await
                .expect_err("too long to keep");

            // An `InvalidInput` reaches the model as a tool error it can fix by
            // writing less, rather than as a failure it has no lever on.
            assert!(
                matches!(refused, taurus_tools::ToolError::InvalidInput(_)),
                "{refused:?}"
            );
            assert!(load(&workspace()).is_empty());
        }

        #[test]
        fn the_row_in_the_transcript_says_what_is_being_remembered() {
            let tool = Remember::new(workspace(), "s1");
            let preview = tool.preview(&serde_json::json!({ "note": "a short one" }));
            assert_eq!(preview, "Remember: a short one");
        }

        #[test]
        fn a_long_note_is_shortened_for_that_row_rather_than_wrapping_it() {
            let tool = Remember::new(workspace(), "s1");
            let preview = tool.preview(&serde_json::json!({ "note": "y".repeat(200) }));
            assert!(preview.len() < 100, "{preview}");
            assert!(preview.ends_with('…'));
        }

        /// Nothing in the workspace changes, so nothing is gated. See the
        /// comment on `Remember::effect`.
        #[test]
        fn it_asks_no_permission_because_it_touches_nothing_of_the_users() {
            let tool = Remember::new(workspace(), "s1");
            assert_eq!(tool.effect(), taurus_tools::Effect::Read);
        }
    }

    #[test]
    fn a_torn_line_costs_its_own_note_and_no_others() {
        let _home = isolated_home();
        write("s1", "intact");
        let path = notes_path(&workspace());
        let mut body = std::fs::read_to_string(&path).unwrap();
        body.push_str("{\"at\": 1, \"session\": \"s1\", \"te\n");
        std::fs::write(&path, body).unwrap();

        let notes = load(&workspace());
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "intact");
    }
}
