//! The open folder and whether its config is trusted.

use super::*;

#[tauri::command]
pub async fn get_status(state: State<'_, Arc<AppState>>) -> CmdResult<AppStatus> {
    // The frontend asks for this once, on mount, and keeps what it gets. That
    // races the startup reload, and losing the race meant a permanent `0`
    // beside a drawer full of skills — see [`AppState::loaded`]. Every later
    // change arrives on `EVENT_STATUS` rather than by being asked for again.
    state.loaded().await;
    Ok(status_of(&state).await)
}

#[tauri::command]
pub async fn set_workspace(state: State<'_, Arc<AppState>>, path: String) -> CmdResult<String> {
    // Refused here rather than only in the window. The move reconnects every
    // MCP server, so a turn running through it starts failing mid-call, and a
    // turn is no longer something one screen can account for: it keeps running
    // in a conversation the window has moved on from. The rail disables the
    // button while anything is working; this is what makes that a rule.
    let running = running_ids(&state).await;
    if !running.is_empty() {
        return Err(format!(
            "{} still running. Stop it before switching folders.",
            match running.len() {
                1 => "A turn is".to_string(),
                many => format!("{many} turns are"),
            }
        ));
    }

    let resolved = state.host.set_workspace(&PathBuf::from(path)).await?;
    let shown = taurus_tools::path_guard::plain(&resolved);
    info!(workspace = %shown.display(), "workspace changed");
    // Everything the shell shows about the app belongs to the folder, so all of
    // it has just changed at once.
    emit_status(&state).await;
    // The new folder's servers, once the shell has everything else.
    reconnect_mcp(&state);
    Ok(shown.display().to_string())
}

/// Reconnects the MCP servers in the background, then tells the window.
///
/// For the commands that change which servers apply — a folder switch, a trust
/// decision — and that return as soon as everything else has taken effect.
/// Startup splits a reload the same way and for the same reason: a server can
/// take seconds to start, and a folder with three of them must not hold the
/// rail on the old folder until the last one answers. See `lib.rs`.
pub(super) fn reconnect_mcp(state: &Arc<AppState>) {
    let state = state.clone();
    tauri::async_runtime::spawn(async move {
        state.host.reload_mcp().await;
        emit_status(&state).await;
    });
}

/// Whether this workspace's own config is being read, and what it holds.
///
/// Polled by the app rather than pushed, because the answer changes when a file
/// appears in a directory nobody is watching — a `git pull` that adds
/// `.taurus/mcp.json` is exactly the case the gate exists for, and it arrives
/// with no event attached to it. See [`taurus_host::trust`].
#[tauri::command]
pub async fn workspace_trust(state: State<'_, Arc<AppState>>) -> CmdResult<TrustStatus> {
    Ok(state.host.trust_status().await)
}

#[tauri::command]
pub async fn trust_workspace(state: State<'_, Arc<AppState>>) -> CmdResult<TrustStatus> {
    state.host.trust_workspace().await?;
    let status = state.host.trust_status().await;
    info!(workspace = %status.workspace, "workspace trusted");
    // Saying yes is what loads this project's skills, agents and servers; the
    // counts on the rail move with it, and again when its servers answer.
    emit_status(&state).await;
    reconnect_mcp(&state);
    Ok(status)
}

#[tauri::command]
pub async fn revoke_workspace_trust(state: State<'_, Arc<AppState>>) -> CmdResult<TrustStatus> {
    state.host.revoke_trust().await?;
    let status = state.host.trust_status().await;
    info!(workspace = %status.workspace, "workspace trust revoked");
    emit_status(&state).await;
    // Which is what shuts down a server this project started.
    reconnect_mcp(&state);
    Ok(status)
}
