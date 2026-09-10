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

/// Which of the two things a notebook holds.
///
/// A note is Markdown and a sketch is an Excalidraw drawing. They share a
/// directory and every rule in this file — the name check, the two scopes, the
/// compare-and-swap — because a sketch is the other half of the same act:
/// somebody writing something down, where a drawing is sometimes the better way
/// to write it. Each kind has a namespace of its own, so `Auth.md` and
/// `Auth.excalidraw` are a note and the sketch it embeds rather than a
/// collision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PageKind {
    Note,
    Sketch,
}

impl PageKind {
    /// The extension a file of this kind has.
    ///
    /// Only these two are listed: the directory is somebody's to put things in,
    /// and a stray `.png` beside a note is a file they meant to keep rather than
    /// a note that failed to load.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Note => "md",
            Self::Sketch => "excalidraw",
        }
    }

    /// What a message calls one.
    pub fn word(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Sketch => "sketch",
        }
    }

    fn max_bytes(self) -> u64 {
        match self {
            Self::Note => MAX_NOTE_BYTES,
            Self::Sketch => MAX_SKETCH_BYTES,
        }
    }

    fn of(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "md" => Some(Self::Note),
            "excalidraw" => Some(Self::Sketch),
            _ => None,
        }
    }
}

/// What a new sketch holds: nothing drawn, in the shape Excalidraw writes.
///
/// Written out rather than left empty, because an empty file is not an
/// Excalidraw drawing, and the editor refuses to open what it cannot read — the
/// rule that stops a blank canvas saving itself over a file it failed to parse.
const EMPTY_SKETCH: &str = "{\n  \"type\": \"excalidraw\",\n  \"version\": 2,\n  \"source\": \"taurus\",\n  \"elements\": [],\n  \"appState\": {},\n  \"files\": {}\n}\n";

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

/// Bytes past which a sketch is not opened.
///
/// Sixteen times a note's, and the reason is pictures: Excalidraw keeps an image
/// pasted into a drawing *inside* the file, as a data URL, so one screenshot is
/// most of a sketch's size. Past this the file is mostly pictures, and it
/// crosses the IPC channel as one string on every open and every save.
pub const MAX_SKETCH_BYTES: u64 = 8 * 1024 * 1024;

/// One note, as a list shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PageRef {
    pub scope: Scope,
    pub kind: PageKind,
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
    pub kind: PageKind,
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

/// Every note and sketch in both scopes, newest first within each.
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
                let kind = PageKind::of(&path)?;
                let name = path.file_stem()?.to_str()?.to_string();
                let meta = entry.metadata().ok()?;
                if !meta.is_file() {
                    return None;
                }
                Some(PageRef {
                    scope,
                    kind,
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

/// One note or sketch, read off disk.
pub fn read(
    scope: Scope,
    workspace: Option<&Path>,
    kind: PageKind,
    name: &str,
) -> Result<Page, String> {
    let path = file(scope, workspace, kind, name)?;
    let word = kind.word();
    let meta = std::fs::metadata(&path)
        .map_err(|_| format!("There is no {word} called '{name}' any more."))?;
    if meta.len() > kind.max_bytes() {
        return Err(too_big(kind, name, meta.len()));
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("Could not read '{name}': {e}"))?;
    Ok(Page {
        scope,
        kind,
        name: name.to_string(),
        fingerprint: document::fingerprint(&meta),
        text,
    })
}

/// Why a file is too big to open or to save, in the words for its kind.
///
/// The two kinds are too big for different reasons, and the message says the
/// one that is true: prose that long has outgrown being a note, while a sketch
/// that size is nearly always pictures pasted into it.
fn too_big(kind: PageKind, name: &str, size: u64) -> String {
    match kind {
        PageKind::Note => format!(
            "'{name}' is {} and the editor opens notes up to {}. It is a file rather than a note \
             now — open it in the canvas instead.",
            bytes(size),
            bytes(MAX_NOTE_BYTES)
        ),
        PageKind::Sketch => format!(
            "The sketch '{name}' is {} and sketches open up to {}. A sketch that size is nearly \
             always pictures pasted into it — Excalidraw keeps each one inside the file — so \
             removing one or two is what brings it back.",
            bytes(size),
            bytes(MAX_SKETCH_BYTES)
        ),
    }
}

/// Writes a note or sketch, unless it has changed since it was read.
///
/// Refuses what it would then refuse to open. A save that went through at any
/// size would leave a file behind that the next read turns away — the typing
/// kept, and the note lost to the person who typed it.
pub fn save(
    scope: Scope,
    workspace: Option<&Path>,
    kind: PageKind,
    name: &str,
    text: &str,
    fingerprint: &str,
) -> Result<PageSaved, String> {
    let path = file(scope, workspace, kind, name)?;
    if text.len() as u64 > kind.max_bytes() {
        return Err(format!(
            "Not saved. {}",
            too_big(kind, name, text.len() as u64)
        ));
    }
    let wrote = document::write_if_current(&path, &format!("'{name}'"), text, fingerprint)?;
    let (stale, written) = match wrote {
        document::Wrote::Stale(w) => (true, w),
        document::Wrote::Written(w) => (false, w),
    };
    let page = Page {
        scope,
        kind,
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
pub fn create(
    scope: Scope,
    workspace: Option<&Path>,
    kind: PageKind,
    name: &str,
) -> Result<Page, String> {
    let name = check(name)?;
    let word = kind.word();
    let path = file(scope, workspace, kind, &name)?;
    if path.exists() {
        return Err(format!(
            "There is already a {word} called '{name}' in {}. Open it, or pick another name.",
            place(scope)
        ));
    }
    let dir = path
        .parent()
        .ok_or_else(|| format!("That {word} has nowhere to go."))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not make {}: {e}", dir.display()))?;

    // A heading rather than an empty file. The name is the filename, so a note
    // that opens with its own title is the one place the two are stated together
    // — and a Markdown file whose first line is a heading previews as something
    // rather than as a blank page.
    let text = match kind {
        PageKind::Note => format!("# {name}\n\n"),
        PageKind::Sketch => EMPTY_SKETCH.to_string(),
    };
    std::fs::write(&path, &text).map_err(|e| format!("Could not write '{name}': {e}"))?;
    read(scope, workspace, kind, &name)
}

/// Renames a note, keeping its contents.
///
/// The file moves, because the name *is* the filename. Refuses a taken name for
/// the same reason [`create`] does, and refuses a move onto itself with a
/// different case only where the filesystem would treat the two as one file —
/// which is why this checks the destination rather than comparing the strings.
///
/// A sketch that is renamed is not renamed inside the notes that embed it. The
/// app editing prose somebody else wrote, to keep a link pointing where it did,
/// is the kind of help nobody asked for — so the embed says the sketch is not
/// there, and the fix is one line in the note.
pub fn rename(
    scope: Scope,
    workspace: Option<&Path>,
    kind: PageKind,
    name: &str,
    to: &str,
) -> Result<Page, String> {
    let to = check(to)?;
    let word = kind.word();
    let from = file(scope, workspace, kind, name)?;
    let onto = file(scope, workspace, kind, &to)?;
    if !from.exists() {
        return Err(format!("There is no {word} called '{name}' any more."));
    }
    if onto.exists() && onto != from {
        return Err(format!(
            "There is already a {word} called '{to}' in {}. Pick another name.",
            place(scope)
        ));
    }
    std::fs::rename(&from, &onto)
        .map_err(|e| format!("Could not rename '{name}' to '{to}': {e}"))?;
    read(scope, workspace, kind, &to)
}

/// Deletes a note, and gives back what is left.
///
/// The list comes back in the same call for the reason `forget_note` returns
/// one: the caller's next act is always to show the rest, and a second read
/// could see a directory somebody else has changed in between.
pub fn forget(
    scope: Scope,
    workspace: Option<&Path>,
    kind: PageKind,
    name: &str,
) -> Result<Vec<PageRef>, String> {
    let path = file(scope, workspace, kind, name)?;
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
fn file(
    scope: Scope,
    workspace: Option<&Path>,
    kind: PageKind,
    name: &str,
) -> Result<PathBuf, String> {
    let name = check(name)?;
    let dir = dir(scope, workspace).ok_or_else(|| {
        format!(
            "A project {} needs a workspace open. Choose a folder, or keep this one in the \
             global notes.",
            kind.word()
        )
    })?;
    Ok(dir.join(format!("{name}.{}", kind.extension())))
}

/// Where a note's file is, as a person would write it.
///
/// Workspace-relative for a project note, which is how every path the user sees
/// is written and how `files_changed` reports it. Home-relative for a global
/// one, since it is not in the workspace at all.
fn shown(scope: Scope, kind: PageKind, name: &str) -> String {
    let file = format!("{NOTES_DIR_NAME}/{name}.{}", kind.extension());
    match scope {
        Scope::Workspace => format!("{}/{file}", config::WORKSPACE_DIR_NAME),
        Scope::Global => format!("~/{}/{file}", config::HOME_DIR_NAME),
    }
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
        let page = read(
            input.scope,
            Some(&self.workspace),
            PageKind::Note,
            &input.name,
        )
        .map_err(taurus_tools::ToolError::InvalidInput)?;
        if page.text.trim().is_empty() {
            // Distinguished from a note that is missing, which `read` already
            // reports: an empty answer and a nonexistent note are different
            // facts, and a model told neither will invent the contents.
            return Ok(format!("The note '{}' is empty.", page.name).into());
        }
        let mut out = page.text;
        let about = sketches_in(input.scope, Some(&self.workspace), &out);
        if !about.is_empty() {
            out.push_str("\n\n---\n");
            out.push_str(&about);
        }
        Ok(out.into())
    }
}

/// Named as a constant because the host registers it by name and a literal in
/// two files can drift apart.
pub const READ_NOTE_TOOL: &str = "read_note";
pub const WRITE_NOTE_TOOL: &str = "write_note";
pub const OPEN_NOTE_TOOL: &str = "open_note";

/// What the model is told about the sketches a note embeds.
///
/// A sketch is drawn for the person and cannot be drawn for the model — there is
/// no picture of it anywhere, only the scene Excalidraw keeps. But the scene
/// carries every piece of text written on the drawing, and "the boxes are
/// labelled Client, Gateway, Token store" is most of what a question about a
/// system sketch needs. So that much is said, and the rest is named as unseen
/// rather than left for the model to guess at.
fn sketches_in(scope: Scope, workspace: Option<&Path>, note: &str) -> String {
    let mut out = String::new();
    for name in embeds(note) {
        let line = match read(scope, workspace, PageKind::Sketch, &name) {
            Ok(sketch) => {
                let labels = sketch_text(&sketch.text);
                if labels.is_empty() {
                    format!(
                        "The sketch '{name}' is embedded above. It is drawn for the user and you \
                         cannot see it, and it has no text written on it."
                    )
                } else {
                    format!(
                        "The sketch '{name}' is embedded above. It is drawn for the user and you \
                         cannot see it; the text written on it is: {}.",
                        labels.join(" · ")
                    )
                }
            }
            Err(_) => format!(
                "The note embeds a sketch called '{name}' that is not in this notebook, so the \
                 user sees a placeholder there."
            ),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// The sketches a note embeds, by name, in the order it embeds them.
///
/// An embed is a Markdown image whose destination is a `.excalidraw` file beside
/// the note — `![Auth flow](<Auth flow.excalidraw>)`, with the angle brackets a
/// name with a space in it needs, or `![x](flow.excalidraw)` without. A
/// destination with a separator in it points outside this notebook and is not
/// one of these.
///
/// A scanner rather than a Markdown parser, for what it costs: this runs once
/// per `read_note`, looks for one shape, and a note that fools it gets a
/// sentence fewer, never a wrong one.
pub fn embeds(note: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = note;
    while let Some(at) = rest.find("](") {
        rest = &rest[at + 2..];
        let (target, after) = if let Some(inner) = rest.strip_prefix('<') {
            match inner.find('>') {
                Some(end) => (&inner[..end], &inner[end + 1..]),
                None => break,
            }
        } else {
            let end = rest
                .find(|c: char| c == ')' || c.is_whitespace())
                .unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };
        rest = after;
        let target = target.replace("%20", " ");
        let target = target.strip_prefix("./").unwrap_or(&target);
        let Some(name) = target.strip_suffix(".excalidraw") else {
            continue;
        };
        if name.is_empty() || name.contains(['/', '\\']) {
            continue;
        }
        if !found.iter().any(|f| f == name) {
            found.push(name.to_string());
        }
    }
    found
}

/// The text written on a sketch, in the order it was drawn.
///
/// Every live text element's words, whitespace folded, repeats dropped, and cut
/// at a length that keeps one sketch from costing more context than the note it
/// sits in. A scene that is not the shape expected yields nothing rather than an
/// error: the sketch is still there for the person, and "no text" is the most
/// that can honestly be said about one this cannot read.
pub fn sketch_text(scene: &str) -> Vec<String> {
    const MAX_LABELS: usize = 60;
    let Ok(value) = serde_json::from_str::<serde_json::Value>(scene) else {
        return Vec::new();
    };
    let Some(elements) = value.get("elements").and_then(|e| e.as_array()) else {
        return Vec::new();
    };
    let mut labels: Vec<String> = Vec::new();
    for element in elements {
        if element.get("type").and_then(|t| t.as_str()) != Some("text") {
            continue;
        }
        if element.get("isDeleted").and_then(|d| d.as_bool()) == Some(true) {
            continue;
        }
        let Some(text) = element.get("text").and_then(|t| t.as_str()) else {
            continue;
        };
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() && !labels.contains(&text) {
            labels.push(text);
        }
        if labels.len() == MAX_LABELS {
            break;
        }
    }
    labels
}

/// Writes one of somebody's notes.
///
/// # Why this exists when `write_file` does
///
/// For a project note it is a convenience with one real advantage — the model
/// names a note the way the pane does rather than reconstructing a directory
/// layout this module owns. For a global note it is the only way: the note is
/// above the workspace root, and `write_file` resolves through the path guard,
/// which will not go there.
///
/// Its guarantees are the ones a write has everywhere else. It is `Write`, so it
/// asks first, with the diff of what it would change. A project note is
/// declared in [`touches`](taurus_tools::Tool::touches), so the checkpoint
/// recorder takes a copy first and rewinding the turn puts the note back — and
/// `files_changed` names it, which is how an open editor learns to reload. A
/// global note is outside every workspace, so neither applies to it; the diff in
/// the prompt is the whole of its safety, and the docs say so.
///
/// Sketches are not written. A model can write an Excalidraw scene, but not one
/// worth looking at, and the notebook's diagram language for a model is a
/// Mermaid fence in a note — which draws, and which it writes well.
pub struct WriteNote {
    workspace: PathBuf,
}

impl WriteNote {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
        }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WriteNoteInput {
    /// Which notebook: `workspace` for a note kept with this project, `global`
    /// for one the person keeps across all of them.
    pub scope: Scope,
    /// The note's name — its filename without the extension. A name that does
    /// not exist yet makes a new note.
    pub name: String,
    /// The whole note, as Markdown. It replaces what is there, so read the note
    /// with read_note first and send it back with your change in it.
    pub text: String,
}

#[async_trait::async_trait]
impl taurus_tools::Tool for WriteNote {
    fn name(&self) -> &str {
        WRITE_NOTE_TOOL
    }

    fn description(&self) -> &str {
        "Write one of the user's notes, creating it if the name is new. The text replaces the \
         whole note, so read it with read_note first and send it back with your change in it. \
         Use it when the user asks you to write something down, add to a note, or keep a record \
         in their notes. A mermaid code block in the note is drawn as a diagram for the user. \
         Only notes: sketches are drawn by the user."
    }

    fn input_schema(&self) -> serde_json::Value {
        taurus_tools::tool::schema_for::<WriteNoteInput>()
    }

    fn effect(&self) -> taurus_tools::Effect {
        taurus_tools::Effect::Write
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let name = input.get("name").and_then(|n| n.as_str()).unwrap_or("?");
        let which = match input.get("scope").and_then(|s| s.as_str()) {
            Some("global") => "global",
            _ => "project",
        };
        let size = input
            .get("text")
            .and_then(|t| t.as_str())
            .map_or(0, str::len);
        format!("Write the {which} note '{name}' ({size} bytes)")
    }

    /// A project note only. That is what lets a rewind put it back, and what
    /// makes `files_changed` name it so an open editor reloads.
    fn touches(&self, input: &serde_json::Value) -> Vec<String> {
        let Ok(input) = serde_json::from_value::<WriteNoteInput>(input.clone()) else {
            return Vec::new();
        };
        match (input.scope, check(&input.name)) {
            (Scope::Workspace, Ok(name)) => vec![shown(Scope::Workspace, PageKind::Note, &name)],
            _ => Vec::new(),
        }
    }

    async fn diff(
        &self,
        input: &serde_json::Value,
        _workspace: &Path,
    ) -> Option<taurus_tools::FileDiff> {
        let input: WriteNoteInput = serde_json::from_value(input.clone()).ok()?;
        let path = file(
            input.scope,
            Some(&self.workspace),
            PageKind::Note,
            &input.name,
        )
        .ok()?;
        let before = std::fs::read_to_string(&path).ok();
        Some(taurus_tools::diff::of_change(
            shown(input.scope, PageKind::Note, &input.name),
            before.as_deref(),
            Some(&input.text),
        ))
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &taurus_tools::ToolContext,
    ) -> taurus_tools::ToolResult {
        use taurus_tools::ToolError;
        let input: WriteNoteInput = taurus_tools::tool::parse_input(input)?;
        let name = check(&input.name).map_err(ToolError::InvalidInput)?;
        if input.text.len() as u64 > MAX_NOTE_BYTES {
            return Err(ToolError::InvalidInput(too_big(
                PageKind::Note,
                &name,
                input.text.len() as u64,
            )));
        }
        let path = file(input.scope, Some(&self.workspace), PageKind::Note, &name)
            .map_err(ToolError::InvalidInput)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| ToolError::Failed(format!("could not make {}: {e}", dir.display())))?;
        }
        // The file's own line endings, the rule `write_file` and the editor's
        // save both keep — a note written on Windows stays a CRLF note.
        let existing = std::fs::read_to_string(&path).ok();
        let created = existing.is_none();
        let text = match existing.as_deref() {
            Some(prior) if prior.contains("\r\n") => {
                taurus_tools::builtin::fs::to_crlf(&input.text)
            }
            _ => input.text,
        };
        std::fs::write(&path, &text)
            .map_err(|e| ToolError::Failed(format!("could not write the note '{name}': {e}")))?;
        let which = match input.scope {
            Scope::Workspace => "project",
            Scope::Global => "global",
        };
        let did = if created { "Created" } else { "Wrote" };
        Ok(format!(
            "{did} the {which} note '{name}' ({} bytes). It is in the notes pane if the user \
             wants to look; do not repeat it.",
            text.len()
        )
        .into())
    }
}

/// Puts a note in front of the person, as a card they can open.
///
/// `open_file`'s argument, carried across to the notebook: the card holds the
/// notebook and the name and never the note, so a conversation reopened next
/// month opens next month's note. Unlike the canvas it does not open anything
/// by itself. The canvas is a split and appears beside the conversation; the
/// notes pane *replaces* it, and a turn that swapped the screen out from under
/// somebody reading its answer would be the app deciding where they look.
pub struct OpenNote {
    workspace: PathBuf,
}

impl OpenNote {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
        }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct OpenNoteInput {
    /// Which notebook the note is in.
    pub scope: Scope,
    /// The note's name, spelled exactly as it was given.
    pub name: String,
}

#[async_trait::async_trait]
impl taurus_tools::Tool for OpenNote {
    fn name(&self) -> &str {
        OPEN_NOTE_TOOL
    }

    fn description(&self) -> &str {
        "Put one of the user's notes in the conversation as a card they can open. Use it after \
         writing a note, or when the answer to a question is in a note they should look at. It \
         does not give you the note's contents — use read_note for that."
    }

    fn input_schema(&self) -> serde_json::Value {
        taurus_tools::tool::schema_for::<OpenNoteInput>()
    }

    fn effect(&self) -> taurus_tools::Effect {
        taurus_tools::Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Open note: {}",
            input.get("name").and_then(|n| n.as_str()).unwrap_or("?")
        )
    }

    /// From the input alone, so the card is there the moment the call is
    /// announced — the order `open_file` keeps.
    fn view(&self, _id: &str, input: &serde_json::Value) -> Option<taurus_tools::TranscriptView> {
        let input: OpenNoteInput = serde_json::from_value(input.clone()).ok()?;
        Some(taurus_tools::TranscriptView::Note {
            scope: input.scope,
            name: input.name,
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &taurus_tools::ToolContext,
    ) -> taurus_tools::ToolResult {
        let input: OpenNoteInput = taurus_tools::tool::parse_input(input)?;
        let page = read(
            input.scope,
            Some(&self.workspace),
            PageKind::Note,
            &input.name,
        )
        .map_err(taurus_tools::ToolError::InvalidInput)?;
        let lines = page.text.lines().count();
        Ok(format!(
            "Put a card for the note '{}' in the conversation; the user can open it from there. It \
             has {lines} lines. Do not repeat its contents.",
            page.name
        )
        .into())
    }
}

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
        create(scope, Some(w), PageKind::Note, name).unwrap()
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
            assert!(read(Scope::Workspace, Some(w), PageKind::Note, "../../secrets").is_err());
            assert!(save(
                Scope::Workspace,
                Some(w),
                PageKind::Note,
                "../x",
                "hi",
                "0-0"
            )
            .is_err());
            assert!(forget(Scope::Workspace, Some(w), PageKind::Note, "..").is_err());
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
            assert!(create(Scope::Global, None, PageKind::Note, "Reading list").is_ok());
            assert_eq!(list(None).len(), 1);
        });
    }

    #[test]
    fn a_project_note_with_no_workspace_says_what_to_do() {
        isolated(|_| {
            let why = create(Scope::Workspace, None, PageKind::Note, "Nowhere").unwrap_err();
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
            let why = create(Scope::Workspace, Some(w), PageKind::Note, "Notes").unwrap_err();
            assert!(why.contains("already a note"), "{why}");
            assert!(why.contains("this project"), "{why}");
            // And the same name in the other scope is a different note.
            assert!(create(Scope::Global, Some(w), PageKind::Note, "Notes").is_ok());
        });
    }

    #[test]
    fn renaming_moves_the_file_and_keeps_the_text() {
        isolated(|w| {
            let made = page(Scope::Workspace, w, "Draft");
            save(
                Scope::Workspace,
                Some(w),
                PageKind::Note,
                "Draft",
                "# Draft\n\nbody\n",
                &made.fingerprint,
            )
            .unwrap();
            let moved =
                rename(Scope::Workspace, Some(w), PageKind::Note, "Draft", "Final").unwrap();
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
            assert!(rename(Scope::Workspace, Some(w), PageKind::Note, "One", "Two").is_err());
            // Onto itself is not a collision, so a case-only change is allowed
            // wherever the filesystem can tell the two apart.
            assert!(rename(Scope::Workspace, Some(w), PageKind::Note, "One", "One").is_ok());
        });
    }

    #[test]
    fn forgetting_a_note_that_is_already_gone_is_the_outcome_that_was_asked_for() {
        isolated(|w| {
            page(Scope::Workspace, w, "Notes");
            assert_eq!(
                forget(Scope::Workspace, Some(w), PageKind::Note, "Notes")
                    .unwrap()
                    .len(),
                0
            );
            assert!(forget(Scope::Workspace, Some(w), PageKind::Note, "Notes").is_ok());
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
                PageKind::Note,
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
            let saved = save(
                Scope::Workspace,
                Some(w),
                PageKind::Note,
                "Notes",
                "mine\n",
                "0-0",
            )
            .unwrap();
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
            let why = read(Scope::Workspace, Some(w), PageKind::Note, "Huge").unwrap_err();
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
                PageKind::Note,
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
            let made = create(Scope::Global, Some(w), PageKind::Note, "Reading list").unwrap();
            save(
                Scope::Global,
                Some(w),
                PageKind::Note,
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
            save(
                Scope::Workspace,
                Some(w),
                PageKind::Note,
                "Blank",
                "",
                &made.fingerprint,
            )
            .unwrap();
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

    /* ------------------------------------------------------------ sketches */

    #[test]
    fn a_note_and_a_sketch_can_share_a_name() {
        // `Auth.md` and `Auth.excalidraw` are a note and the sketch it embeds,
        // which is the ordinary pair — refusing the second would make naming a
        // note's own diagram after it impossible.
        isolated(|w| {
            page(Scope::Workspace, w, "Auth");
            let sketch = create(Scope::Workspace, Some(w), PageKind::Sketch, "Auth").unwrap();
            assert_eq!(sketch.kind, PageKind::Sketch);
            assert!(w.join(".taurus/notes/Auth.excalidraw").is_file());
            let kinds: Vec<_> = list(Some(w)).iter().map(|p| p.kind).collect();
            assert!(kinds.contains(&PageKind::Note), "{kinds:?}");
            assert!(kinds.contains(&PageKind::Sketch), "{kinds:?}");
        });
    }

    #[test]
    fn a_new_sketch_is_a_drawing_excalidraw_can_open() {
        // An empty file is not a drawing, and the editor refuses what it cannot
        // parse — so a new sketch that were empty would open as an error.
        isolated(|w| {
            let sketch = create(Scope::Workspace, Some(w), PageKind::Sketch, "Flow").unwrap();
            let scene: serde_json::Value = serde_json::from_str(&sketch.text).unwrap();
            assert_eq!(scene["type"], "excalidraw");
            assert!(scene["elements"].as_array().unwrap().is_empty());
        });
    }

    #[test]
    fn a_taken_sketch_name_is_refused_in_the_words_for_a_sketch() {
        isolated(|w| {
            create(Scope::Workspace, Some(w), PageKind::Sketch, "Flow").unwrap();
            let why = create(Scope::Workspace, Some(w), PageKind::Sketch, "Flow").unwrap_err();
            assert!(why.contains("already a sketch called 'Flow'"), "{why}");
        });
    }

    /// A save that went through at any size would leave behind a file the next
    /// read refuses: the typing kept, and the note lost to whoever typed it.
    #[test]
    fn a_save_too_big_to_open_again_is_refused_and_writes_nothing() {
        isolated(|w| {
            let made = page(Scope::Workspace, w, "Big");
            let huge = "x".repeat(MAX_NOTE_BYTES as usize + 1);
            let why = save(
                Scope::Workspace,
                Some(w),
                PageKind::Note,
                "Big",
                &huge,
                &made.fingerprint,
            )
            .unwrap_err();
            assert!(why.starts_with("Not saved."), "{why}");
            assert_eq!(
                read(Scope::Workspace, Some(w), PageKind::Note, "Big")
                    .unwrap()
                    .text,
                "# Big\n\n"
            );
        });
    }

    #[test]
    fn a_sketch_too_big_to_open_blames_the_pictures() {
        isolated(|w| {
            create(Scope::Workspace, Some(w), PageKind::Sketch, "Shots").unwrap();
            let path = w.join(".taurus/notes/Shots.excalidraw");
            std::fs::write(&path, "x".repeat(MAX_SKETCH_BYTES as usize + 1)).unwrap();
            let why = read(Scope::Workspace, Some(w), PageKind::Sketch, "Shots").unwrap_err();
            assert!(why.contains("pictures pasted into it"), "{why}");
        });
    }

    #[test]
    fn embeds_are_found_in_either_spelling_and_only_beside_the_note() {
        let note = "![a](<Auth flow.excalidraw>) ![b](flow.excalidraw) ![c](./flow.excalidraw) \
                    ![d](shot.png) ![e](../other/far.excalidraw) ![f](Auth%20flow.excalidraw) \
                    [a link](notes.excalidraw)";
        // `[a link](…)` has the same shape as an image and names a sketch too;
        // counting it costs a sentence about a sketch the note does point at.
        assert_eq!(embeds(note), vec!["Auth flow", "flow", "notes"]);
    }

    #[test]
    fn a_sketch_says_the_words_written_on_it_and_nothing_else() {
        let scene = r#"{"elements":[
            {"type":"text","text":"Client"},
            {"type":"rectangle"},
            {"type":"text","text":"Token\n  store"},
            {"type":"text","text":"gone","isDeleted":true},
            {"type":"text","text":"Client"}
        ]}"#;
        assert_eq!(sketch_text(scene), vec!["Client", "Token store"]);
        assert!(sketch_text("not a scene").is_empty());
        assert!(sketch_text(r#"{"elements":"nope"}"#).is_empty());
    }

    #[test]
    fn read_note_says_what_is_written_on_the_sketches_it_embeds() {
        isolated(|w| {
            let note = page(Scope::Workspace, w, "Auth");
            let sketch = create(Scope::Workspace, Some(w), PageKind::Sketch, "Auth flow").unwrap();
            save(
                Scope::Workspace,
                Some(w),
                PageKind::Sketch,
                "Auth flow",
                r#"{"type":"excalidraw","elements":[{"type":"text","text":"Gateway"}]}"#,
                &sketch.fingerprint,
            )
            .unwrap();
            save(
                Scope::Workspace,
                Some(w),
                PageKind::Note,
                "Auth",
                "# Auth\n\n![Auth flow](<Auth flow.excalidraw>)\n\n![Gone](<Gone.excalidraw>)\n",
                &note.fingerprint,
            )
            .unwrap();
            let text = block(read_tool(w, Scope::Workspace, "Auth")).unwrap();
            assert!(
                text.contains("the text written on it is: Gateway"),
                "{text}"
            );
            assert!(text.contains("you cannot see it"), "{text}");
            assert!(
                text.contains("'Gone' that is not in this notebook"),
                "{text}"
            );
        });
    }

    /* ------------------------------------------------------ writing a note */

    async fn write_tool(w: &Path, input: serde_json::Value) -> Result<String, String> {
        use taurus_tools::Tool;
        WriteNote::new(w)
            .execute(input, &context(w))
            .await
            .map(|out| out.as_text().unwrap_or_default().to_string())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn write_note_makes_a_note_and_says_that_it_made_one() {
        isolated(|w| {
            let said = block(write_tool(
                w,
                serde_json::json!({ "scope": "workspace", "name": "Plan", "text": "# Plan\n" }),
            ))
            .unwrap();
            assert!(
                said.starts_with("Created the project note 'Plan'"),
                "{said}"
            );
            assert_eq!(
                read(Scope::Workspace, Some(w), PageKind::Note, "Plan")
                    .unwrap()
                    .text,
                "# Plan\n"
            );
        });
    }

    #[test]
    fn write_note_keeps_a_crlf_note_crlf() {
        isolated(|w| {
            let dir = w.join(".taurus/notes");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("Plan.md"), "# Plan\r\n").unwrap();
            block(write_tool(
                w,
                serde_json::json!({ "scope": "workspace", "name": "Plan", "text": "# Plan\n\nmore\n" }),
            ))
            .unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.join("Plan.md")).unwrap(),
                "# Plan\r\n\r\nmore\r\n"
            );
        });
    }

    #[test]
    fn write_note_cannot_be_talked_out_of_the_notes_directory() {
        isolated(|w| {
            let why = block(write_tool(
                w,
                serde_json::json!({ "scope": "workspace", "name": "../../escape", "text": "x" }),
            ))
            .unwrap_err();
            assert!(why.contains("cannot contain /"), "{why}");
            assert!(!w.join("escape.md").exists());
        });
    }

    /// The two things that make approving a project note's write safe: a rewind
    /// can put it back, and an open editor learns that it changed.
    #[test]
    fn write_note_declares_a_project_note_and_nothing_else() {
        use taurus_tools::Tool;
        let tool = WriteNote::new(Path::new("/w"));
        let at = |scope: &str, name: &str| {
            tool.touches(&serde_json::json!({ "scope": scope, "name": name, "text": "" }))
        };
        assert_eq!(
            at("workspace", "Plan"),
            vec![".taurus/notes/Plan.md".to_string()]
        );
        // A global note is outside every workspace, so there is nothing to
        // declare — and the docs say that is the one write a rewind cannot undo.
        assert!(at("global", "Plan").is_empty());
        // A name that could not be a note declares nothing rather than a path.
        assert!(at("workspace", "../x").is_empty());
    }

    #[test]
    fn write_note_shows_the_change_it_asks_to_make() {
        isolated(|w| {
            use taurus_tools::Tool;
            page(Scope::Workspace, w, "Plan");
            let diff = block(WriteNote::new(w).diff(
                &serde_json::json!({ "scope": "workspace", "name": "Plan", "text": "# Plan\n\nnew\n" }),
                w,
            ))
            .unwrap();
            assert!(!diff.is_empty());
        });
    }

    /* ------------------------------------------------------ opening a note */

    #[test]
    fn open_note_draws_a_card_from_its_input_and_sends_none_of_the_note() {
        isolated(|w| {
            use taurus_tools::Tool;
            let made = page(Scope::Workspace, w, "Plan");
            save(
                Scope::Workspace,
                Some(w),
                PageKind::Note,
                "Plan",
                "# Plan\n\nsecret sauce\n",
                &made.fingerprint,
            )
            .unwrap();
            let tool = OpenNote::new(w);
            let input = serde_json::json!({ "scope": "workspace", "name": "Plan" });
            assert!(matches!(
                tool.view("call-1", &input),
                Some(taurus_tools::TranscriptView::Note { ref name, .. }) if name == "Plan"
            ));
            let said = block(async {
                tool.execute(input, &context(w))
                    .await
                    .map(|out| out.as_text().unwrap_or_default().to_string())
                    .map_err(|e| e.to_string())
            })
            .unwrap();
            assert!(said.contains("3 lines"), "{said}");
            assert!(
                !said.contains("secret sauce"),
                "the note reached the model: {said}"
            );
        });
    }

    /// A note listed but deleted underneath is the ordinary consequence of the
    /// directory being somebody's to edit.
    #[test]
    fn reading_a_note_that_is_gone_says_so() {
        isolated(|w| {
            let why = read(Scope::Workspace, Some(w), PageKind::Note, "Ghost").unwrap_err();
            assert!(why.contains("no note called 'Ghost'"), "{why}");
        });
    }
}
