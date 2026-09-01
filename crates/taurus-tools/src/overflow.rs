//! What to do when a tool's output does not fit.
//!
//! Two halves of one answer. [`cut`] keeps the ends of something too long and
//! says how much went; [`spill`] writes the whole of it somewhere the model can
//! go and read. Neither is much use without the other — a cut with no file
//! behind it turns "the middle of this is elsewhere" into "the middle of this is
//! gone", and the only route back to it is to run the thing again.
//!
//! Extracted from the shell tool, which had both to itself. The argument for
//! sharing them is not that the code was duplicated — it was not yet — but that
//! the built-ins were the only tools bounded at all. An MCP server is a program
//! nobody here wrote, returning as much text as it likes into the same context
//! window, and it had none of this. The safeguards should not be weakest around
//! the least trusted thing in the process.

use std::path::{Path, PathBuf};

use crate::tool::ToolContext;

/// How many cut streams one workspace keeps on disk.
///
/// Enough that a model can still reach back several turns for the middle of a
/// build it was shown the ends of; few enough that a directory of logs never
/// grows into something somebody has to go and notice. Trimmed on the way to
/// writing one, which is the only moment this code runs at all.
pub const KEPT: usize = 20;

/// Keeps the head and tail of `text`, with `gap` filling in for the middle.
///
/// Both ends rather than a prefix, because the two most useful parts of a long
/// output are what it started doing and how it ended: errors and summaries live
/// at the bottom, and a cut that kept only the top would reliably discard the
/// answer while keeping the preamble.
///
/// `gap` is handed the number of bytes removed and returns the sentence that
/// stands in their place. It is a callback rather than a string because what is
/// worth saying there depends on whether the full text was written out
/// somewhere — see [`spill`].
pub fn cut(text: &str, cap: usize, gap: impl FnOnce(usize) -> String) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let head_len = cap * 2 / 3;
    let head = floor_boundary(text, head_len);
    let tail_start = text.len() - (cap - head_len);
    let tail = ceil_boundary(text, tail_start);
    format!(
        "{}\n\n[… {} …]\n\n{}",
        &text[..head],
        gap(text.len() - cap),
        &text[tail..]
    )
}

/// Writes text out whole and says where it went.
///
/// `None` when there is nowhere to put it, or the write failed, and both are
/// silent on purpose. The tool ran. Losing the copy costs the model a second
/// look at the middle, and turning that into a failed tool call would throw away
/// the result along with it.
///
/// `label` distinguishes two spills from one call — the shell writes `stdout`
/// and `stderr` separately — and becomes part of the filename.
pub fn spill(text: &str, label: &str, ctx: &ToolContext) -> Option<PathBuf> {
    let dir = ctx.command_output.as_ref()?;
    std::fs::create_dir_all(dir).ok()?;
    // Before the write rather than after, so the directory is at its bound
    // once this one lands rather than one over it until the next command runs.
    prune(dir, KEPT.saturating_sub(1));
    let path = dir.join(format!(
        "{}-{}-{}.txt",
        slug(ctx.session_id.as_deref().unwrap_or("session")),
        slug(ctx.call_id.as_deref().unwrap_or("command")),
        slug(label)
    ));
    std::fs::write(&path, text).ok()?;
    // Canonicalized because this is about to be handed back as a path to read,
    // and the guard that decides whether it may be read canonicalizes both
    // sides before comparing them.
    path.canonicalize().ok()
}

/// Keeps the newest `keep` files in a directory and deletes the rest.
///
/// Every failure here is ignored. This is tidying, and a directory that cannot
/// be tidied is not a reason to fail the call whose output was about to go into
/// it.
pub fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, entry.path()))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    // Newest first, so what survives is the head of the list.
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

/// A session or call id as a filename component.
///
/// Both are ids this process was handed rather than ids it chose — a provider
/// names the call, and an MCP server names its tools — so nothing guarantees
/// they are made of characters a path may contain.
fn slug(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    cleaned.trim_matches('-').chars().take(64).collect()
}

pub fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub fn ceil_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn something_that_fits_is_returned_untouched() {
        assert_eq!(cut("short", 100, |_| unreachable!()), "short");
    }

    #[test]
    fn both_ends_survive_the_cut() {
        // The whole argument for keeping the tail: an error message is at the
        // bottom, and a prefix-only cut discards it every time.
        let text = format!("START{}END", "x".repeat(500));
        let out = cut(&text, 100, |n| format!("{n} bytes omitted"));
        assert!(out.starts_with("START"), "{out}");
        assert!(out.ends_with("END"), "{out}");
        assert!(out.contains("bytes omitted"), "{out}");
    }

    #[test]
    fn the_gap_is_told_how_much_went() {
        let text = "x".repeat(1000);
        let out = cut(&text, 100, |n| format!("{n} gone"));
        assert!(out.contains("900 gone"), "{out}");
    }

    #[test]
    fn a_cut_never_lands_inside_a_character() {
        // Multi-byte throughout, so a naive byte index is overwhelmingly
        // likely to split one — and slicing on a non-boundary panics.
        let text = "é".repeat(1000);
        let out = cut(&text, 101, |n| format!("{n}"));
        assert!(out.contains('é'));
    }

    #[test]
    fn an_id_that_is_not_a_filename_becomes_one() {
        // A provider names the call and an MCP server names its tools, so
        // neither is guaranteed to be path-safe.
        assert_eq!(slug("../../etc/passwd"), "etc-passwd");
        assert_eq!(slug("call:1/2"), "call-1-2");
        // Case survives: two ids differing only in case are two files.
        assert_eq!(slug("toolu_01AbC"), "toolu-01AbC");
        assert!(!slug(&"x".repeat(200)).is_empty());
        assert!(slug(&"x".repeat(200)).len() <= 64);
    }

    #[test]
    fn spilling_with_nowhere_to_put_it_is_not_an_error() {
        // The tool ran. Losing the copy is not a reason to fail the call.
        let (ctx, _dir) = crate::test_support::test_ctx();
        assert!(spill("anything", "out", &ctx).is_none());
    }
}
