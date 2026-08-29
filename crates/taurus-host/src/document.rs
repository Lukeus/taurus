//! One text file, as the editor beside the conversation sees it.
//!
//! # Why the canvas reads rather than being told
//!
//! `open_file` is the tool that opens a document, and the obvious shortcut
//! would be for it to hand the text over with the instruction. It does not,
//! and the separation is the whole of what keeps the feature honest.
//!
//! A tool call is a **record**: it is written into a transcript, kept, and
//! replayed a week later when somebody reopens the conversation. A file is the
//! opposite — it is whatever it says right now, and the interesting version is
//! always today's. Binding the two would mean a card that reopens last
//! Tuesday's `README.md`, or a transcript that grows by the size of every file
//! anybody looked at. So the call carries the *path*, and this reads the bytes.
//!
//! It falls out of that separation that the model and the editor can never
//! disagree about what a file says: `read_file` and this one read the same
//! disk, and neither is quoting a copy the other made.
//!
//! # Two writers, and the rule that keeps them apart
//!
//! Both the person and the model can change the same file, and neither waits
//! for the other. The rule is that **a save never overwrites something it has
//! not seen**: [`save`] is handed the fingerprint the editor was holding, and
//! refuses if the file on disk no longer matches it.
//!
//! A refusal is not an error. It comes back as [`Saved::Stale`] carrying what
//! is on disk *now*, in the same round trip, because the only useful thing to
//! do with "somebody else wrote this" is show both versions — and asking for
//! the other one afterwards would be a second call that can itself be raced.
//!
//! The fingerprint is the same length-and-mtime pair the search index and
//! [`crate::freshness`] compare, because a third rule for "has this file moved"
//! is a third rule to get wrong.
//!
//! # Why saving asks no permission
//!
//! Every *tool* that writes goes through the permission engine, and this does
//! not, which looks like a hole until you say who is acting. A tool call is the
//! model changing a file; this is a person typing in an editor they opened on
//! purpose, in their own workspace. Prompting for that would be the app asking
//! whether you meant to press the keys you just pressed — the same reason
//! nothing prompts for what is typed into the terminal dock.
//!
//! It follows that a canvas edit is **not** in the Changes drawer either. That
//! drawer is what this conversation changed, and the way back from it; your own
//! typing is neither, and git is the undo that covers it.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Bytes past which a file is not opened in the editor.
///
/// Not a rendering limit — the canvas paints only the lines on screen, so
/// scrolling a long file costs what scrolling a short one does. It is a
/// *transfer* limit: the file crosses the IPC channel as one string, and past a
/// few megabytes that is a stall with nothing on screen to explain it. Files
/// that large are generated, and a generated file is read in windows.
///
/// Deliberately the same number the tool enforces. Two limits that could drift
/// apart would produce a file the model is told it opened and the editor then
/// refuses.
pub const MAX_DOCUMENT_BYTES: u64 = 4 * 1024 * 1024;

/// A file's contents, and enough to notice it changing underneath.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Document {
    /// Workspace-relative with forward slashes, the way every path the user
    /// sees is written. This is what goes back into `open_file` and into
    /// [`crate::OnScreen`], so it has to be the form both of those take.
    pub path: String,
    /// The file, whole.
    pub text: String,
    /// How many lines it has, counted here so the gutter does not have to
    /// split the text a second time to find out.
    pub lines: u32,
    /// Length and mtime, as one opaque string.
    ///
    /// Opaque on purpose: the frontend's only correct use of this is to hand
    /// it back unchanged, and a structured pair invites arithmetic on a
    /// timestamp that means nothing on the other side of the channel.
    pub fingerprint: String,
}

/// What the file looked like when it was read.
///
/// Length and mtime — the comparison `make` and `rsync` have always used, and
/// the one [`taurus_index`] and [`crate::freshness`] already make. Not a hash:
/// a hash means reading every byte of every file to answer a question this
/// answers with a `stat`, and the failure it protects against — a file rewritten
/// within the same mtime tick, to the same length, with different contents — is
/// rare enough that no build system in fifty years has thought it worth the
/// cost.
///
/// Formatted rather than returned as a pair so the value can only be compared,
/// never interpreted.
pub fn fingerprint(meta: &std::fs::Metadata) -> String {
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{modified}", meta.len())
}

/// The fingerprint of the file at `path`, or `None` when there is no file.
///
/// A missing file is a state rather than a failure here, the same call
/// [`crate::freshness`] makes: the question being asked is "is what I hold
/// still current", and "it is gone" is an answer to that.
pub fn fingerprint_of(path: &Path) -> Option<String> {
    std::fs::metadata(path).ok().map(|m| fingerprint(&m))
}

/// What became of a save.
///
/// An enum rather than a `Result`, because being beaten to the file is not a
/// failure — it is an ordinary thing that happens when two writers share a
/// workspace, and the answer to it is a decision the person makes rather than
/// an error they read. Real failures — no permission, no disk — are still the
/// `Err` half of the [`Result`] this is wrapped in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum Saved {
    /// Written. Carries the document as it now is, with its new fingerprint,
    /// so the editor can go on saving without re-reading.
    Written { document: Document },
    /// Not written: the file changed after the editor last read it.
    ///
    /// Carries what is on disk **now**, in the same round trip. The only useful
    /// response to "somebody else wrote this" is to show both versions, and a
    /// second call to fetch the other one could itself be raced by a third
    /// write.
    Stale { current: Document },
}

/// Writes a file, unless it has moved since it was read.
///
/// # Line endings
///
/// Preserved, and this is the one place it would be easiest to get wrong. A
/// browser's `<textarea>` reports its value with LF endings whatever went in,
/// so a CRLF file edited in the canvas comes back here with every ending
/// silently changed. Written as-is, a three-line edit would land as a diff
/// touching every line in the file — which is not a cosmetic problem: it is the
/// actual change made unreviewable.
///
/// So the endings of the file on disk win, using the same [`to_crlf`] the
/// `write_file` tool uses, because two rules about line endings is one rule too
/// many.
pub fn save(workspace: &Path, path: &str, text: &str, fingerprint: &str) -> Result<Saved, String> {
    let resolved = taurus_tools::path_guard::resolve(workspace, path).map_err(|e| e.to_string())?;
    let shown = taurus_tools::path_guard::display(workspace, &resolved);

    let existing = std::fs::read_to_string(&resolved)
        .map_err(|e| format!("Could not read {shown} before saving it: {e}"))?;
    let now = fingerprint_of(&resolved).ok_or_else(|| format!("{shown} is no longer there."))?;

    // The whole of the guarantee. Checked against the file as it is rather than
    // against anything remembered here, so a write from any source — the model,
    // git, another editor — is caught by the same comparison.
    if now != fingerprint {
        return Ok(Saved::Stale {
            current: Document {
                lines: existing.lines().count() as u32,
                fingerprint: now,
                path: shown,
                text: existing,
            },
        });
    }

    let body = if existing.contains("\r\n") {
        taurus_tools::builtin::fs::to_crlf(text)
    } else {
        text.to_string()
    };

    // A plain write rather than temp-and-rename, matching `write_file`, which
    // writes the same files. Renaming would replace the inode, and a source
    // file's permissions, hard links and the editor another program has open on
    // it are all attached to that — a cost worth paying for a config file the
    // app owns, and not for one in somebody's repository.
    std::fs::write(&resolved, &body).map_err(|e| format!("Could not save {shown}: {e}"))?;

    let after = fingerprint_of(&resolved)
        .ok_or_else(|| format!("{shown} went missing as it was written."))?;
    Ok(Saved::Written {
        document: Document {
            lines: body.lines().count() as u32,
            fingerprint: after,
            path: shown,
            text: body,
        },
    })
}

/// Marks a moment as "now" for a fingerprint, for tests that need two of them.
#[cfg(test)]
pub(crate) fn at(len: u64, when: SystemTime) -> String {
    let nanos = when
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{len}-{nanos}")
}

#[cfg(not(test))]
#[allow(dead_code)]
fn _unused(_: SystemTime) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, text: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_file_that_has_not_moved_fingerprints_the_same_twice() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "a.md", "hello\n");
        assert_eq!(fingerprint_of(&path), fingerprint_of(&path));
    }

    /// The case this exists for: the model rewrote the file while it was open.
    #[test]
    fn a_rewritten_file_fingerprints_differently() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "a.md", "hello\n");
        let before = fingerprint_of(&path).unwrap();
        // A different length, which is what changes on nearly every real edit
        // and is the half of this that does not depend on clock resolution.
        std::fs::write(&path, "hello there\n").unwrap();
        assert_ne!(fingerprint_of(&path).unwrap(), before);
    }

    /// A rewrite that keeps the length is still caught, by the other half.
    #[test]
    fn a_same_length_rewrite_is_caught_by_the_timestamp() {
        let one = at(12, UNIX_EPOCH + Duration::from_secs(1_000));
        let two = at(12, UNIX_EPOCH + Duration::from_secs(1_001));
        assert_ne!(one, two);
    }

    /* ----------------------------------------------------------- saving */

    fn saved(dir: &TempDir, name: &str, text: &str, at: &str) -> Saved {
        save(&dir.path().canonicalize().unwrap(), name, text, at).unwrap()
    }

    #[test]
    fn a_save_writes_and_hands_back_the_new_fingerprint() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "a.md", "one\n");
        let before = fingerprint_of(&path).unwrap();

        let Saved::Written { document } = saved(&dir, "a.md", "one\ntwo\n", &before) else {
            panic!("a save against the current fingerprint should write");
        };
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        assert_eq!(document.lines, 2);
        // The new one, so the editor can go on saving without re-reading.
        assert_ne!(document.fingerprint, before);
        assert_eq!(document.fingerprint, fingerprint_of(&path).unwrap());
    }

    /// The whole guarantee: a save never overwrites something it has not seen.
    #[test]
    fn a_save_against_a_stale_fingerprint_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "a.md", "one\n");
        let held = fingerprint_of(&path).unwrap();

        // Somebody else — the model, git, another editor — gets there first.
        std::fs::write(&path, "theirs, entirely different\n").unwrap();

        let Saved::Stale { current } = saved(&dir, "a.md", "mine\n", &held) else {
            panic!("a save against a stale fingerprint must not write");
        };
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "theirs, entirely different\n",
            "the other writer's work was overwritten"
        );
        // And what is on disk now comes back in the same round trip, because
        // asking for it separately could be raced by a third write.
        assert_eq!(current.text, "theirs, entirely different\n");
        assert_eq!(current.fingerprint, fingerprint_of(&path).unwrap());
    }

    /// Saving again with what the refusal handed back is how "take theirs"
    /// works, and it has to succeed or the conflict would be unresolvable.
    #[test]
    fn saving_with_the_fingerprint_a_refusal_returned_succeeds() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "a.md", "one\n");
        let held = fingerprint_of(&path).unwrap();
        std::fs::write(&path, "theirs\n").unwrap();

        let Saved::Stale { current } = saved(&dir, "a.md", "mine\n", &held) else {
            panic!("expected a refusal");
        };
        let Saved::Written { .. } = saved(&dir, "a.md", "mine\n", &current.fingerprint) else {
            panic!("a save against the fingerprint just handed back should write");
        };
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine\n");
    }

    /// A `<textarea>` reports LF whatever went in, so without this a
    /// three-line edit to a CRLF file lands as a diff touching every line —
    /// the actual change made unreviewable.
    #[test]
    fn a_crlf_file_stays_crlf_however_the_editor_reports_it() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "w.txt", "a\r\nb\r\n");
        let at = fingerprint_of(&path).unwrap();

        saved(&dir, "w.txt", "a\nb\nc\n", &at);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\r\nb\r\nc\r\n");
    }

    #[test]
    fn an_lf_file_is_not_given_carriage_returns() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "w.txt", "a\nb\n");
        let at = fingerprint_of(&path).unwrap();

        saved(&dir, "w.txt", "a\nb\nc\n", &at);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nb\nc\n");
    }

    /// The guard, on the write side. An entry can arrive from a transcript
    /// somebody reopened, and `..` in one must not write outside the tree.
    #[test]
    fn a_path_climbing_out_of_the_workspace_is_refused() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.md", "one\n");
        let root = dir.path().canonicalize().unwrap();
        assert!(save(&root, "../escaped.md", "x", "1-2").is_err());
    }

    #[test]
    fn a_file_that_is_gone_has_no_fingerprint() {
        let dir = TempDir::new().unwrap();
        assert!(fingerprint_of(&dir.path().join("nothing.md")).is_none());
    }
}
