//! What a command printed: whole while it is small, and its two ends once it
//! is not.
//!
//! A command's output was read a line at a time into one string, with no bound
//! on a line or on the whole. One 100 MB line was buffered, copied into the
//! model's copy, copied again for the screen, sent to the webview as a single
//! message, and later copied up to three more times by the filters that shorten
//! it — to show the model half a megabyte at most.
//!
//! [`Capture`] keeps a stream whole only while everything downstream can use all
//! of it. Under [`HOLD`], sixteen times the most a command may show the model,
//! nothing changes: the model's copy is the whole stream, repeats are collapsed
//! across all of it, and a cut spills it at the end. Past [`HOLD`] the stream is
//! written to the spill file as it arrives — what was held, then every chunk
//! after — and memory keeps only the head and the tail the model will be shown.

use std::path::PathBuf;

use crate::overflow::{SpillTo, Spilling};

/// How much of a stream is kept whole.
pub const HOLD: usize = 8 * 1024 * 1024;

/// How much of each end is kept past [`HOLD`]: the most a cut shows of either,
/// the largest cut being `MAX_OUTPUT_BYTES` in the shell tool.
pub const ENDS: usize = 512 * 1024;

/// A stream of output as it arrives.
pub struct Capture {
    hold: usize,
    ends: usize,
    /// Everything, while the stream is under `hold`. Emptied when it passes.
    whole: Vec<u8>,
    /// Past `hold`: the stream's first `ends` bytes.
    head: Vec<u8>,
    /// Past `hold`: at least its newest `ends` bytes, and at most twice that
    /// between trims.
    tail: Vec<u8>,
    total: usize,
    /// Where the stream goes once it passes `hold`. `None` where nowhere was
    /// given — a test, an example, a tool run outside a session.
    spill_to: Option<SpillTo>,
    spilling: Option<Spilling>,
    over: bool,
}

/// What a finished capture holds.
#[derive(Debug)]
pub enum Captured {
    /// The whole stream.
    Whole(Vec<u8>),
    /// A stream that passed [`HOLD`]: its two ends, how long it was, and where
    /// the whole of it was written, if anywhere could take it.
    Ends {
        head: Vec<u8>,
        tail: Vec<u8>,
        total: usize,
        path: Option<PathBuf>,
    },
}

impl Captured {
    /// What it holds as text, for a message with no model to fit it to: the
    /// whole stream, or its two ends with a mark where the rest was.
    pub fn lossy_text(&self) -> String {
        match self {
            Captured::Whole(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            Captured::Ends { head, tail, .. } => format!(
                "{}\n[…]\n{}",
                String::from_utf8_lossy(head),
                String::from_utf8_lossy(tail)
            ),
        }
    }

    /// The same capture with its text passed through `f` — for the terminal
    /// path, whose escape sequences come off before the model reads it.
    ///
    /// Only what is in memory: a stream written to disk past [`HOLD`] keeps its
    /// escapes there, which a model reading the file can see past.
    pub fn map_text(self, f: impl Fn(&str) -> String) -> Self {
        let apply = |bytes: Vec<u8>| f(&String::from_utf8_lossy(&bytes)).into_bytes();
        match self {
            Captured::Whole(bytes) => Captured::Whole(apply(bytes)),
            Captured::Ends {
                head,
                tail,
                total,
                path,
            } => Captured::Ends {
                head: apply(head),
                tail: apply(tail),
                total,
                path,
            },
        }
    }
}

impl Capture {
    pub fn new(spill_to: Option<SpillTo>) -> Self {
        Self::bounded(HOLD, ENDS, spill_to)
    }

    /// With both sizes given, so a test can work in bytes rather than
    /// megabytes.
    pub(crate) fn bounded(hold: usize, ends: usize, spill_to: Option<SpillTo>) -> Self {
        Self {
            hold,
            ends,
            whole: Vec::new(),
            head: Vec::new(),
            tail: Vec::new(),
            total: 0,
            spill_to,
            spilling: None,
            over: false,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        if !self.over {
            if self.whole.len() + bytes.len() <= self.hold {
                self.whole.extend_from_slice(bytes);
                return;
            }
            self.pass_hold();
        }
        if let Some(spilling) = &mut self.spilling {
            spilling.write(bytes);
        }
        self.keep_tail(bytes);
    }

    /// The moment the stream stops fitting. What was held goes to the file,
    /// and from here on memory keeps only the ends.
    fn pass_hold(&mut self) {
        self.over = true;
        let whole = std::mem::take(&mut self.whole);
        self.spilling = self.spill_to.as_ref().and_then(SpillTo::open);
        if let Some(spilling) = &mut self.spilling {
            spilling.write(&whole);
        }
        self.head = whole[..whole.len().min(self.ends)].to_vec();
        self.keep_tail(&whole);
    }

    /// Keeps the newest `ends` bytes, trimming only once there is twice that,
    /// so a trim moves `ends` bytes once per `ends` bytes of output rather than
    /// on every chunk.
    fn keep_tail(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.ends {
            self.tail.clear();
            self.tail
                .extend_from_slice(&bytes[bytes.len() - self.ends..]);
            return;
        }
        self.tail.extend_from_slice(bytes);
        if self.tail.len() > 2 * self.ends {
            let cut = self.tail.len() - self.ends;
            self.tail.drain(..cut);
        }
    }

    pub fn finish(self) -> Captured {
        if !self.over {
            return Captured::Whole(self.whole);
        }
        let mut tail = self.tail;
        let extra = tail.len().saturating_sub(self.ends);
        tail.drain(..extra);
        Captured::Ends {
            head: self.head,
            tail,
            total: self.total,
            path: self.spilling.and_then(Spilling::finish),
        }
    }
}

/// Text from bytes that arrive in pieces, without splitting a character across
/// two of them.
///
/// A piece ends wherever the pipe's buffer happened to, which is as likely to
/// be inside a multi-byte character as between two. Decoded as it stood, such
/// a character reached the screen as two replacement marks. The bytes of one
/// not yet complete are held for the next piece instead.
#[derive(Default)]
pub struct Utf8Carry(Vec<u8>);

impl Utf8Carry {
    pub fn text(&mut self, bytes: &[u8]) -> String {
        self.0.extend_from_slice(bytes);
        let keep = incomplete_suffix(&self.0);
        let rest = self.0.split_off(self.0.len() - keep);
        let text = String::from_utf8_lossy(&self.0).into_owned();
        self.0 = rest;
        text
    }
}

/// How many bytes at the end of `bytes` begin a character they do not finish.
fn incomplete_suffix(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(4) {
        let byte = bytes[bytes.len() - back];
        if byte & 0b1100_0000 == 0b1000_0000 {
            continue;
        }
        let width = match byte {
            0xF0..=0xF7 => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        return if width > back { back } else { 0 };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_ctx;

    /// Five thousand bytes of numbered lines, so any byte out of place shows.
    fn stream() -> Vec<u8> {
        (0..1000u32)
            .flat_map(|i| format!("{i:04}\n").into_bytes())
            .collect()
    }

    #[test]
    fn a_stream_under_the_hold_is_kept_whole() {
        let mut capture = Capture::bounded(64, 16, None);
        capture.push(b"under ");
        capture.push(b"the hold");
        assert!(matches!(capture.finish(), Captured::Whole(bytes) if bytes == b"under the hold"));
    }

    #[test]
    fn a_stream_exactly_at_the_hold_is_still_whole() {
        let mut capture = Capture::bounded(8, 4, None);
        capture.push(b"12345678");
        assert!(matches!(capture.finish(), Captured::Whole(bytes) if bytes.len() == 8));
    }

    #[test]
    fn past_the_hold_memory_keeps_the_ends_and_the_file_keeps_everything() {
        let (mut ctx, _dir) = test_ctx();
        let spills = tempfile::TempDir::new().unwrap();
        ctx.command_output = Some(spills.path().to_path_buf());
        let stream = stream();

        let mut capture = Capture::bounded(64, 16, SpillTo::new("stdout", &ctx));
        for chunk in stream.chunks(7) {
            capture.push(chunk);
            // The whole point: however long the stream runs, this is all that
            // is in memory.
            assert!(capture.whole.len() <= 64 && capture.head.len() <= 16);
            assert!(capture.tail.len() <= 32, "{}", capture.tail.len());
        }

        let Captured::Ends {
            head,
            tail,
            total,
            path,
        } = capture.finish()
        else {
            panic!("a stream past the hold comes back as its ends");
        };
        assert_eq!(total, stream.len());
        assert_eq!(head, &stream[..16]);
        assert_eq!(tail, &stream[stream.len() - 16..]);
        let path = path.expect("the stream was written out");
        assert_eq!(std::fs::read(path).unwrap(), stream);
    }

    #[test]
    fn with_nowhere_to_write_it_the_ends_still_come_back() {
        let stream = stream();
        let mut capture = Capture::bounded(64, 16, None);
        for chunk in stream.chunks(100) {
            capture.push(chunk);
        }
        let Captured::Ends { tail, path, .. } = capture.finish() else {
            panic!("a stream past the hold comes back as its ends");
        };
        assert_eq!(tail, &stream[stream.len() - 16..]);
        assert!(path.is_none());
    }

    #[test]
    fn a_character_split_between_two_pieces_arrives_whole() {
        let mut carry = Utf8Carry::default();
        let e = "é".as_bytes();
        assert_eq!(carry.text(&[b'a', e[0]]), "a");
        assert_eq!(carry.text(&[e[1], b'b']), "éb");
        // Four bytes over three pieces.
        let crab = "🦀".as_bytes();
        assert_eq!(carry.text(&crab[..1]), "");
        assert_eq!(carry.text(&crab[1..3]), "");
        assert_eq!(carry.text(&crab[3..]), "🦀");
    }

    #[test]
    fn a_byte_that_begins_nothing_is_marked_at_once() {
        // Not held back waiting for the rest of a character it is not part
        // of: a stray byte is shown as one where it stood.
        let mut carry = Utf8Carry::default();
        assert_eq!(carry.text(&[0xFF, b'a']), "\u{FFFD}a");
    }
}
