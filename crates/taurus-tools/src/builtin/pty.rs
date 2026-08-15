//! Running a command under a pseudo-terminal.
//!
//! The default path in [`super::shell`] gives a child three pipes and no
//! terminal, which is right for almost everything an agent runs and wrong for a
//! specific, common minority. A program that asks `isatty` and hears "no"
//! behaves like one being piped into a file: `git` pages nothing and colors
//! nothing, `npm create` refuses its scaffolding prompts outright, and anything
//! built on a full-screen prompt library fails at startup rather than running.
//! Those are not exotic commands. They are the ones a person would reach for.
//!
//! A pty is the answer, and it is a different thing from *interactivity*. Under
//! a pty a program that wants an answer still waits for one, so the two arrive
//! together: the caller can hand over the keystrokes up front. That is the whole
//! interface — a terminal to be seen through, and a script to be read from.
//!
//! What it costs is that stdout and stderr become one stream. A terminal has
//! only one, so the split the model reads elsewhere is not recoverable here.
//! That is a property of the thing, not of this implementation.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio::sync::mpsc;

use crate::tool::{ToolError, ToolProgress};

/// The window a program is told it has.
///
/// Not the user's real terminal, which the harness does not know and which the
/// desktop app does not have. Fixed and roomy instead, so a tool that wraps its
/// output to the width wraps at a width that reads well in a transcript rather
/// than at the 80 columns it would assume from silence.
const ROWS: u16 = 40;
const COLS: u16 = 120;

/// What a finished pty command produced.
#[derive(Debug)]
pub struct PtyOutput {
    /// stdout and stderr interleaved, as a terminal would show them.
    pub text: String,
    pub exit_code: Option<i32>,
}

/// Runs `command` under a pseudo-terminal.
///
/// Blocking work — spawning, reading, waiting — is pushed onto the blocking
/// pool, because `portable-pty` is a synchronous API and its reader has no
/// async form. Holding it on a runtime thread would stall every other task in
/// the process for as long as the command ran.
pub async fn run(
    program: impl AsRef<str>,
    args: &[impl AsRef<str>],
    cwd: &std::path::Path,
    stdin: Option<String>,
    timeout: Duration,
    cancel: tokio_util::sync::CancellationToken,
    progress: Option<Arc<dyn ToolProgress>>,
) -> Result<PtyOutput, ToolError> {
    let mut builder = CommandBuilder::new(program.as_ref());
    for arg in args {
        builder.arg(arg.as_ref());
    }
    builder.cwd(cwd);
    // Named so the child believes in a terminal it can actually drive. Left
    // unset, a curses program assumes the most primitive terminal there is and
    // either degrades or refuses; set to something exotic, it looks for a
    // terminfo entry that may not exist on the machine.
    builder.env("TERM", "xterm-256color");

    let (tx, rx) = mpsc::channel::<String>(super::shell::STREAM_BACKLOG);
    let forward =
        progress.map(|progress| tokio::spawn(super::shell::batch_to_progress(rx, progress)));

    // Handed back before the worker blocks, and the reason this is not simply
    // an `abort()` on the task: a blocking task cannot be cancelled. Left to
    // itself, a worker parked in `read` on a child that will never exit holds a
    // thread until the process ends — and the runtime will not shut down while
    // it does, so a single hung command outlives the session that started it.
    // Killing the child is what ends the read.
    let (killer_tx, killer_rx) = tokio::sync::oneshot::channel();
    let worker = tokio::task::spawn_blocking(move || pump(builder, stdin, tx, killer_tx));

    // `worker` is consumed by the timeout arm, so cancellation cannot also
    // await it. Aborting the handle detaches this task from the thread; the
    // kill is what actually stops the work behind it.
    let handle = worker.abort_handle();
    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            stop(killer_rx).await;
            handle.abort();
            if let Some(forward) = forward {
                forward.abort();
            }
            return Err(ToolError::Canceled);
        }
        result = tokio::time::timeout(timeout, worker) => result,
    };

    if let Some(forward) = forward {
        forward.abort();
    }

    match outcome {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(ToolError::Failed(format!("pty task failed: {e}"))),
        Err(_) => {
            stop(killer_rx).await;
            Err(ToolError::Failed(format!(
                "Command timed out after {}s under a pseudo-terminal and was killed. If it was \
                 waiting for input, pass what it should read as `stdin`; if it simply needs \
                 longer, raise timeout_secs.",
                timeout.as_secs()
            )))
        }
    }
}

/// Kills the child, if it got far enough to have one.
///
/// The receiver resolves as soon as the process is spawned, so by the time
/// there is anything to time out there is something to kill. A closed channel
/// means the command never started, and there is nothing to do.
async fn stop(
    killer: tokio::sync::oneshot::Receiver<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
) {
    if let Ok(mut killer) = killer.await {
        let _ = killer.kill();
    }
}

/// The whole synchronous lifecycle, on a blocking thread.
fn pump(
    builder: CommandBuilder,
    stdin: Option<String>,
    tx: mpsc::Sender<String>,
    killer_tx: tokio::sync::oneshot::Sender<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
) -> Result<PtyOutput, ToolError> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: ROWS,
            cols: COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| ToolError::Failed(format!("cannot open a pseudo-terminal: {e}")))?;

    let mut child = pair
        .slave
        .spawn_command(builder)
        .map_err(|e| ToolError::Failed(format!("cannot start command: {e}")))?;

    // Before anything that can block, so a timeout always has a way to end it.
    let _ = killer_tx.send(child.clone_killer());

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| ToolError::Failed(format!("cannot read the pseudo-terminal: {e}")))?;

    // Written before reading starts, and the writer dropped immediately after.
    // A program waiting on input needs the end-of-file as much as the bytes:
    // holding the write side open is how a `cat` with nothing more to read
    // hangs until the timeout.
    if let Some(text) = stdin {
        if let Ok(mut writer) = pair.master.take_writer() {
            let _ = writer.write_all(text.as_bytes());
            let _ = writer.flush();
        }
    }

    // The slave handle has to go before reading, or the reader never sees EOF:
    // this process would still be holding a terminal open after the child that
    // was using it exited.
    drop(pair.slave);

    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                // Never blocks: a display that has fallen behind loses lines
                // rather than stalling the child, the same bargain the piped
                // path makes.
                let _ = tx.try_send(strip_ansi(&String::from_utf8_lossy(&buf[..n])));
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| ToolError::Failed(format!("cannot wait for the command: {e}")))?;

    Ok(PtyOutput {
        text: strip_ansi(&String::from_utf8_lossy(&raw)),
        // `portable-pty` reports one unsigned code on every platform rather
        // than a signal, so a killed child arrives here as its shell's code.
        exit_code: Some(status.exit_code() as i32),
    })
}

/// Removes terminal control sequences from output.
///
/// The cost of asking for a terminal: the program now writes colour, cursor
/// moves, and progress-bar redraws, none of which mean anything to a model
/// reading a transcript and all of which it pays tokens for. A `cargo build`
/// under a pty is more escape bytes than text.
///
/// Handles the two forms that carry the payload — CSI (`ESC [ … final`) and OSC
/// (`ESC ] … BEL` or `ST`) — plus the two-byte escapes, and drops carriage
/// returns used to overwrite a line. Anything else passes through, because the
/// job is to remove noise, not to be a terminal emulator.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\r' {
            // A bare carriage return is a progress bar rewriting its own line.
            // Keeping it makes a transcript that overwrites itself; dropping it
            // where it precedes a newline keeps CRLF intact.
            if chars.peek() == Some(&'\n') {
                continue;
            }
            out.push('\n');
            continue;
        }
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters and intermediates, then one final byte.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: a string terminated by BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Two-byte escape; the second byte is the whole of it.
            Some(_) | None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_codes_are_removed_but_the_words_are_not() {
        // A `cargo build` under a pty is more escape bytes than text, and the
        // model pays tokens for every one of them.
        assert_eq!(strip_ansi("\u{1b}[32mpassed\u{1b}[0m: 12"), "passed: 12");
    }

    #[test]
    fn a_progress_bar_does_not_overwrite_the_transcript() {
        // A bare carriage return means "redraw this line". Kept, it produces a
        // transcript that scrolls over itself.
        assert_eq!(strip_ansi("50%\r100%\r"), "50%\n100%\n");
    }

    #[test]
    fn crlf_survives_intact() {
        // Otherwise every Windows command's output gains a blank line per line.
        assert_eq!(strip_ansi("one\r\ntwo\r\n"), "one\ntwo\n");
    }

    #[test]
    fn a_window_title_sequence_is_removed_whole() {
        // OSC carries a string, so stopping at the first final byte the way CSI
        // does would leave the title text in the output.
        assert_eq!(strip_ansi("\u{1b}]0;my title\u{7}done"), "done");
        assert_eq!(strip_ansi("\u{1b}]0;my title\u{1b}\\done"), "done");
    }

    #[test]
    fn cursor_movement_leaves_no_residue() {
        assert_eq!(strip_ansi("a\u{1b}[2K\u{1b}[1Gb"), "ab");
    }

    #[test]
    fn ordinary_text_is_untouched() {
        // The job is removing noise, not emulating a terminal.
        let text = "fn main() { println!(\"[ok] 100% — done\"); }\n";
        assert_eq!(strip_ansi(text), text);
    }

    #[tokio::test]
    async fn a_command_sees_a_terminal() {
        // The whole point. Under the piped path this prints "not a tty".
        if cfg!(windows) {
            return;
        }
        let out = run(
            "/bin/sh",
            &["-c", "test -t 1 && echo tty || echo pipe"],
            std::path::Path::new("."),
            None,
            Duration::from_secs(10),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect("the command runs");
        assert!(out.text.contains("tty"), "{:?}", out.text);
        assert_eq!(out.exit_code, Some(0));
    }

    #[tokio::test]
    async fn supplied_input_reaches_the_command() {
        // What makes a pty useful rather than merely accurate: a program that
        // waits for an answer can be given one up front.
        if cfg!(windows) {
            return;
        }
        let out = run(
            "/bin/sh",
            &["-c", "read answer; echo \"got:$answer\""],
            std::path::Path::new("."),
            Some("yes\n".into()),
            Duration::from_secs(10),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect("the command runs");
        assert!(out.text.contains("got:yes"), "{:?}", out.text);
    }

    #[tokio::test]
    async fn a_failing_command_reports_its_code_rather_than_erroring() {
        // A non-zero exit is information for the model, not a harness failure —
        // the same rule the piped path follows.
        if cfg!(windows) {
            return;
        }
        let out = run(
            "/bin/sh",
            &["-c", "exit 3"],
            std::path::Path::new("."),
            None,
            Duration::from_secs(10),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect("a non-zero exit is not an error here");
        assert_eq!(out.exit_code, Some(3));
    }

    #[tokio::test]
    async fn a_command_that_waits_forever_is_stopped_by_the_timeout() {
        // The gap this feature could otherwise widen: under a pty an
        // interactive program waits rather than hitting EOF, so the ceiling has
        // to hold or a session hangs for good.
        if cfg!(windows) {
            return;
        }
        let err = run(
            "/bin/sh",
            &["-c", "read answer"],
            std::path::Path::new("."),
            None,
            Duration::from_millis(400),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect_err("waiting for input must not hang the session");
        let message = err.to_string();
        assert!(message.contains("timed out"), "{message}");
        // And it says what to do about it.
        assert!(message.contains("stdin"), "{message}");
    }
}
