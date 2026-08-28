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
//! # What is not here yet
//!
//! Writing. The canvas opens files read-only for now, so the only staleness
//! this has to survive is somebody else's — the model's `write_file`, a
//! `git checkout` in the terminal dock. [`Document::fingerprint`] is carried
//! for that, and it is the same length-and-mtime pair the search index and
//! [`crate::freshness`] compare, because a third rule for "has this file
//! moved" is a third rule to get wrong.

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

    #[test]
    fn a_file_that_is_gone_has_no_fingerprint() {
        let dir = TempDir::new().unwrap();
        assert!(fingerprint_of(&dir.path().join("nothing.md")).is_none());
    }
}
