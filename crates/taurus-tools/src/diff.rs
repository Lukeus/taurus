//! What a write would actually change.
//!
//! The permission prompt's job is to let someone approve *this* call rather
//! than the tool in general, and for a file write the one-line preview stops
//! just short of doing that: `Write src/widget.rs (2140 bytes)` says a file is
//! about to be replaced and nothing about what with. For a new file that is the
//! whole story. For an overwrite it is the least informative moment in the
//! product — the bytes being destroyed are right there on disk, and the bytes
//! replacing them are in the tool call.
//!
//! So they are diffed. The pre-image this reads is the same one the checkpoint
//! log takes a moment later, which is why this costs a read that was going to
//! happen anyway.

use std::path::Path;

use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use ts_rs::TS;

/// Lines of unchanged context kept around each change.
const CONTEXT: usize = 3;

/// How many lines a prompt will show before it stops and says so.
///
/// A permission dialog is a thing someone reads in a couple of seconds to make
/// one decision. A thousand-line diff scrolled into it is not more information,
/// it is a wall that gets approved unread — which is the failure this feature
/// exists to prevent, arrived at from the other side.
const MAX_LINES: usize = 160;

/// A file whose contents are not text is not diffable and says so rather than
/// rendering its bytes as mojibake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// The line's text, with its trailing newline stripped.
    pub text: String,
    /// 1-based line number in the file as it is now. `None` on an added line.
    pub old_line: Option<usize>,
    /// 1-based line number in the file as it would be. `None` on a removal.
    pub new_line: Option<usize>,
}

/// A run of changed lines with its surrounding context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffHunk {
    pub lines: Vec<DiffLine>,
}

/// The change one call would make to one file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileDiff {
    /// Workspace-relative where possible, so it matches what the model was told.
    pub path: String,
    /// Nothing was there before. The dialog says "create", not "replace", and
    /// there is no removed side to show.
    pub created: bool,
    /// Nothing is there after. Only a recorded change can be this — a write
    /// never removes a file — and it is worth its own flag because an
    /// all-removed diff is otherwise indistinguishable from a file truncated
    /// to nothing, which is a different thing to have done.
    pub deleted: bool,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<DiffHunk>,
    /// Lines the [`MAX_LINES`] cap kept out. Zero means the diff is complete,
    /// which is the fact worth being sure of before approving one.
    pub elided: usize,
}

impl FileDiff {
    /// True when the call would leave the file byte-identical.
    ///
    /// Worth knowing at the prompt: a write that changes nothing is usually a
    /// model looping, and approving it is a wasted decision.
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0
    }
}

/// Diffs a proposed replacement against what is on disk.
///
/// Returns `None` when no useful diff exists: the file is there but is not
/// UTF-8 text, so rendering it would be noise rather than evidence. A file that
/// is simply absent is not a failure — it is a creation, and says so.
pub fn against_disk(workspace: &Path, path: &Path, updated: &str) -> Option<FileDiff> {
    // The same rendering every tool result uses, so a path in the dialog
    // reads identically to one in the transcript.
    let display = crate::path_guard::display(workspace, path);

    let original = match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            // Deliberately not lossy-decoded. A diff of replacement characters
            // says a file changed without saying how, which is the same
            // non-answer the byte count already gave.
            Err(_) => return None,
        },
        Err(_) => None,
    };

    let created = original.is_none();
    let original = original.unwrap_or_default();
    Some(build(display, created, false, &original, updated))
}

/// Diffs two strings that have already been read.
///
/// Used by `edit_file`, which computes its replacement from the original and
/// has both halves in hand.
pub fn between(display: String, original: &str, updated: &str) -> FileDiff {
    build(display, false, false, original, updated)
}

/// Diffs a change that has already happened, where either side may be nothing.
///
/// [`against_disk`] and [`between`] both describe a write that is about to
/// happen, and a write always leaves a file behind. A recorded change does not:
/// a turn that ran `rm` has an original and no replacement, and rendering that
/// as a file truncated to zero lines would describe the wrong act. `None` on
/// either side means the file was not there, which is how a creation and a
/// deletion tell themselves apart.
pub fn of_change(path: String, before: Option<&str>, after: Option<&str>) -> FileDiff {
    build(
        path,
        before.is_none(),
        after.is_none(),
        before.unwrap_or_default(),
        after.unwrap_or_default(),
    )
}

fn build(path: String, created: bool, deleted: bool, original: &str, updated: &str) -> FileDiff {
    let diff = TextDiff::from_lines(original, updated);

    let mut added = 0;
    let mut removed = 0;
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut shown = 0;
    let mut elided = 0;

    for group in diff.grouped_ops(CONTEXT) {
        let mut lines = Vec::new();
        for op in group {
            for change in diff.iter_changes(&op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => DiffLineKind::Context,
                    ChangeTag::Insert => {
                        added += 1;
                        DiffLineKind::Added
                    }
                    ChangeTag::Delete => {
                        removed += 1;
                        DiffLineKind::Removed
                    }
                };
                // Counting continues past the cap so the totals stay true: the
                // header says how big the change is even when the body cannot
                // show all of it.
                if shown >= MAX_LINES {
                    elided += 1;
                    continue;
                }
                shown += 1;
                lines.push(DiffLine {
                    kind,
                    text: change.value().trim_end_matches(['\n', '\r']).to_string(),
                    old_line: change.old_index().map(|i| i + 1),
                    new_line: change.new_index().map(|i| i + 1),
                });
            }
        }
        if !lines.is_empty() {
            hunks.push(DiffHunk { lines });
        }
    }

    FileDiff {
        path,
        created,
        deleted,
        added,
        removed,
        hunks,
        elided,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(diff: &FileDiff) -> Vec<(DiffLineKind, &str)> {
        diff.hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .map(|l| (l.kind, l.text.as_str()))
            .collect()
    }

    #[test]
    fn a_replaced_line_shows_both_sides() {
        // The whole point: `Write src/widget.rs (2140 bytes)` says a file is
        // about to be replaced and nothing about what with.
        let diff = between("a.rs".into(), "one\ntwo\nthree\n", "one\nTWO\nthree\n");
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
        let lines = kinds(&diff);
        assert!(lines.contains(&(DiffLineKind::Removed, "two")), "{lines:?}");
        assert!(lines.contains(&(DiffLineKind::Added, "TWO")), "{lines:?}");
        assert!(lines.contains(&(DiffLineKind::Context, "one")), "{lines:?}");
    }

    #[test]
    fn line_numbers_are_the_files_own() {
        // So a number read off the dialog means the same thing as one read off
        // `read_file`.
        let diff = between("a.rs".into(), "a\nb\nc\n", "a\nB\nc\n");
        let removed = diff.hunks[0]
            .lines
            .iter()
            .find(|l| l.kind == DiffLineKind::Removed)
            .unwrap();
        assert_eq!(removed.old_line, Some(2));
        assert_eq!(removed.new_line, None);
    }

    #[test]
    fn a_write_that_changes_nothing_is_reported_as_empty() {
        // Usually a model looping. Approving it is a wasted decision, and the
        // dialog can say so instead of showing an empty box.
        let diff = between("a.rs".into(), "same\n", "same\n");
        assert!(diff.is_empty());
        assert!(diff.hunks.is_empty());
    }

    #[test]
    fn a_new_file_is_a_creation_rather_than_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let diff =
            against_disk(dir.path(), &dir.path().join("new.txt"), "hello\n").expect("a diff");
        assert!(diff.created);
        assert_eq!(diff.removed, 0);
        assert_eq!(diff.added, 1);
    }

    #[test]
    fn an_existing_file_is_a_replacement_with_its_old_lines_shown() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("old.txt");
        std::fs::write(&path, "before\n").unwrap();

        let diff = against_disk(&root, &path, "after\n").expect("a diff");
        assert!(!diff.created);
        assert_eq!((diff.added, diff.removed), (1, 1));
        assert!(kinds(&diff).contains(&(DiffLineKind::Removed, "before")));
        // Relative, so it reads as the same path the model was given.
        assert_eq!(diff.path, "old.txt");
    }

    #[test]
    fn a_file_that_is_not_text_produces_no_diff_rather_than_mojibake() {
        // Replacement characters would say a file changed without saying how —
        // the same non-answer the byte count already gave.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        std::fs::write(&path, [0xff, 0xfe, 0x00, 0x01]).unwrap();
        assert!(against_disk(dir.path(), &path, "text").is_none());
    }

    #[test]
    fn a_recorded_deletion_is_not_a_file_truncated_to_nothing() {
        // Both have every line on the removed side. Only one of them means the
        // file is gone, and the Changes drawer has to say which.
        let deleted = of_change("gone.txt".into(), Some("a\nb\n"), None);
        let emptied = of_change("kept.txt".into(), Some("a\nb\n"), Some(""));

        assert!(deleted.deleted && !deleted.created);
        assert!(!emptied.deleted && !emptied.created);
        assert_eq!((deleted.added, deleted.removed), (0, 2));
        assert_eq!(emptied.removed, 2);
    }

    #[test]
    fn a_recorded_creation_has_no_removed_side() {
        // `State::Absent` is how the checkpoint log records a file the turn
        // brought into being, and it reads as a creation from either direction.
        let diff = of_change("new.rs".into(), None, Some("fn main() {}\n"));
        assert!(diff.created && !diff.deleted);
        assert_eq!((diff.added, diff.removed), (1, 0));
    }

    #[test]
    fn a_huge_diff_is_capped_but_still_counts_honestly() {
        // A wall of lines in a modal gets approved unread, which is the failure
        // this feature exists to prevent, arrived at from the other side. The
        // totals stay true so the header can say how big the change really is.
        let original: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let updated: String = (0..500).map(|i| format!("changed {i}\n")).collect();

        let diff = between("big.txt".into(), &original, &updated);
        let shown: usize = diff.hunks.iter().map(|h| h.lines.len()).sum();
        assert!(shown <= MAX_LINES, "{shown} lines is more than the cap");
        assert!(diff.elided > 0);
        assert_eq!(diff.added, 500, "the count must cover what is not shown");
        assert_eq!(diff.removed, 500);
    }

    #[test]
    fn unchanged_regions_between_edits_are_not_shown() {
        // Two edits a hundred lines apart are two hunks, not one diff carrying
        // the hundred unchanged lines between them.
        let original: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let updated = original
            .replace("line 5\n", "EDIT 5\n")
            .replace("line 150\n", "EDIT 150\n");

        let diff = between("f.txt".into(), &original, &updated);
        assert_eq!(diff.hunks.len(), 2);
        let shown: usize = diff.hunks.iter().map(|h| h.lines.len()).sum();
        assert!(shown < 30, "{shown} lines for two one-line edits");
    }
}
