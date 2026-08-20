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
//!
//! # On Windows this opens a console window
//!
//! A pty here is a ConPTY, and a ConPTY is a real `conhost.exe`. `portable-pty`
//! builds its own `CreateProcessW` call rather than taking a
//! `tokio::process::Command`, so [`crate::spawn::no_console`] — the flag every
//! other child this program starts is given — has nothing to attach to. In a
//! release build the app has no console of its own
//! (`windows_subsystem = "windows"`), so one is created, and it is visible for
//! as long as the command runs.
//!
//! It does not reproduce in a development build, which has a console already,
//! and it does not reproduce in `taurus` on the terminal for the same reason.
//! That is the whole shape of the bug: it appears only in the built desktop
//! app, on the one platform, on the subset of commands the model asks for a
//! terminal for. See the known-gaps entry.

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

/// Why a pty command could not be attempted at all.
///
/// Separated from every other failure because the caller does something
/// different with it: a command that *ran* and failed is a result, and a
/// terminal that could not be opened is not the command's fault at all. The
/// second one is recoverable by running the command the ordinary way, and
/// before this existed it was reported as the command failing — so a machine
/// missing its console host turned every `pty: true` call into an error the
/// model could do nothing with.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    /// No pseudo-terminal could be opened here. Fall back to pipes.
    #[error("{0}")]
    Unavailable(String),
    /// The command was attempted and something else went wrong.
    #[error(transparent)]
    Failed(#[from] ToolError),
}

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
) -> Result<PtyOutput, PtyError> {
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
            return Err(ToolError::Canceled.into());
        }
        result = tokio::time::timeout(timeout, worker) => result,
    };

    if let Some(forward) = forward {
        forward.abort();
    }

    match outcome {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(ToolError::Failed(format!("pty task failed: {e}")).into()),
        Err(_) => {
            stop(killer_rx).await;
            Err(ToolError::Failed(format!(
                "Command timed out after {}s under a pseudo-terminal and was killed. If it was \
                 waiting for input, pass what it should read as `stdin`; if it simply needs \
                 longer, raise timeout_secs.",
                timeout.as_secs()
            ))
            .into())
        }
    }
}

/// Whether the sideloaded ConPTY runtime is beside the executable, and what is
/// missing if it is not.
///
/// Exists because the failure it detects is otherwise silent. If the bundle
/// ships without these files, `portable-pty` quietly falls back to the system's
/// `conhost.exe` and everything works — except that a console window appears on
/// every pty command, which is a thing only a user on Windows can see and only
/// in an installed build. Nothing else in the program would ever notice.
///
/// So the app asks at startup and logs the answer, and that log line is the
/// only evidence available that the packaging is right. `Ok(())` off Windows,
/// where there is nothing to sideload.
pub fn sideload_status() -> Result<(), String> {
    #[cfg(not(windows))]
    return Ok(());

    #[cfg(windows)]
    {
        let Ok(exe) = std::env::current_exe() else {
            return Err("could not locate the executable".into());
        };
        let Some(dir) = exe.parent() else {
            return Err("the executable has no directory".into());
        };

        // `conpty.dll` beside the binary, and a console host in a directory
        // named for the architecture — Microsoft's layout, not a choice made
        // here. See `scripts/conpty.mjs`.
        let mut missing = Vec::new();
        if !dir.join("conpty.dll").is_file() {
            missing.push("conpty.dll".to_string());
        }
        // Only the host for the architecture actually running matters; the
        // other is carried for the machine this build is not on.
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        };
        if !dir.join(arch).join("OpenConsole.exe").is_file() {
            missing.push(format!("{arch}\\OpenConsole.exe"));
        }

        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing.join(", "))
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
) -> Result<PtyOutput, PtyError> {
    // The one failure that means "this machine cannot do ptys" rather than
    // "this command went wrong". On Windows it is what a missing or unusable
    // console host looks like, which is a bundling mistake rather than anything
    // the user typed — so it must not read as their command failing.
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: ROWS,
            cols: COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| PtyError::Unavailable(format!("cannot open a pseudo-terminal: {e}")))?;

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
/// Every escape form is modelled by *how long it is*, because that is the only
/// thing this needs to know. An escape whose length is guessed wrong does not
/// merely survive — it spills its own tail into the output as text, which is
/// how a stray `B` appears in the middle of a sentence and why the shapes below
/// are enumerated rather than approximated:
///
/// | Form | Written | Ends at |
/// | --- | --- | --- |
/// | CSI | `ESC [ … final` | a byte in `@`–`~` |
/// | OSC | `ESC ] … ` | `BEL`, or `ESC \` |
/// | String | `ESC P`/`X`/`^`/`_` … | `BEL`, or `ESC \` |
/// | Designation | `ESC ( ) * + - . / #` + one byte | the byte after |
/// | Everything else | `ESC` + one byte | the byte after |
///
/// The designation row is the one that bit: `ESC ( B` — "G0 is US-ASCII" — is
/// three bytes, and reading it as two leaves the `B` behind. Windows emits it
/// constantly, which is why this was a Windows-looking bug in something with no
/// Windows in it.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                // A bare carriage return is a progress bar rewriting its own
                // line. Keeping it makes a transcript that overwrites itself;
                // dropping it where it precedes a newline keeps CRLF intact.
                if chars.peek() == Some(&'\n') {
                    continue;
                }
                out.push('\n');
            }
            // A terminal moves the cursor back and the next write covers what
            // was there. Applied rather than dropped, because dropping it
            // leaves the text the program meant to erase — the half-written
            // word in front of a progress counter.
            '\u{8}' => {
                if !out.ends_with('\n') {
                    out.pop();
                }
            }
            // Rings a bell nobody can hear. Dropped rather than passed on,
            // because in a transcript it is an unprintable character sitting in
            // the middle of a line.
            '\u{7}' => {}
            '\u{1b}' => match chars.next() {
                // CSI: parameters and intermediates, then one final byte.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC and the other string-carrying escapes — DCS, SOS, PM,
                // APC — all run until a string terminator. They are grouped
                // because the only thing that differs is the introducer, and
                // treating any of them as two bytes spills a whole payload into
                // the output rather than a single character.
                Some(']' | 'P' | 'X' | '^' | '_') => {
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
                // Three bytes: an introducer, then the character set or the
                // test pattern being selected.
                Some('(' | ')' | '*' | '+' | '-' | '.' | '/' | '#' | ' ') => {
                    chars.next();
                }
                // Two-byte escape; the second byte is the whole of it.
                Some(_) | None => {}
            },
            _ => out.push(c),
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
    fn a_character_set_designation_does_not_leave_its_last_byte_behind() {
        // `ESC ( B` is "G0 is US-ASCII" and is three bytes. Read as two, the
        // `B` lands in the output as text — which is what a stray capital
        // letter in the middle of a Windows command's output turned out to be.
        assert_eq!(strip_ansi("\u{1b}(Bhello"), "hello");
        assert_eq!(strip_ansi("\u{1b})0hello"), "hello");
        assert_eq!(strip_ansi("\u{1b}#8hello"), "hello");
        // The whole family, since getting one right and the rest wrong is how
        // this survived the first time.
        assert_eq!(strip_ansi("\u{1b}*Ax"), "x");
        assert_eq!(strip_ansi("\u{1b}+Ax"), "x");
    }

    #[test]
    fn a_string_carrying_escape_does_not_spill_its_payload() {
        // DCS, PM and APC end at a string terminator like OSC does. Treated as
        // two-byte escapes they leak the entire payload as text, which is a
        // worse version of the same bug.
        assert_eq!(strip_ansi("\u{1b}Psome payload\u{1b}\\hello"), "hello");
        assert_eq!(strip_ansi("\u{1b}_payload\u{1b}\\hello"), "hello");
        assert_eq!(strip_ansi("\u{1b}^payload\u{1b}\\hello"), "hello");
    }

    #[test]
    fn a_two_byte_escape_is_still_only_two_bytes() {
        // The fix must not overshoot: these have no third byte, and eating one
        // would delete a character of real output.
        assert_eq!(strip_ansi("\u{1b}7keep"), "keep");
        assert_eq!(strip_ansi("\u{1b}=keep"), "keep");
        assert_eq!(strip_ansi("\u{1b}ckeep"), "keep");
    }

    #[test]
    fn a_backspace_erases_rather_than_surviving_as_a_control_character() {
        // What a terminal shows. Dropping the backspace instead would leave
        // "abc" where the program had rewritten it to "ax".
        assert_eq!(strip_ansi("abc\u{8}\u{8}x"), "ax");
        // Never past the start of a line: a backspace at column zero moves
        // nothing, and joining two lines would be a change to the text.
        assert_eq!(strip_ansi("one\n\u{8}two"), "one\ntwo");
    }

    #[test]
    fn a_bell_does_not_reach_the_transcript() {
        assert_eq!(strip_ansi("ding\u{7}done"), "dingdone");
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
