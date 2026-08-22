//! A shell you type into, in the window the agent works in.
//!
//! [`taurus_tools::builtin::pty`] already opens pseudo-terminals, and this is
//! not that. That one runs *a command*: it writes the answers it was given up
//! front, closes the input so the child sees end-of-file, reads to EOF, strips
//! every escape sequence on the way out — because a model reading cursor moves
//! learns nothing — and hands back one string. Every one of those decisions is
//! right for a tool call and wrong for a terminal.
//!
//! So this is the other half. Input stays open for as long as the shell lives,
//! output leaves as the bytes that arrived, and the window is whatever size the
//! pane happens to be. What it produces is not a result; it is a stream that
//! ends when the shell does.
//!
//! # Bytes, not text
//!
//! Output crosses to the webview as base64 and is decoded into the emulator as
//! bytes. It would be shorter to send a string, and it would be wrong: a read
//! returns whatever the kernel had ready, which splits multi-byte characters
//! across chunk boundaries roughly whenever a terminal is busy. Decoding each
//! chunk on its own turns those into replacement characters — a box-drawing
//! frame that comes apart under `htop`, an emoji that arrives as garbage. The
//! emulator on the other side reassembles them, because it is the only thing
//! here that can see both sides of the split.
//!
//! # Why output is never dropped
//!
//! [`taurus_tools::builtin::shell`] lets a display that has fallen behind lose
//! lines rather than stall the child, which is the right bargain when the
//! authoritative copy is the string the model will read anyway. Here there is
//! no second copy, and the stream is stateful: a dropped chunk can carry the
//! half of an escape sequence that would have switched the screen back, so what
//! is lost is not one frame but every frame after it. This applies
//! backpressure instead. A reader that cannot keep up slows the shell down,
//! which is exactly what a real terminal does.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine;
use dashmap::DashMap;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::mpsc;
use tracing::{info, warn};
use ts_rs::TS;

/// How much is read from the pty at a time.
const READ_CHUNK: usize = 16 * 1024;

/// How many reads may be waiting to be forwarded.
///
/// Small, because it is a smoothing buffer and not a store: the whole point of
/// the previous note is that a backlog past this slows the shell rather than
/// growing. A few chunks is enough to absorb a burst that arrives between two
/// turns of the runtime.
const READ_BACKLOG: usize = 32;

/// How much output may be gathered into one message to the webview.
///
/// There is no timer here, unlike the agent's streaming path, and that is the
/// difference between the two. A tool call is watched; a terminal is *typed
/// into*, and a tenth of a second between a keystroke and its echo is the
/// difference between an application that feels native and one that feels
/// remote. So a chunk leaves as soon as it arrives, and coalescing only ever
/// picks up what was already queued behind it — which is nothing at all while
/// someone is typing, and thousands of lines during a build.
const COALESCE_BYTES: usize = 64 * 1024;

/// What the shell tells the pane.
#[derive(Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum TerminalEvent {
    /// Raw terminal output, base64. See the module note on why it is not text.
    Output { data: String },
    /// The shell ended, and this session is gone. A pane that receives this has
    /// nothing left to write to.
    Exited {
        /// `None` where the platform reported no code at all, which in practice
        /// means the shell was killed rather than exited.
        #[ts(optional)]
        code: Option<i32>,
    },
}

/// One live shell.
///
/// Three handles onto the same pty, split by what they are for and locked
/// separately: a resize must not wait behind a keystroke, and neither waits
/// behind the reader, which does not appear here at all — it belongs to the
/// blocking thread that owns the read loop.
struct Shell {
    /// Kept for [`MasterPty::resize`] and nothing else. Dropping it would close
    /// the terminal, so it is held for the life of the session.
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// Cloned from the child at spawn, because the child itself moves into the
    /// read loop. Without it there is no way to end a shell that is ignoring
    /// its input — and a blocking read cannot be cancelled, so killing the
    /// child is the only thing that ends one.
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

impl Shell {
    fn write(&self, bytes: &[u8]) -> Result<(), String> {
        let mut writer = lock(&self.writer);
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|e| format!("could not send input to the shell: {e}"))
    }

    fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        lock(&self.master)
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("could not resize the terminal: {e}"))
    }

    fn kill(&self) {
        let _ = lock(&self.killer).kill();
    }
}

/// Every shell this window has open, keyed by the id the pane holds.
#[derive(Default)]
pub struct Terminals {
    open: DashMap<String, Arc<Shell>>,
}

impl Terminals {
    /// Starts a shell and streams it to `events`.
    ///
    /// Answers with the id everything else here is keyed by. The shell is the
    /// user's own — `$SHELL`, or the password database when that is unset, and
    /// the console the OS names on Windows — started with no arguments, which
    /// under a pty is what makes it interactive and so what makes it read the
    /// rc file it would read in any other terminal.
    ///
    /// It is *not* started as a login shell. A login shell would re-read the
    /// profile to rebuild `PATH`, and that has already happened: the app asks
    /// the login shell for its `PATH` once at startup and adopts it, so this
    /// child inherits a repaired environment rather than repairing it again.
    /// See [`taurus_tools::login_path`].
    pub fn open(
        self: &Arc<Self>,
        cwd: &Path,
        rows: u16,
        cols: u16,
        events: Channel<TerminalEvent>,
    ) -> Result<String, String> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("could not open a terminal: {e}"))?;

        let mut builder = CommandBuilder::new_default_prog();
        builder.cwd(cwd);
        // The same two answers the tool path gives, for the same reasons: a
        // shell told nothing assumes the most primitive terminal there is, and
        // one told something exotic looks for a terminfo entry that may not be
        // on the machine. `COLORTERM` is what a modern prompt reads before it
        // will use more than sixteen colours.
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        // Convention rather than requirement, and the reason it is worth
        // setting: a prompt or a script that wants to know where it is running
        // looks here, and "unknown" is the answer that makes shell integration
        // impossible to write later.
        builder.env("TERM_PROGRAM", "Taurus");
        builder.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));

        let mut child = pair
            .slave
            .spawn_command(builder)
            .map_err(|e| format!("could not start a shell: {e}"))?;

        let killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("could not read the terminal: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("could not write to the terminal: {e}"))?;

        // The slave handle has to go before the read loop starts, or the reader
        // never sees end-of-file: this process would still be holding the
        // terminal open after the shell using it had exited.
        drop(pair.slave);

        let id = uuid::Uuid::new_v4().to_string();
        self.open.insert(
            id.clone(),
            Arc::new(Shell {
                master: Mutex::new(pair.master),
                writer: Mutex::new(writer),
                killer: Mutex::new(killer),
            }),
        );

        let (tx, rx) = mpsc::channel::<Pump>(READ_BACKLOG);

        // `portable-pty` has no async reader, so this is a blocking thread for
        // the life of the shell rather than a task. It ends when the read ends,
        // which is what `kill` is for.
        tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    // Blocking rather than dropping — see the module note.
                    // A closed channel means the pane is gone.
                    Ok(n) => {
                        if tx.blocking_send(Pump::Data(buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                }
            }
            let code = child.wait().ok().map(|status| status.exit_code() as i32);
            let _ = tx.blocking_send(Pump::Exit(code));
        });

        let terminals = self.clone();
        let closing = id.clone();
        tokio::spawn(async move {
            forward(rx, events).await;
            // Whether the shell exited or the channel went away, this session
            // can no longer be written to. Left in the map it would be an id
            // that accepts input and swallows it.
            terminals.open.remove(&closing);
        });

        info!(terminal = %id, cwd = %cwd.display(), "shell started");
        Ok(id)
    }

    pub fn write(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        self.get(id)?.write(bytes)
    }

    pub fn resize(&self, id: &str, rows: u16, cols: u16) -> Result<(), String> {
        self.get(id)?.resize(rows.max(1), cols.max(1))
    }

    /// Ends a shell. Quiet about an id that is already gone, because the two
    /// ways a pane closes — the user closing it, and the shell exiting under it
    /// — race, and neither is a failure.
    pub fn close(&self, id: &str) {
        if let Some((_, shell)) = self.open.remove(id) {
            // Logged to match the line `open` writes. The pair is what makes a
            // leaked shell — one opened and never closed — visible in a log
            // rather than only in a process list.
            info!(terminal = %id, "shell closed");
            shell.kill();
        }
    }

    /// Ends every shell this window has open.
    ///
    /// Called when the window goes away. A pty child is not in this process's
    /// tree in any way the OS will clean up for us, so without this a closed
    /// window leaves a shell — and anything it was running — alive with nothing
    /// attached to it.
    pub fn close_all(&self) {
        let ids: Vec<String> = self.open.iter().map(|e| e.key().clone()).collect();
        if !ids.is_empty() {
            info!(count = ids.len(), "closing shells");
        }
        for id in ids {
            self.close(&id);
        }
    }

    fn get(&self, id: &str) -> Result<Arc<Shell>, String> {
        self.open
            .get(id)
            .map(|e| e.clone())
            // Reachable in ordinary use: a shell that exits while a keystroke
            // is in flight removes itself, and the pane finds out one message
            // later. So this says what happened rather than naming an id.
            .ok_or_else(|| "that terminal has closed".to_string())
    }
}

/// What the read loop hands to the forwarder.
enum Pump {
    Data(Vec<u8>),
    Exit(Option<i32>),
}

/// Moves output to the webview, gathering up whatever has already queued.
///
/// The gathering is the only reason this is not a two-line loop. A build
/// produces output far faster than a webview can be messaged, and one message
/// per read would put tens of thousands of them through the IPC channel to draw
/// text that arrives in the same frame regardless.
async fn forward(mut rx: mpsc::Receiver<Pump>, events: Channel<TerminalEvent>) {
    let mut held: Vec<u8> = Vec::new();
    while let Some(next) = rx.recv().await {
        match next {
            Pump::Data(chunk) => {
                held.extend_from_slice(&chunk);
                // Only what is already waiting; this never waits for more.
                while held.len() < COALESCE_BYTES {
                    match rx.try_recv() {
                        Ok(Pump::Data(more)) => held.extend_from_slice(&more),
                        Ok(Pump::Exit(code)) => {
                            send(&events, &mut held);
                            let _ = events.send(TerminalEvent::Exited { code });
                            return;
                        }
                        Err(_) => break,
                    }
                }
                if !send(&events, &mut held) {
                    return;
                }
            }
            Pump::Exit(code) => {
                send(&events, &mut held);
                let _ = events.send(TerminalEvent::Exited { code });
                return;
            }
        }
    }
}

/// Sends what has been gathered, and says whether anyone was listening.
fn send(events: &Channel<TerminalEvent>, held: &mut Vec<u8>) -> bool {
    if held.is_empty() {
        return true;
    }
    let data = base64::engine::general_purpose::STANDARD.encode(&held[..]);
    held.clear();
    match events.send(TerminalEvent::Output { data }) {
        Ok(()) => true,
        Err(e) => {
            // The window closed, or the pane was torn down with output still
            // in flight. The shell is killed by whoever removed it; there is
            // nothing to do here but stop.
            warn!(error = %e, "terminal output had nowhere to go");
            false
        }
    }
}

/// A lock that a panicking holder cannot take the terminal down with.
///
/// Nothing under these locks can panic — a write, a resize, a kill — so
/// poisoning would only ever be inherited from somewhere else. Recovering the
/// value keeps a terminal usable rather than turning one unrelated panic into a
/// pane that refuses every keystroke afterwards.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// A channel that records what it is sent.
    ///
    /// `Channel::new` takes the serialized body rather than the event, so this
    /// matches on the JSON. Coarse, and the right grain for what these tests
    /// are about: that a shell starts, speaks, and is heard ending.
    fn recorder() -> (Channel<TerminalEvent>, Arc<Mutex<Vec<String>>>) {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                lock(&sink).push(json);
            }
            Ok(())
        });
        (channel, seen)
    }

    /// Waits for `wanted` to appear in what the channel has been sent.
    async fn until(seen: &Arc<Mutex<Vec<String>>>, wanted: &str) -> bool {
        for _ in 0..200 {
            if lock(seen).iter().any(|line| line.contains(wanted)) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_shell_runs_what_it_is_typed_and_is_heard_when_it_ends() {
        let terminals = Arc::new(Terminals::default());
        let (channel, seen) = recorder();
        let id = terminals
            .open(&std::env::temp_dir(), 24, 80, channel)
            .expect("a shell must start");

        // Buffered by the pty until the shell is ready to read it, so there is
        // no startup to wait for here.
        terminals
            .write(&id, b"exit\n")
            .expect("a live shell must take input");

        assert!(
            until(&seen, "\"kind\":\"exited\"").await,
            "the shell ended and nothing said so: {:?}",
            lock(&seen),
        );
        // Removed by the forwarder as it finishes, so the id stops accepting
        // input that would go nowhere.
        for _ in 0..20 {
            if terminals.write(&id, b"x").is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("a shell that has exited is still taking keystrokes");
    }

    #[tokio::test]
    async fn resizing_a_live_shell_is_accepted() {
        let terminals = Arc::new(Terminals::default());
        let (channel, _seen) = recorder();
        let id = terminals
            .open(&std::env::temp_dir(), 24, 80, channel)
            .expect("a shell must start");

        terminals.resize(&id, 40, 120).expect("a resize must land");
        // Zero columns is what a pane laid out to nothing measures as, and it
        // is a size no terminal can have.
        terminals.resize(&id, 0, 0).expect("a collapsed pane is clamped");

        terminals.close(&id);
    }

    #[tokio::test]
    async fn typing_into_a_terminal_that_has_gone_says_so_readably() {
        let terminals = Arc::new(Terminals::default());
        // The id of a shell that exited a moment ago looks exactly like one
        // that never existed, and the message has to work for both.
        let err = terminals
            .write("not-a-terminal", b"ls\n")
            .expect_err("an unknown terminal cannot take input");
        assert_eq!(err, "that terminal has closed");
    }

    #[tokio::test]
    async fn closing_a_terminal_twice_is_not_an_error() {
        let terminals = Arc::new(Terminals::default());
        let (channel, _seen) = recorder();
        let id = terminals
            .open(&std::env::temp_dir(), 24, 80, channel)
            .expect("a shell must start");

        // The pane closing and the shell exiting race, and neither is a
        // failure — so the second one through must be quiet.
        terminals.close(&id);
        terminals.close(&id);
        terminals.close_all();
    }

    #[tokio::test]
    async fn closing_the_window_ends_every_shell() {
        let terminals = Arc::new(Terminals::default());
        let mut ids = Vec::new();
        let mut sinks = Vec::new();
        for _ in 0..3 {
            let (channel, seen) = recorder();
            ids.push(
                terminals
                    .open(&std::env::temp_dir(), 24, 80, channel)
                    .expect("a shell must start"),
            );
            sinks.push(seen);
        }

        terminals.close_all();

        // Killed rather than merely forgotten: a shell left running with
        // nothing attached to it is the whole reason this exists.
        for seen in &sinks {
            assert!(
                until(seen, "\"kind\":\"exited\"").await,
                "a shell outlived the window that opened it",
            );
        }
        assert!(terminals.open.is_empty());
    }

    #[test]
    fn a_poisoned_lock_does_not_take_the_terminal_with_it() {
        let held = Arc::new(Mutex::new(7));
        let poisoned = Arc::new(AtomicBool::new(false));
        let other = held.clone();
        let flag = poisoned.clone();
        let _ = std::thread::spawn(move || {
            let _guard = other.lock().expect("first lock");
            flag.store(true, Ordering::SeqCst);
            panic!("poisoning on purpose");
        })
        .join();

        assert!(poisoned.load(Ordering::SeqCst));
        assert_eq!(*lock(&held), 7, "a keystroke must survive someone else's panic");
    }
}
