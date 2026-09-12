//! Background commands, the terminal dock, and the window asking for attention.

use super::*;

/// The background commands, and the output of the one being watched.
///
/// `cursor` is `0` for a first look, and otherwise the one the last answer
/// carried. It is the pane's own place in the stream: `check_command` keeps a
/// separate one, so neither reader takes lines from the other. See
/// [`taurus_tools::Jobs`].
#[tauri::command]
pub async fn background(
    state: State<'_, Arc<AppState>>,
    watching: Option<u32>,
    cursor: usize,
) -> CmdResult<Background> {
    let jobs = state.host.jobs();
    // An id that is not in the list is not an error here. The pane polls, and
    // a command can go between the tick that drew the tab and the tick that
    // reads it.
    let output = watching.and_then(|id| state.host.job_output(id, cursor).ok());
    Ok(Background { jobs, output })
}

/// Ends one background command from the window.
///
/// The same stop `stop_command` gives the model, so a build the user ends and
/// a build the model ends leave the same trace: the job reports itself
/// stopped, and what it changed is swept into the running turn.
#[tauri::command]
pub async fn background_stop(state: State<'_, Arc<AppState>>, id: u32) -> CmdResult<String> {
    let stopped = state.host.stop_job(id).await?;
    info!(job = id, "stopped a background command from the window");
    Ok(stopped)
}

/* ------------------------------------------------------------- terminal */

/// Starts a shell and streams it to the pane that asked.
///
/// `cwd` is the workspace root when the pane does not name one, which is what
/// it wants on the first open: a terminal that starts in the home directory
/// beside a window that is looking at a project is one `cd` away from being
/// useful and nobody remembers to type it.
///
/// The size is the pane's, measured after it is laid out. A terminal opened at
/// a guessed size and corrected a moment later shows the shell's first prompt
/// wrapped at the wrong column, which is the one artifact of a resize that does
/// not redraw away.
#[tauri::command]
pub async fn terminal_open(
    state: State<'_, Arc<AppState>>,
    cwd: Option<String>,
    rows: u16,
    cols: u16,
    on_event: Channel<TerminalEvent>,
) -> CmdResult<String> {
    let root = state.host.workspace().await;
    let cwd = match cwd {
        Some(path) => PathBuf::from(path),
        None => root.clone(),
    };
    // A folder that has been renamed or unmounted under the window would
    // otherwise fail inside the spawn as a message about the shell, which is
    // the wrong thing to name.
    let cwd = if cwd.is_dir() { cwd } else { root };
    state.terminals.open(&cwd, rows, cols, on_event)
}

/// Sends keystrokes. `data` is the text the emulator produced, escape
/// sequences and all — arrow keys and Ctrl chords arrive here as the bytes a
/// terminal would have sent.
///
/// Not async, and that is the point. Tauri runs a plain command where its
/// message arrives, one after another, so keystrokes reach the shell in the
/// order they were typed; an async command is spawned per message, and two of
/// them are free to land in either order. It can afford to be plain because it
/// only queues the bytes — the write itself happens on the shell's own input
/// thread, and nothing here waits for a program to read. See `terminal::Shell`.
#[tauri::command]
pub fn terminal_write(state: State<'_, Arc<AppState>>, id: String, data: String) -> CmdResult<()> {
    state.terminals.write(&id, data.as_bytes())
}

/// Credits output the pane has drawn, so the shell can send more.
///
/// Plain rather than async because it only moves a counter. See
/// `terminal::Credit` for why the pane has to say so at all.
#[tauri::command]
pub fn terminal_ack(state: State<'_, Arc<AppState>>, id: String, bytes: usize) {
    state.terminals.ack(&id, bytes);
}

/// Tells the shell how big its window is now.
///
/// This is what makes a full-screen program redraw at the new size, and it is
/// also the only thing that tells a shell where to wrap. A pane that resizes
/// without saying so leaves every program inside it drawing to the old
/// geometry.
#[tauri::command]
pub async fn terminal_resize(
    state: State<'_, Arc<AppState>>,
    id: String,
    rows: u16,
    cols: u16,
) -> CmdResult<()> {
    state.terminals.resize(&id, rows, cols)
}

/// Ends a shell, and anything it is running.
#[tauri::command]
pub async fn terminal_close(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<()> {
    state.terminals.close(&id);
    Ok(())
}

/// Says, to the desktop rather than to the window, that the turn needs a person.
///
/// The one thing a webview cannot do for itself, and the gap it closes is the
/// whole reason a turn against a local model is worth walking away from: a
/// permission dialog or a question card parks the turn indefinitely, and until
/// this there was nothing outside the window saying so. Somebody alt-tabs, the
/// model finishes its third tool call, and the run waits until they happen to
/// look back.
///
/// Two signals, because they answer different questions. `waiting` is a count
/// and it persists — a badge on the dock icon that says *something is still
/// owed*, readable at any point later. `ring` is an event and it fires once —
/// a bounce or a taskbar flash at the moment the turn stops being able to
/// continue on its own. Sending the count every time and the ring only on the
/// transition is what keeps the second from becoming noise.
///
/// The frontend decides what counts, because it is what knows: this carries
/// the answer to the OS and makes no judgement of its own. It does not gate on
/// focus either — `request_user_attention` is documented as a no-op while the
/// application is focused, so the platform already holds that rule and holding
/// it twice would mean two places to get it wrong.
///
/// ## Platform-specific
///
/// - **Windows:** no badge. `set_badge_count` is unsupported there and the
///   documented substitute is an overlay icon, which means shipping a rendered
///   image per count. The taskbar flash is the signal that carries, and it
///   works.
/// - **Linux:** the badge needs a desktop with `libunity`; where there is
///   none it is quietly dropped.
///
/// Errors are logged and swallowed. A dock badge that cannot be set is not a
/// reason to fail the call that noticed the turn had parked.
#[tauri::command]
pub async fn attention(window: tauri::WebviewWindow, waiting: u32, ring: bool) -> CmdResult<()> {
    // `Some(0)` and `None` both clear it; `None` is the spelling that says so.
    let badge = (waiting > 0).then_some(i64::from(waiting));
    if let Err(e) = window.set_badge_count(badge) {
        info!("cannot set the dock badge: {e}");
    }
    if ring {
        // Informational rather than Critical: on macOS Critical bounces until
        // the app is activated, which is the right level for a crash and the
        // wrong one for "your turn wants a yes". This bounces once.
        if let Err(e) = window.request_user_attention(Some(tauri::UserAttentionType::Informational))
        {
            info!("cannot ask for attention: {e}");
        }
    }
    Ok(())
}
