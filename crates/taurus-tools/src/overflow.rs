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

use std::io::{BufWriter, Write};
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
    join_ends(&text[..head], &gap(text.len() - cap), &text[tail..])
}

/// A head and a tail, with the sentence that stands for what went between
/// them, laid out the one way every cut here is.
pub fn join_ends(head: &str, gap: &str, tail: &str) -> String {
    format!("{head}\n\n[… {gap} …]\n\n{tail}")
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
    let mut spilling = SpillTo::new(label, ctx)?.open()?;
    // Megabytes written, at the end of every command whose output was cut —
    // from inside the shell's, an MCP server's, or a skill script's tool, none
    // of which can hand it to a blocking thread of its own. See [`blocking`].
    blocking(|| {
        spilling.write(text.as_bytes());
        spilling.finish()
    })
}

/// Where a stream will be written whole, settled before any of it exists.
///
/// For a stream too long to hold, which is written as it arrives rather than
/// all at once at the end — see [`crate::capture`]. [`spill`] is the same
/// thing for text already in hand.
#[derive(Debug)]
pub struct SpillTo {
    dir: PathBuf,
    path: PathBuf,
}

impl SpillTo {
    /// `None` when there is nowhere to put it, which is every caller outside a
    /// session. `label` becomes part of the filename, as it does for [`spill`].
    pub fn new(label: &str, ctx: &ToolContext) -> Option<Self> {
        let dir = ctx.command_output.clone()?;
        let path = dir.join(format!(
            "{}-{}-{}.txt",
            slug(ctx.session_id.as_deref().unwrap_or("session")),
            slug(ctx.call_id.as_deref().unwrap_or("command")),
            slug(label)
        ));
        Some(Self { dir, path })
    }

    /// Starts the file. `None`, silently, when it cannot be: the tool ran, and
    /// losing the copy costs the model a second look at the middle rather than
    /// the result.
    pub fn open(&self) -> Option<Spilling> {
        blocking(|| {
            std::fs::create_dir_all(&self.dir).ok()?;
            // Before the write rather than after, so the directory is at its
            // bound once this one lands rather than one over it until the next
            // command runs.
            prune(&self.dir, KEPT.saturating_sub(1));
            let file = std::fs::File::create(&self.path).ok()?;
            Some(Spilling {
                file: Some(BufWriter::with_capacity(256 * 1024, file)),
                path: self.path.clone(),
            })
        })
    }
}

/// A spill being written.
#[derive(Debug)]
pub struct Spilling {
    /// Gone after a failed write, so the rest are skipped and the gap says only
    /// how much went — never a path to a file that stops partway.
    file: Option<BufWriter<std::fs::File>>,
    path: PathBuf,
}

impl Spilling {
    pub fn write(&mut self, bytes: &[u8]) {
        if let Some(file) = &mut self.file {
            if file.write_all(bytes).is_err() {
                self.file = None;
            }
        }
    }

    /// The path to hand back as one to read, if every byte made it there.
    pub fn finish(self) -> Option<PathBuf> {
        let mut file = self.file?;
        file.flush().ok()?;
        drop(file);
        // Canonicalized because this is about to be handed back as a path to
        // read, and the guard that decides whether it may be read canonicalizes
        // both sides before comparing them.
        self.path.canonicalize().ok()
    }
}

/// Runs blocking file work without stalling the async runtime's other tasks.
///
/// On a multi-threaded runtime this is `block_in_place`, which hands the work
/// queued on this worker to the others before it starts, so the stream and the
/// permission prompt keep moving while a spill is written. Anywhere else — a
/// single-threaded runtime, or none — it simply runs: there is no other worker
/// to hand anything to.
fn blocking<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
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
