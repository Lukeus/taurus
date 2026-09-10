//! Notes somebody writes, in two scopes.
//!
//! Markdown files in a directory, and deliberately nothing more. The whole of
//! what this module is for is that a note about a project can be written next to
//! the project, read by the conversation about it, and committed with it.
//!
//! # Why these are files in the repository, when memory notes are not
//!
//! [`crate::memory`] argues at length that a note belongs in the config home
//! rather than the workspace, because "a note is prose about the contents of
//! somebody's project, and a file in the project is a file that gets committed".
//! That argument is right about the notes it is about — ones the *model* writes,
//! unreviewed, as a side effect of a conversation — and it inverts here.
//!
//! A note in this notebook is written by a person on purpose. Getting committed
//! is the point of the project-scoped ones: a design note and its diagram are
//! worth more to the next person than to the one who wrote them, and a note that
//! cannot be shared, reviewed, or survive a fresh clone is a scratchpad. So a
//! project note is `.taurus/notes/`, beside the skills, agents and themes a
//! repository already carries, and it shows up in `git status` like anything
//! else somebody wrote.
//!
//! The global scope is the other half of the same thought — a note about how you
//! work rather than about this repository — and lives in `~/.taurus/notes/`.
//! Nothing merges: unlike config, where the workspace layer overrides the global
//! one, two notes with the same name in the two scopes are two notes. Merging
//! prose would mean choosing which paragraph wins.
//!
//! # Why a note's name is its filename
//!
//! Recipes made this decision first: the name is the filename and not a
//! frontmatter field, `deny_unknown_fields` so a stray `name:` errors rather
//! than being quietly ignored. A note has no frontmatter at all, which is the
//! same decision taken further — the file is exactly the Markdown somebody
//! wrote, openable in any editor, with nothing above it that only this app
//! understands.
//!
//! It follows that the name in the list is the name on disk, so it is
//! **validated rather than transformed**. `normalize_name` in the data catalog
//! reasons the other way and is right to: a *model* passes `"User Events"` there,
//! and the useful answer is a dataset called `user_events`. Here a *person* types
//! the name into a box and then looks for it in a directory. Silently renaming
//! `Auth redesign` to `auth_redesign` would be the app editing something the
//! person can see.
//!
//! # Two writers, one rule, already written
//!
//! The person types and the model writes, neither waits for the other, and a
//! save never overwrites something it has not seen. That is
//! [`crate::document`]'s rule and this uses its implementation rather than a
//! second copy — the same length-and-mtime fingerprint, the same refusal
//! carrying what is on disk now, the same line-ending preservation.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config::{self, Scope};
use crate::document;

/// Where notes live inside either config directory.
pub const NOTES_DIR_NAME: &str = "notes";

/// The extension a note has. Only these are listed: the directory is somebody's
/// to put things in, and a stray `.png` beside a note is a file they meant to
/// keep rather than a note that failed to load.
const EXTENSION: &str = "md";

/// Longest a note's name may be.
///
/// A filename, so the ceiling is the filesystem's — 255 bytes on every target,
/// less whatever the extension takes. This is well under that and is about
/// reading rather than storage: a name is a row in a list, and one that needs
/// truncating there is a title pretending to be a name.
pub const MAX_NAME_CHARS: usize = 80;

/// Bytes past which a note is not opened.
///
/// The same limit and the same reason as [`document::MAX_DOCUMENT_BYTES`] — the
/// file crosses the IPC channel as one string — except that here it is also a
/// statement about what a note is. Deliberately much smaller: four megabytes of
/// prose is not a note anybody is going to read, and the editor wraps rather
/// than windowing its paint, so this is the number that keeps that honest.
pub const MAX_NOTE_BYTES: u64 = 512 * 1024;

/// One note, as a list shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PageRef {
    pub scope: Scope,
    /// The filename without its extension, which is what the note is called and
    /// how every other call addresses it.
    pub name: String,
    /// Unix seconds, last written. Formatting is the frontend's business, as it
    /// is for a session's timestamps.
    ///
    /// `number` rather than the `bigint` ts-rs gives a `u64`, matching
    /// [`crate::memory::Note`]: the value crosses as JSON, so it arrives as a
    /// number whatever the type says, and a `bigint` would be a declaration the
    /// runtime does not keep.
    #[ts(type = "number")]
    pub at: u64,
    /// How big it is, so a list can say so without reading every note in the
    /// directory.
    #[ts(type = "number")]
    pub bytes: u64,
}

/// One note, whole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Page {
    pub scope: Scope,
    pub name: String,
    /// The file, whole.
    pub text: String,
    /// Length and mtime, as one opaque string. The frontend's only correct use
    /// of this is to hand it back unchanged — see [`document::Document`].
    pub fingerprint: String,
}

/// What became of a save.
///
/// [`document::Saved`]'s argument, applied to a note: being beaten to the file
/// is not a failure but an ordinary thing that happens when two writers share a
/// directory, and the only useful response is to show both versions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum PageSaved {
    /// Written, with its new fingerprint, so the editor can go on saving
    /// without re-reading.
    Written { page: Page },
    /// Not written: the file changed after the editor last read it. Carries
    /// what is on disk **now**, in the same round trip.
    Stale { current: Page },
}

/// The notes directory for a scope, which need not exist.
///
/// `None` for [`Scope::Workspace`] with no workspace open — which is a real
/// state, not an error: the app starts before a folder is chosen, and the global
/// notes are readable the whole time.
pub fn dir(scope: Scope, workspace: Option<&Path>) -> Option<PathBuf> {
    match scope {
        Scope::Global => Some(config::home_dir().join(NOTES_DIR_NAME)),
        Scope::Workspace => workspace.map(|w| config::workspace_dir(w).join(NOTES_DIR_NAME)),
    }
}

/// Every note in both scopes, newest first within each.
///
/// A directory that is not there yields nothing rather than an error, the same
/// reading every other config directory gets: not having written a note yet is
/// the ordinary state of a fresh workspace.
pub fn list(workspace: Option<&Path>) -> Vec<PageRef> {
    let mut out = Vec::new();
    for scope in [Scope::Workspace, Scope::Global] {
        let Some(dir) = dir(scope, workspace) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<PageRef> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
                    return None;
                }
                let name = path.file_stem()?.to_str()?.to_string();
                let meta = entry.metadata().ok()?;
                if !meta.is_file() {
                    return None;
                }
                Some(PageRef {
                    scope,
                    name,
                    at: seconds(&meta),
                    bytes: meta.len(),
                })
            })
            .collect();
        // Newest first, then by name, so a directory whose files share an mtime
        // — a fresh clone, where git writes them all at once — still comes back
        // in a stable order rather than the filesystem's.
        found.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.name.cmp(&b.name)));
        out.extend(found);
    }
    out
}

/// One note, read off disk.
pub fn read(scope: Scope, workspace: Option<&Path>, name: &str) -> Result<Page, String> {
    let path = file(scope, workspace, name)?;
    let meta = std::fs::metadata(&path)
        .map_err(|_| format!("There is no note called '{name}' any more."))?;
    if meta.len() > MAX_NOTE_BYTES {
        return Err(format!(
            "'{name}' is {} and the editor opens notes up to {}. It is a file rather than a note \
             now — open it in the canvas instead.",
            bytes(meta.len()),
            bytes(MAX_NOTE_BYTES)
        ));
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("Could not read '{name}': {e}"))?;
    Ok(Page {
        scope,
        name: name.to_string(),
        fingerprint: document::fingerprint(&meta),
        text,
    })
}

/// Writes a note, unless it has changed since it was read.
pub fn save(
    scope: Scope,
    workspace: Option<&Path>,
    name: &str,
    text: &str,
    fingerprint: &str,
) -> Result<PageSaved, String> {
    let path = file(scope, workspace, name)?;
    let wrote = document::write_if_current(&path, &format!("'{name}'"), text, fingerprint)?;
    let (stale, written) = match wrote {
        document::Wrote::Stale(w) => (true, w),
        document::Wrote::Written(w) => (false, w),
    };
    let page = Page {
        scope,
        name: name.to_string(),
        text: written.text,
        fingerprint: written.fingerprint,
    };
    Ok(if stale {
        PageSaved::Stale { current: page }
    } else {
        PageSaved::Written { page }
    })
}

/// Starts a note, and refuses rather than picking a different name.
///
/// A name already taken is refused instead of being suffixed, for the reason the
/// data catalog refuses one: `Auth redesign 2` beside `Auth redesign` is a
/// second note nobody asked for, and the person who typed the name knows which
/// of the two they meant.
pub fn create(scope: Scope, workspace: Option<&Path>, name: &str) -> Result<Page, String> {
    let name = check(name)?;
    let path = file(scope, workspace, &name)?;
    if path.exists() {
        return Err(format!(
            "There is already a note called '{name}' in {}. Open it, or pick another name.",
            place(scope)
        ));
    }
    let dir = path
        .parent()
        .ok_or_else(|| "That note has nowhere to go.".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not make {}: {e}", dir.display()))?;

    // A heading rather than an empty file. The name is the filename, so a note
    // that opens with its own title is the one place the two are stated together
    // — and a Markdown file whose first line is a heading previews as something
    // rather than as a blank page.
    let text = format!("# {name}\n\n");
    std::fs::write(&path, &text).map_err(|e| format!("Could not write '{name}': {e}"))?;
    read(scope, workspace, &name)
}

/// Renames a note, keeping its contents.
///
/// The file moves, because the name *is* the filename. Refuses a taken name for
/// the same reason [`create`] does, and refuses a move onto itself with a
/// different case only where the filesystem would treat the two as one file —
/// which is why this checks the destination rather than comparing the strings.
pub fn rename(
    scope: Scope,
    workspace: Option<&Path>,
    name: &str,
    to: &str,
) -> Result<Page, String> {
    let to = check(to)?;
    let from = file(scope, workspace, name)?;
    let onto = file(scope, workspace, &to)?;
    if !from.exists() {
        return Err(format!("There is no note called '{name}' any more."));
    }
    if onto.exists() && onto != from {
        return Err(format!(
            "There is already a note called '{to}' in {}. Pick another name.",
            place(scope)
        ));
    }
    std::fs::rename(&from, &onto)
        .map_err(|e| format!("Could not rename '{name}' to '{to}': {e}"))?;
    read(scope, workspace, &to)
}

/// Deletes a note, and gives back what is left.
///
/// The list comes back in the same call for the reason `forget_note` returns
/// one: the caller's next act is always to show the rest, and a second read
/// could see a directory somebody else has changed in between.
pub fn forget(scope: Scope, workspace: Option<&Path>, name: &str) -> Result<Vec<PageRef>, String> {
    let path = file(scope, workspace, name)?;
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        // Already gone is the outcome that was asked for.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Could not delete '{name}': {e}")),
    }
    Ok(list(workspace))
}

/// Where a note's file is, or why that name cannot name one.
///
/// Every entry point goes through this, so the name a caller passes is checked
/// once and in one place. A name arriving from the frontend has been through
/// [`check`] already when it was created — this is the guard that means a call
/// hand-made against the IPC channel cannot address `../../../.ssh/id_rsa`.
fn file(scope: Scope, workspace: Option<&Path>, name: &str) -> Result<PathBuf, String> {
    let name = check(name)?;
    let dir = dir(scope, workspace).ok_or_else(|| {
        "A project note needs a workspace open. Choose a folder, or write this one in the global \
         notes."
            .to_string()
    })?;
    Ok(dir.join(format!("{name}.{EXTENSION}")))
}

/// Windows keeps these as device names, whatever the extension.
///
/// Checked on every platform rather than behind `cfg(windows)`: a project note
/// is committed, and a repository that will not clone on Windows is a worse
/// outcome than a name refused on macOS.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A name that can be a filename, or why this one cannot.
///
/// Validating rather than transforming — see the module header. Every message
/// names what to do instead, because this one is read by somebody who has just
/// typed into a box.
pub fn check(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("A note needs a name.".to_string());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "That name is {} characters and a note's name can be {MAX_NAME_CHARS}. The rest of it \
             belongs in the note.",
            name.chars().count()
        ));
    }
    if let Some(bad) = name.chars().find(|c| r#"/\:*?"<>|"#.contains(*c)) {
        return Err(format!(
            "A note's name is its filename, so it cannot contain {bad}. Try a hyphen or a space."
        ));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("A note's name cannot contain control characters.".to_string());
    }
    if name.starts_with('.') {
        return Err(
            "A note's name cannot start with a dot, which would hide the file.".to_string(),
        );
    }
    // Windows drops a trailing dot or space silently, so `Notes.` and `Notes`
    // become one file — and the second one to be written wins with no error.
    if name.ends_with('.') {
        return Err("A note's name cannot end with a dot.".to_string());
    }
    if name == "." || name == ".." {
        return Err("That is not a name.".to_string());
    }
    let stem = name.split('.').next().unwrap_or(name);
    if RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        return Err(format!(
            "'{stem}' is a device name on Windows, so a file cannot be called that. Add a word to \
             it."
        ));
    }
    Ok(name.to_string())
}

/// Which directory a message is talking about, in the words the UI uses.
fn place(scope: Scope) -> &'static str {
    match scope {
        Scope::Global => "your global notes",
        Scope::Workspace => "this project",
    }
}

fn seconds(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A size a person reads, for the one message that quotes two of them.
fn bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} bytes");
    }
    if n < 1024 * 1024 {
        return format!("{} KB", n / 1024);
    }
    format!("{} MB", n / (1024 * 1024))
}

/// Reads one of somebody's notes.
///
/// # Why this exists when `read_file` does
///
/// Two reasons, and the second is the one that makes it necessary rather than
/// convenient.
///
/// A note is addressed by a name and a scope, not by a path. The pane says "the
/// project note 'Auth redesign'" through [`crate::onscreen::NoteOnScreen`], and
/// a model that had to turn that into `.taurus/notes/Auth redesign.md` would be
/// reconstructing a layout this module owns — and would get it wrong the first
/// time the extension or the directory changed.
///
/// And a **global note is outside the workspace**. `read_file` resolves through
/// the path guard, which refuses anything above the workspace root, and rightly:
/// that guard is what stops a model reading `~/.ssh`. So without this, half the
/// notebook would be a surface the app tells the model about and gives it no way
/// to read. That is the shape of half-finished feature this codebase refuses
/// elsewhere, and it is why the tool ships with the pane rather than after it.
///
/// It is not a hole in the guard. The only paths reachable are the two notes
/// directories, one file deep, through [`check`] — a name with a separator in it
/// is refused before it becomes a path, and both directories hold nothing but
/// what somebody wrote there as a note.
pub struct ReadNote {
    workspace: PathBuf,
}

impl ReadNote {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
        }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ReadNoteInput {
    /// Which notebook: `workspace` for a note kept with this project, `global`
    /// for one the person keeps across all of them. The notes pane says which
    /// when it says what is on screen.
    pub scope: Scope,
    /// The note's name, spelled exactly as it was given — this is its filename
    /// without the extension, so it is case-sensitive and may contain spaces.
    pub name: String,
}

#[async_trait::async_trait]
impl taurus_tools::Tool for ReadNote {
    fn name(&self) -> &str {
        READ_NOTE_TOOL
    }

    fn description(&self) -> &str {
        "Read one of the user's notes. Notes are Markdown files the user writes themselves, in two \
         notebooks: `workspace` notes live with the project and are committed with it, `global` \
         notes follow the user between projects. Use this when a message refers to \"my note\", or \
         when the notes pane says which note is on screen. A note may contain mermaid diagrams, \
         which are drawn for the user but reach you as their source. Do not guess a note's path \
         and read it with read_file: a global note is outside the workspace and read_file cannot \
         reach it."
    }

    fn input_schema(&self) -> serde_json::Value {
        taurus_tools::tool::schema_for::<ReadNoteInput>()
    }

    fn effect(&self) -> taurus_tools::Effect {
        taurus_tools::Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Read note: {}",
            input.get("name").and_then(|n| n.as_str()).unwrap_or("?")
        )
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &taurus_tools::ToolContext,
    ) -> taurus_tools::ToolResult {
        let input: ReadNoteInput = taurus_tools::tool::parse_input(input)?;
        // The error this gives already names the note and says what to do, so it
        // is passed through rather than wrapped in a second sentence about it.
        let page = read(input.scope, Some(&self.workspace), &input.name)
            .map_err(taurus_tools::ToolError::InvalidInput)?;
        if page.text.trim().is_empty() {
            // Distinguished from a note that is missing, which `read` already
            // reports: an empty answer and a nonexistent note are different
            // facts, and a model told neither will invent the contents.
            return Ok(format!("The note '{}' is empty.", page.name).into());
        }
        Ok(page.text.into())
    }
}

/// Named as a constant because the host registers it by name and a literal in
/// two files can drift apart.
pub const READ_NOTE_TOOL: &str = "read_note";

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A workspace and an isolated `~/.taurus`, so a global note written here
    /// cannot reach the real one. The guard is held for the whole body — see
    /// [`crate::testing`] for why that is not optional.
    fn isolated<T>(body: impl FnOnce(&Path) -> T) -> T {
        let _home = crate::testing::isolated_home();
        let workspace = TempDir::new().unwrap();
        body(workspace.path())
    }

    fn page(scope: Scope, w: &Path, name: &str) -> Page {
        create(scope, Some(w), name).unwrap()
    }

    /* ------------------------------------------------------------ naming */

    #[test]
    fn a_name_is_kept_as_typed() {
        // The data catalog normalizes because a model passes the name there. A
        // person types this one and then looks for it in a directory.
        assert_eq!(check("Auth redesign").unwrap(), "Auth redesign");
        assert_eq!(check("  Auth redesign  ").unwrap(), "Auth redesign");
    }

    #[test]
    fn a_name_that_cannot_be_a_filename_is_refused_with_what_to_do() {
        for bad in [
            "",
            "   ",
            "a/b",
            "a\\b",
            "a:b",
            "a*b",
            "a?b",
            ".hidden",
            "trailing.",
        ] {
            let why = check(bad).unwrap_err();
            assert!(!why.is_empty(), "{bad} was allowed");
        }
        assert!(check("a/b").unwrap_err().contains("hyphen"));
        assert!(check(&"x".repeat(MAX_NAME_CHARS + 1))
            .unwrap_err()
            .contains("81"));
    }

    /// Checked on every platform, because a project note gets committed and a
    /// repository that will not clone on Windows is the worse outcome.
    #[test]
    fn a_windows_device_name_is_refused_everywhere() {
        assert!(check("NUL").unwrap_err().contains("device name"));
        assert!(check("con").unwrap_err().contains("device name"));
        assert!(check("com1.notes").unwrap_err().contains("device name"));
        assert!(check("console").is_ok());
    }

    #[test]
    fn a_traversal_cannot_address_a_file_outside_the_notes_directory() {
        isolated(|w| {
            assert!(read(Scope::Workspace, Some(w), "../../secrets").is_err());
            assert!(save(Scope::Workspace, Some(w), "../x", "hi", "0-0").is_err());
            assert!(forget(Scope::Workspace, Some(w), "..").is_err());
        });
    }

    /* ---------------------------------------------------------- the pair */

    #[test]
    fn the_two_scopes_are_two_directories_and_do_not_merge() {
        isolated(|w| {
            page(Scope::Workspace, w, "Notes");
            page(Scope::Global, w, "Notes");
            let all = list(Some(w));
            assert_eq!(all.len(), 2, "{all:?}");
            assert_eq!(all[0].scope, Scope::Workspace);
            assert_eq!(all[1].scope, Scope::Global);
        });
    }

    #[test]
    fn a_project_note_lands_in_the_workspace_where_it_can_be_committed() {
        isolated(|w| {
            page(Scope::Workspace, w, "Auth redesign");
            assert!(w.join(".taurus/notes/Auth redesign.md").is_file());
        });
    }

    #[test]
    fn a_global_note_can_be_written_with_no_workspace_open() {
        isolated(|_| {
            assert!(create(Scope::Global, None, "Reading list").is_ok());
            assert_eq!(list(None).len(), 1);
        });
    }

    #[test]
    fn a_project_note_with_no_workspace_says_what_to_do() {
        isolated(|_| {
            let why = create(Scope::Workspace, None, "Nowhere").unwrap_err();
            assert!(why.contains("workspace"), "{why}");
            assert!(why.contains("global"), "{why}");
        });
    }

    #[test]
    fn a_missing_notes_directory_is_no_notes_rather_than_an_error() {
        isolated(|w| assert_eq!(list(Some(w)), Vec::new()));
    }

    #[test]
    fn only_markdown_files_are_notes() {
        isolated(|w| {
            page(Scope::Workspace, w, "Real");
            let dir = dir(Scope::Workspace, Some(w)).unwrap();
            std::fs::write(dir.join("sketch.png"), b"\x89PNG").unwrap();
            std::fs::create_dir(dir.join("archive")).unwrap();
            assert_eq!(
                list(Some(w)).iter().map(|p| &p.name).collect::<Vec<_>>(),
                vec!["Real"]
            );
        });
    }

    /* --------------------------------------------------------- the notes */

    #[test]
    fn a_new_note_opens_with_its_own_title() {
        isolated(|w| {
            assert_eq!(
                page(Scope::Workspace, w, "Auth redesign").text,
                "# Auth redesign\n\n"
            );
        });
    }

    #[test]
    fn a_name_already_taken_is_refused_rather_than_suffixed() {
        isolated(|w| {
            page(Scope::Workspace, w, "Notes");
            let why = create(Scope::Workspace, Some(w), "Notes").unwrap_err();
            assert!(why.contains("already a note"), "{why}");
            assert!(why.contains("this project"), "{why}");
            // And the same name in the other scope is a different note.
            assert!(create(Scope::Global, Some(w), "Notes").is_ok());
        });
    }

    #[test]
    fn renaming_moves_the_file_and_keeps_the_text() {
        isolated(|w| {
            let made = page(Scope::Workspace, w, "Draft");
            save(
                Scope::Workspace,
                Some(w),
                "Draft",
                "# Draft\n\nbody\n",
                &made.fingerprint,
            )
            .unwrap();
            let moved = rename(Scope::Workspace, Some(w), "Draft", "Final").unwrap();
            assert_eq!(moved.name, "Final");
            assert_eq!(moved.text, "# Draft\n\nbody\n");
            assert!(!w.join(".taurus/notes/Draft.md").exists());
            assert!(w.join(".taurus/notes/Final.md").is_file());
        });
    }

    #[test]
    fn renaming_onto_a_taken_name_is_refused() {
        isolated(|w| {
            page(Scope::Workspace, w, "One");
            page(Scope::Workspace, w, "Two");
            assert!(rename(Scope::Workspace, Some(w), "One", "Two").is_err());
            // Onto itself is not a collision, so a case-only change is allowed
            // wherever the filesystem can tell the two apart.
            assert!(rename(Scope::Workspace, Some(w), "One", "One").is_ok());
        });
    }

    #[test]
    fn forgetting_a_note_that_is_already_gone_is_the_outcome_that_was_asked_for() {
        isolated(|w| {
            page(Scope::Workspace, w, "Notes");
            assert_eq!(forget(Scope::Workspace, Some(w), "Notes").unwrap().len(), 0);
            assert!(forget(Scope::Workspace, Some(w), "Notes").is_ok());
        });
    }

    /* -------------------------------------------------------- two writers */

    #[test]
    fn a_save_that_has_seen_the_file_is_written() {
        isolated(|w| {
            let made = page(Scope::Workspace, w, "Notes");
            let saved = save(
                Scope::Workspace,
                Some(w),
                "Notes",
                "# Notes\n\nnew\n",
                &made.fingerprint,
            )
            .unwrap();
            match saved {
                PageSaved::Written { page } => {
                    assert_eq!(page.text, "# Notes\n\nnew\n");
                    assert_ne!(page.fingerprint, made.fingerprint);
                }
                PageSaved::Stale { .. } => panic!("refused a save it should have taken"),
            }
        });
    }

    /// The case the rule exists for: somebody else wrote the file while it was
    /// open.
    #[test]
    fn a_save_that_has_not_seen_the_file_comes_back_with_what_is_there_now() {
        isolated(|w| {
            page(Scope::Workspace, w, "Notes");
            std::fs::write(
                w.join(".taurus/notes/Notes.md"),
                "# Notes\n\nsomebody else\n",
            )
            .unwrap();
            let saved = save(Scope::Workspace, Some(w), "Notes", "mine\n", "0-0").unwrap();
            match saved {
                PageSaved::Stale { current } => {
                    assert_eq!(current.text, "# Notes\n\nsomebody else\n");
                }
                PageSaved::Written { .. } => panic!("overwrote something it had not seen"),
            }
        });
    }

    #[test]
    fn a_note_too_big_to_open_says_what_to_do_instead() {
        isolated(|w| {
            page(Scope::Workspace, w, "Huge");
            let path = w.join(".taurus/notes/Huge.md");
            std::fs::write(&path, "x".repeat(MAX_NOTE_BYTES as usize + 1)).unwrap();
            let why = read(Scope::Workspace, Some(w), "Huge").unwrap_err();
            assert!(why.contains("512 KB"), "{why}");
            assert!(why.contains("canvas"), "{why}");
        });
    }

    /* ------------------------------------------------------------- the tool */

    /// The tool ignores its context — it is `Effect::Read` and asks nothing —
    /// so this builds the cheapest one that exists.
    fn context(w: &Path) -> taurus_tools::ToolContext {
        use std::sync::Arc;
        use taurus_tools::{AllowAll, PermissionEngine, ToolContext};
        let engine = Arc::new(PermissionEngine::new(
            w,
            w.join(".taurus"),
            Box::new(AllowAll),
        ));
        ToolContext::new(
            w.to_path_buf(),
            engine,
            tokio_util::sync::CancellationToken::new(),
        )
    }

    async fn read_tool(w: &Path, scope: Scope, name: &str) -> Result<String, String> {
        use taurus_tools::Tool;
        let tool = ReadNote::new(w);
        let input = serde_json::json!({ "scope": scope, "name": name });
        tool.execute(input, &context(w))
            .await
            .map(|out| out.as_text().unwrap_or_default().to_string())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn the_tool_reads_a_note_by_name_and_scope() {
        isolated(|w| {
            let made = page(Scope::Workspace, w, "Auth redesign");
            save(
                Scope::Workspace,
                Some(w),
                "Auth redesign",
                "# Auth redesign\n\nUse a refresh token.\n",
                &made.fingerprint,
            )
            .unwrap();
            let text = block(read_tool(w, Scope::Workspace, "Auth redesign")).unwrap();
            assert!(text.contains("Use a refresh token."), "{text}");
        });
    }

    /// The reason the tool exists rather than leaving this to `read_file`: a
    /// global note is above the workspace root, where the path guard refuses to
    /// go — and rightly, since that guard is what stops a model reading `~/.ssh`.
    #[test]
    fn the_tool_reaches_a_global_note_that_read_file_cannot() {
        isolated(|w| {
            let made = create(Scope::Global, Some(w), "Reading list").unwrap();
            save(
                Scope::Global,
                Some(w),
                "Reading list",
                "# Reading list\n\nSICP.\n",
                &made.fingerprint,
            )
            .unwrap();
            assert!(block(read_tool(w, Scope::Global, "Reading list"))
                .unwrap()
                .contains("SICP"));
        });
    }

    #[test]
    fn the_tool_cannot_be_talked_out_of_the_notes_directory() {
        isolated(|w| {
            std::fs::write(w.join("secret.md"), "not a note").unwrap();
            for name in ["../secret", "../../secret", "/etc/passwd", "sub/secret"] {
                assert!(
                    block(read_tool(w, Scope::Workspace, name)).is_err(),
                    "{name} was readable"
                );
            }
        });
    }

    /// Two different facts, and a model told neither will invent the contents.
    #[test]
    fn the_tool_tells_an_empty_note_from_a_missing_one() {
        isolated(|w| {
            let made = page(Scope::Workspace, w, "Blank");
            save(Scope::Workspace, Some(w), "Blank", "", &made.fingerprint).unwrap();
            assert!(block(read_tool(w, Scope::Workspace, "Blank"))
                .unwrap()
                .contains("is empty"));
            assert!(block(read_tool(w, Scope::Workspace, "Nothing"))
                .unwrap_err()
                .contains("no note called 'Nothing'"));
        });
    }

    /// One runtime per test rather than a `#[tokio::test]` on each: the isolation
    /// guard is not `Send`, so the body has to stay on this thread.
    fn block<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// A note listed but deleted underneath is the ordinary consequence of the
    /// directory being somebody's to edit.
    #[test]
    fn reading_a_note_that_is_gone_says_so() {
        isolated(|w| {
            let why = read(Scope::Workspace, Some(w), "Ghost").unwrap_err();
            assert!(why.contains("no note called 'Ghost'"), "{why}");
        });
    }
}
