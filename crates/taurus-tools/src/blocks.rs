//! Where one command in a terminal ended and the next began.
//!
//! A terminal's output is one stream. A prompt, the command typed at it, what
//! the command printed and the next prompt all arrive as the same bytes, and
//! nothing in them says where one stops. That's why the dock used to be a
//! terminal and nothing more: there was no "the last command" for the agent to
//! read, and no "this failure" for a person to hand it.
//!
//! Shell integration supplies the boundaries. The shell is started with a few
//! hooks (see [`crate::shell_integration`]) that print invisible marks
//! around each command — the `OSC 133` convention iTerm2, kitty, WezTerm and
//! VS Code all speak:
//!
//! | Mark | Sent | Means |
//! |---|---|---|
//! | `ESC ] 133 ; A` | before the prompt is drawn | a prompt starts |
//! | `ESC ] 133 ; B` | at the end of the prompt | typing starts |
//! | `ESC ] 133 ; C ; cmdline_url=…` | as a command is about to run | output starts |
//! | `ESC ] 133 ; D ; <status>` | when it has finished | output ends |
//! | `ESC ] 7 ; file://host/path` | before each prompt | the shell's directory |
//!
//! The command line rides on `C`, percent-encoded, the way kitty's own
//! integration sends it. Recovering it from the echoed keystrokes instead would
//! mean replaying a line editor: a typo fixed with backspace, a history recall,
//! a completion menu, are all bytes on the wire that never ran.
//!
//! [`BlockTracker`] reads the stream and turns it into [`Block`]s. It sits on
//! the backend's side of the window, not the emulator's, because the agent is
//! one of its readers and the agent has no emulator.
//!
//! # What is kept
//!
//! A bounded tail of each command's output, with its escape sequences stripped
//! (the model learns nothing from a cursor move, and nor does a person reading
//! a quoted error). A build's useful part is its end, so the beginning is what
//! goes once a command prints more than [`MAX_RAW_BYTES`].
//!
//! A program that switches to the alternate screen — `vim`, `less`, `htop` — is
//! recorded as having run, with its exit status, and with no output. What it
//! drew was a screen addressed by coordinate, and stripped down to text it's
//! a heap of fragments that would read as output and mean nothing.

use std::collections::VecDeque;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use ts_rs::TS;

use crate::builtin::pty::strip_ansi;

/// How much of one command's output is held, as the bytes that arrived.
///
/// A quarter of a megabyte is a long build's last few thousand lines, which is
/// where the failure is. What came before is dropped, and the block says how
/// much.
pub const MAX_RAW_BYTES: usize = 256 * 1024;

/// How many finished commands are remembered.
///
/// Past this, the oldest go. A hundred is a long afternoon at a prompt, and at
/// [`MAX_RAW_BYTES`] apiece it's a bound on memory a laptop won't notice.
pub const MAX_BLOCKS: usize = 100;

/// The longest control string read before it is given up on.
///
/// A command line is the only payload here worth reading, and the hooks cap it
/// well below this. A string past it is something else, or a terminal that
/// lost its terminator, and either way waiting for the end would hold the rest
/// of the stream behind it.
const MAX_OSC_BYTES: usize = 16 * 1024;

/// The sequences that put a terminal on its alternate screen.
///
/// `1049` is what every modern full-screen program sends. `47` and `1047` are
/// the older spellings of the same switch, and some programs still use them.
const ALT_SCREEN: [&[u8]; 3] = [b"\x1b[?1049h", b"\x1b[?1047h", b"\x1b[?47h"];

/// One command, as the terminal ran it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct Block {
    /// Counts from 1, in the order commands started, for the life of one shell.
    #[ts(type = "number")]
    pub id: u64,
    /// What was typed. Empty when the shell started a command without saying
    /// what it was — which ours never do, but a hand-rolled prompt might.
    pub command: String,
    /// The shell's directory when the command started, if it said.
    #[ts(optional)]
    pub cwd: Option<String>,
    /// When it started, in Unix milliseconds. Formatting is the reader's job.
    #[ts(type = "number")]
    pub started_at: u64,
    /// How long it ran. `None` while it is still running.
    #[ts(optional, type = "number")]
    pub duration_ms: Option<u64>,
    /// Its exit status. `None` while it runs, and for one that ended without
    /// the shell reporting a status (the shell itself exited under it).
    #[ts(optional)]
    pub exit: Option<i32>,
    /// It is still running.
    pub running: bool,
    /// It took over the screen, so nothing it printed is kept. See the module
    /// note.
    pub full_screen: bool,
    /// Bytes of output dropped from the front to stay under
    /// [`MAX_RAW_BYTES`]. Zero for almost everything.
    #[ts(type = "number")]
    pub dropped_bytes: u64,
}

/// Something worth telling the pane about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockEvent {
    Started(Block),
    Finished(Block),
}

/// A block, with what it printed.
#[derive(Clone, Debug)]
pub struct BlockText {
    pub block: Block,
    /// The output, escape sequences stripped. Empty for a full-screen program.
    pub output: String,
}

/// Where the parser is in the byte stream.
#[derive(Debug)]
enum Scan {
    /// Ordinary output.
    Text,
    /// An `ESC`, and nothing after it yet.
    Escape,
    /// Inside `ESC ]`, collecting the payload.
    Osc,
    /// An `ESC` inside a control string: `\` next ends it.
    OscEscape,
}

/// A command that has started and not finished.
#[derive(Debug)]
struct Running {
    block: Block,
    began: Instant,
    raw: VecDeque<u8>,
    /// The last few bytes seen, so an alternate-screen switch split across two
    /// reads is still noticed.
    carry: Vec<u8>,
}

/// Reads a terminal's output and keeps the commands in it.
///
/// Fed every chunk the shell writes, in order, and returns what changed. It
/// never alters what it is fed: the emulator still gets every byte, marks
/// included, and draws its own gutter from them.
#[derive(Debug)]
pub struct BlockTracker {
    scan: Scan,
    osc: Vec<u8>,
    /// The payload grew past [`MAX_OSC_BYTES`], so the rest of it is skipped.
    osc_overflow: bool,
    cwd: Option<String>,
    next_id: u64,
    running: Option<Running>,
    finished: VecDeque<BlockText>,
    /// The shell has sent at least one mark. Until it does, nothing it says
    /// can be split into commands, and a reader should be told so rather
    /// than shown an empty list as though nothing had run.
    integrated: bool,
}

impl Default for BlockTracker {
    fn default() -> Self {
        Self {
            scan: Scan::Text,
            osc: Vec::new(),
            osc_overflow: false,
            cwd: None,
            next_id: 1,
            running: None,
            finished: VecDeque::new(),
            integrated: false,
        }
    }
}

impl BlockTracker {
    /// Reads one chunk of output.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<BlockEvent> {
        let mut events = Vec::new();
        // Output is copied into the running block a run at a time rather than a
        // byte at a time: during a build nearly every byte is plain text.
        let mut run_start = 0;
        for (i, &byte) in bytes.iter().enumerate() {
            match self.scan {
                Scan::Text => {
                    if byte == 0x1b {
                        self.capture(&bytes[run_start..i]);
                        self.scan = Scan::Escape;
                    }
                }
                Scan::Escape => match byte {
                    b']' => {
                        self.scan = Scan::Osc;
                        self.osc.clear();
                        self.osc_overflow = false;
                    }
                    // Two escapes in a row: the first was a whole sequence on
                    // its own, and the second starts another.
                    0x1b => self.capture(&[0x1b]),
                    // Any other escape is output: a color, a cursor move. Kept,
                    // so the alternate-screen check sees it and the strip on
                    // the way out removes it. The byte after the `ESC` starts
                    // the next run of text.
                    _ => {
                        self.capture(&[0x1b]);
                        self.scan = Scan::Text;
                        run_start = i;
                    }
                },
                Scan::Osc => match byte {
                    0x07 => {
                        self.end_osc(&mut events);
                        run_start = i + 1;
                    }
                    0x1b => self.scan = Scan::OscEscape,
                    _ => self.push_osc(byte),
                },
                Scan::OscEscape => {
                    if byte == b'\\' {
                        self.end_osc(&mut events);
                        run_start = i + 1;
                    } else {
                        // Not a terminator after all. Both bytes belong to the
                        // payload, which is what a terminal would make of them.
                        self.push_osc(0x1b);
                        self.push_osc(byte);
                        self.scan = Scan::Osc;
                    }
                }
            }
        }
        // A sequence still open at the end of the chunk is carried in `scan`
        // and `osc`, and finishes in the next one. Plain text up to here is
        // output now.
        if matches!(self.scan, Scan::Text) {
            self.capture(&bytes[run_start.min(bytes.len())..]);
        }
        events
    }

    /// The shell has spoken shell integration at least once.
    pub fn integrated(&self) -> bool {
        self.integrated
    }

    /// The `limit` most recent commands, oldest first, including one still
    /// running.
    pub fn recent(&self, limit: usize) -> Vec<BlockText> {
        let running = self.running.as_ref().map(|r| BlockText {
            block: r.block.clone(),
            output: render(&r.raw, r.block.full_screen),
        });
        let take = limit.saturating_sub(usize::from(running.is_some()));
        let skip = self.finished.len().saturating_sub(take);
        self.finished
            .iter()
            .skip(skip)
            .cloned()
            .chain(running)
            .collect()
    }

    /// One command by number, if it's still remembered.
    pub fn get(&self, id: u64) -> Option<BlockText> {
        if let Some(r) = self.running.as_ref().filter(|r| r.block.id == id) {
            return Some(BlockText {
                block: r.block.clone(),
                output: render(&r.raw, r.block.full_screen),
            });
        }
        self.finished.iter().find(|b| b.block.id == id).cloned()
    }

    /// Ends whatever is running, because the shell under it has gone.
    pub fn close(&mut self) -> Option<BlockEvent> {
        self.finish(None)
    }

    fn push_osc(&mut self, byte: u8) {
        if self.osc.len() < MAX_OSC_BYTES {
            self.osc.push(byte);
        } else {
            self.osc_overflow = true;
        }
    }

    fn end_osc(&mut self, events: &mut Vec<BlockEvent>) {
        self.scan = Scan::Text;
        if self.osc_overflow {
            return;
        }
        let payload = std::mem::take(&mut self.osc);
        let text = String::from_utf8_lossy(&payload);
        if let Some(mark) = text.strip_prefix("133;") {
            self.integrated = true;
            self.mark(mark, events);
        } else if let Some(url) = text.strip_prefix("7;") {
            if let Some(path) = cwd_from_url(url) {
                self.cwd = Some(path);
            }
        }
        // Every other control string — a window title, a hyperlink — is
        // dropped from the block's copy, which the strip would have done
        // anyway. The emulator still gets it.
    }

    fn mark(&mut self, mark: &str, events: &mut Vec<BlockEvent>) {
        let mut parts = mark.split(';');
        match parts.next() {
            Some("C") => {
                // A command starting while another is still open means the
                // shell skipped a `D` — a hook that failed, or a subshell that
                // printed marks of its own. The open one ends without a status
                // rather than swallowing the next command's output.
                if let Some(event) = self.finish(None) {
                    events.push(event);
                }
                let command = parts
                    .find_map(|p| p.strip_prefix("cmdline_url="))
                    .map(percent_decode)
                    .unwrap_or_default();
                let block = Block {
                    id: self.next_id,
                    command: command.trim().to_string(),
                    cwd: self.cwd.clone(),
                    started_at: now_ms(),
                    duration_ms: None,
                    exit: None,
                    running: true,
                    full_screen: false,
                    dropped_bytes: 0,
                };
                self.next_id += 1;
                events.push(BlockEvent::Started(block.clone()));
                self.running = Some(Running {
                    block,
                    began: Instant::now(),
                    raw: VecDeque::new(),
                    carry: Vec::new(),
                });
            }
            Some("D") => {
                let exit = parts.next().and_then(|s| s.trim().parse::<i32>().ok());
                if let Some(event) = self.finish(exit) {
                    events.push(event);
                }
            }
            // A prompt being drawn ends anything still running: the shell is
            // asking for the next command, so the last one is over whether or
            // not it said so.
            Some("A") => {
                if let Some(event) = self.finish(None) {
                    events.push(event);
                }
            }
            _ => {}
        }
    }

    fn finish(&mut self, exit: Option<i32>) -> Option<BlockEvent> {
        let running = self.running.take()?;
        let mut block = running.block;
        block.running = false;
        block.exit = exit;
        block.duration_ms = Some(running.began.elapsed().as_millis() as u64);
        let output = render(&running.raw, block.full_screen);
        self.finished.push_back(BlockText {
            block: block.clone(),
            output,
        });
        while self.finished.len() > MAX_BLOCKS {
            self.finished.pop_front();
        }
        Some(BlockEvent::Finished(block))
    }

    /// Adds output to the running command, if there is one.
    fn capture(&mut self, bytes: &[u8]) {
        let Some(running) = self.running.as_mut() else {
            return;
        };
        running.push(bytes);
    }
}

impl Running {
    fn push(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if !self.block.full_screen {
            let mut window = std::mem::take(&mut self.carry);
            window.extend_from_slice(bytes);
            if ALT_SCREEN
                .iter()
                .any(|seq| window.windows(seq.len()).any(|w| w == *seq))
            {
                self.block.full_screen = true;
                self.raw.clear();
            }
            let keep = window.len().saturating_sub(8);
            self.carry = window.split_off(keep);
        }
        // A full-screen program's drawing is not kept; see the module note.
        if self.block.full_screen {
            return;
        }
        self.raw.extend(bytes);
        let over = self.raw.len().saturating_sub(MAX_RAW_BYTES);
        if over > 0 {
            self.raw.drain(..over);
            self.block.dropped_bytes += over as u64;
        }
    }
}

/// What a block printed, as a person would read it.
fn render(raw: &VecDeque<u8>, full_screen: bool) -> String {
    if full_screen {
        return String::new();
    }
    let (front, back) = raw.as_slices();
    let mut bytes = Vec::with_capacity(raw.len());
    bytes.extend_from_slice(front);
    bytes.extend_from_slice(back);
    let bytes = without_prompt_sp(&bytes);
    let text = strip_ansi(&String::from_utf8_lossy(bytes));
    // A pty ends every line with CRLF, which the strip folds to one newline;
    // what is left at the ends is the newline after the command line and the
    // one before the next prompt.
    text.trim_matches('\n').to_string()
}

/// `bytes` without zsh's partial-line marker on the end.
///
/// zsh's `PROMPT_SP` prints a reverse-video `%` (or `#` for root), then
/// spaces to the edge of the screen, then `CR SPACE CR`, every time a command
/// finishes. On a screen it's invisible unless the output didn't end in a
/// newline. It's printed before any hook runs, so it can't be kept out of the
/// block from the shell side. In text it's a stray `%` and a line of spaces
/// under every command, so it comes off here.
///
/// Only that exact tail is touched: the `CR SPACE CR`, the spaces before it,
/// and a single `%` or `#` with only styling around it.
fn without_prompt_sp(bytes: &[u8]) -> &[u8] {
    let Some(mut rest) = bytes.strip_suffix(b"\r \r") else {
        return bytes;
    };
    while let Some(shorter) = rest.strip_suffix(b" ") {
        rest = shorter;
    }
    let unstyled = without_trailing_sgr(rest);
    match unstyled.last() {
        Some(b'%' | b'#') => without_trailing_sgr(&unstyled[..unstyled.len() - 1]),
        _ => rest,
    }
}

/// `bytes` without any color or style sequences (`ESC [ … m`) on the end.
fn without_trailing_sgr(mut bytes: &[u8]) -> &[u8] {
    while bytes.last() == Some(&b'm') {
        let Some(start) = bytes.iter().rposition(|&b| b == 0x1b) else {
            break;
        };
        let sequence = &bytes[start..];
        let is_sgr = sequence.len() >= 3
            && sequence[1] == b'['
            && sequence[2..sequence.len() - 1]
                .iter()
                .all(|b| b.is_ascii_digit() || *b == b';');
        if !is_sgr {
            break;
        }
        bytes = &bytes[..start];
    }
    bytes
}

/// The path in an `OSC 7` URL: `file://host/some%20path` becomes `/some path`.
///
/// The host is dropped. It names the machine the shell runs on, and that's
/// this one: the dock doesn't follow an `ssh` session, whose remote shell
/// would need its own integration.
fn cwd_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let decoded = percent_decode(path);
    (!decoded.is_empty()).then_some(decoded)
}

/// `%XX` escapes back into bytes, and the bytes into text.
///
/// Malformed escapes are left as they were rather than refused, because what's
/// being decoded is a label for a person, and a stray `%` in it is still
/// readable.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// The terminal the agent may read, from wherever it lives.
///
/// The dock's shells belong to the desktop app, which sits above every crate
/// the tools are in. So the tool is written against this, and the app
/// implements it downward, the same arrangement [`crate::SecretVault`] uses.
pub trait TerminalReader: Send + Sync {
    /// The shell's name, for the heading of what the tool returns: `zsh`.
    fn shell(&self) -> String;
    /// See [`BlockTracker::integrated`].
    fn integrated(&self) -> bool;
    /// See [`BlockTracker::recent`].
    fn recent(&self, limit: usize) -> Vec<BlockText>;
    /// See [`BlockTracker::get`].
    fn get(&self, id: u64) -> Option<BlockText>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: &str = "\x1b]133;C";
    const BEL: &str = "\x07";

    fn command(cmd: &str) -> String {
        let encoded: String = cmd
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_./~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        format!("{C};cmdline_url={encoded}{BEL}")
    }

    fn done(code: i32) -> String {
        format!("\x1b]133;D;{code}{BEL}")
    }

    fn prompt() -> String {
        format!("\x1b]133;A{BEL}$ \x1b]133;B{BEL}")
    }

    fn finished(events: &[BlockEvent]) -> Vec<Block> {
        events
            .iter()
            .filter_map(|e| match e {
                BlockEvent::Finished(b) => Some(b.clone()),
                BlockEvent::Started(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_command_becomes_a_block_with_its_line_status_and_output() {
        let mut t = BlockTracker::default();
        let stream = format!(
            "\x1b]7;file://mac.local/Users/me/my%20repo{BEL}{}cargo test\r\n{}\x1b[31merror\x1b[0m: 1 failed\r\n{}{}",
            prompt(),
            command("cargo test --workspace"),
            done(101),
            prompt(),
        );
        let events = t.feed(stream.as_bytes());
        let blocks = finished(&events);
        assert_eq!(blocks.len(), 1, "{events:?}");
        let block = &blocks[0];
        assert_eq!(block.id, 1);
        assert_eq!(block.command, "cargo test --workspace");
        assert_eq!(block.exit, Some(101));
        assert_eq!(block.cwd.as_deref(), Some("/Users/me/my repo"));
        assert!(!block.running);
        assert!(block.duration_ms.is_some());
        // The color is gone and the words are not, and the echoed command
        // line before the `C` mark is not part of the output.
        assert_eq!(t.get(1).unwrap().output, "error: 1 failed");
        assert!(t.integrated());
    }

    #[test]
    fn marks_split_across_reads_are_still_found() {
        // A read ends wherever the kernel had bytes ready, which during a busy
        // shell is often in the middle of a mark.
        let stream = format!(
            "{}{}hello\r\n{}{}",
            prompt(),
            command("echo hello"),
            done(0),
            prompt()
        );
        let bytes = stream.as_bytes();
        for split in 1..bytes.len() {
            let mut t = BlockTracker::default();
            let mut events = t.feed(&bytes[..split]);
            events.extend(t.feed(&bytes[split..]));
            let blocks = finished(&events);
            assert_eq!(blocks.len(), 1, "split at {split}: {events:?}");
            assert_eq!(blocks[0].command, "echo hello", "split at {split}");
            assert_eq!(t.get(1).unwrap().output, "hello", "split at {split}");
        }
    }

    #[test]
    fn a_string_terminator_ends_a_mark_as_well_as_a_bell() {
        let mut t = BlockTracker::default();
        let events = t.feed(b"\x1b]133;C;cmdline_url=ls\x1b\\out\r\n\x1b]133;D;0\x1b\\");
        let blocks = finished(&events);
        assert_eq!(blocks[0].command, "ls");
        assert_eq!(blocks[0].exit, Some(0));
        assert_eq!(t.get(1).unwrap().output, "out");
    }

    #[test]
    fn a_full_screen_program_is_recorded_without_its_drawing() {
        let mut t = BlockTracker::default();
        let stream = format!(
            "{}\x1b[?1049h\x1b[2J\x1b[1;1Hfile contents\x1b[?1049l{}",
            command("vim notes.md"),
            done(0)
        );
        let blocks = finished(&t.feed(stream.as_bytes()));
        assert!(blocks[0].full_screen);
        assert_eq!(t.get(1).unwrap().output, "");
    }

    #[test]
    fn a_long_command_keeps_its_end_and_counts_what_it_dropped() {
        let mut t = BlockTracker::default();
        t.feed(command("yes").as_bytes());
        let line = "y\r\n".repeat(1024);
        for _ in 0..(MAX_RAW_BYTES / line.len() + 10) {
            t.feed(line.as_bytes());
        }
        t.feed(b"the end\r\n");
        let blocks = finished(&t.feed(done(130).as_bytes()));
        assert!(blocks[0].dropped_bytes > 0);
        let output = t.get(1).unwrap().output;
        assert!(output.ends_with("the end"), "the tail is what matters");
        assert!(output.len() <= MAX_RAW_BYTES);
    }

    #[test]
    fn a_running_command_can_be_read_before_it_ends() {
        // A dev server never ends, and "what did it say?" is asked while it runs.
        let mut t = BlockTracker::default();
        t.feed(format!("{}listening on :5173\r\n", command("pnpm dev")).as_bytes());
        let recent = t.recent(5);
        assert_eq!(recent.len(), 1);
        assert!(recent[0].block.running);
        assert_eq!(recent[0].output, "listening on :5173");
    }

    #[test]
    fn a_new_prompt_ends_a_command_the_shell_forgot_to_close() {
        let mut t = BlockTracker::default();
        let events = t.feed(format!("{}partial{}", command("make"), prompt()).as_bytes());
        let blocks = finished(&events);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].exit, None,
            "no status was reported, so none is claimed"
        );
    }

    #[test]
    fn a_status_with_no_command_running_is_ignored() {
        // Pressing Enter at an empty prompt, or Ctrl-C at one, runs nothing.
        let mut t = BlockTracker::default();
        let events = t.feed(format!("{}{}{}", prompt(), done(0), prompt()).as_bytes());
        assert!(events.is_empty());
        assert!(t.recent(5).is_empty());
        assert!(t.integrated(), "the marks still prove integration is live");
    }

    #[test]
    fn only_the_newest_blocks_are_remembered() {
        let mut t = BlockTracker::default();
        for n in 0..(MAX_BLOCKS + 5) {
            t.feed(format!("{}{}", command(&format!("echo {n}")), done(0)).as_bytes());
        }
        assert!(t.get(1).is_none());
        let recent = t.recent(MAX_BLOCKS + 50);
        assert_eq!(recent.len(), MAX_BLOCKS);
        assert_eq!(
            recent.last().unwrap().block.command,
            format!("echo {}", MAX_BLOCKS + 4)
        );
        assert_eq!(t.recent(3).len(), 3);
    }

    #[test]
    fn other_control_strings_are_left_out_of_the_output() {
        let mut t = BlockTracker::default();
        let stream = format!(
            "{}\x1b]0;window title{BEL}\x1b]8;;https://example.com{BEL}link\x1b]8;;{BEL}\r\n{}",
            command("ls"),
            done(0)
        );
        t.feed(stream.as_bytes());
        assert_eq!(t.get(1).unwrap().output, "link");
    }

    #[test]
    fn plain_output_with_no_marks_is_not_mistaken_for_integration() {
        let mut t = BlockTracker::default();
        assert!(t.feed(b"Last login: today\r\n$ ls\r\nfile\r\n").is_empty());
        assert!(!t.integrated());
    }

    #[test]
    fn zsh_s_partial_line_marker_is_not_part_of_the_output() {
        let marker = "\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m          \r \r";
        let mut t = BlockTracker::default();
        t.feed(format!("{}hi\r\n{marker}{}", command("echo hi"), done(0)).as_bytes());
        assert_eq!(t.get(1).unwrap().output, "hi");
        // Output with no newline at the end is where the marker is visible,
        // and it still isn't output.
        t.feed(format!("{}no newline{marker}{}", command("printf"), done(0)).as_bytes());
        assert_eq!(t.get(2).unwrap().output, "no newline");
        // A percent sign a command printed is left alone.
        t.feed(format!("{}100%\r\n{}", command("echo"), done(0)).as_bytes());
        assert_eq!(t.get(3).unwrap().output, "100%");
    }

    #[test]
    fn percent_decoding_is_forgiving() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
    }
}
