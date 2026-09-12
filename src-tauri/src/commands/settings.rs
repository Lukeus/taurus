//! Themes, the window, the code index, and reloading config.

use super::*;

#[tauri::command]
pub async fn set_theme(state: State<'_, Arc<AppState>>, theme: Theme) -> CmdResult<()> {
    state.host.set_theme(theme).await;
    // The settings file stays the authority on which theme is in force, and
    // this is how the window is told what it now says.
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn set_theme_id(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<()> {
    state.host.set_theme_id(id).await;
    emit_status(&state).await;
    Ok(())
}

/// Every custom theme on the machine, for the picker.
///
/// Its own command rather than a field on the status, because each theme
/// carries its logo inlined and the status is pushed after anything that moves
/// a number on screen. The picker asks once, when it opens.
#[tauri::command]
pub async fn list_themes(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<CustomTheme>> {
    let themes = state.host.themes().await;
    // The scan records what would not load, and that is a thing the Settings
    // screen shows — so the window has to be told the problem list changed.
    emit_status(&state).await;
    Ok(themes)
}

#[tauri::command]
pub async fn save_theme(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    id: String,
    theme: ThemeFile,
) -> CmdResult<String> {
    let path = state.host.save_theme(scope, &id, &theme).await?;
    emit_status(&state).await;
    Ok(path)
}

#[tauri::command]
pub async fn delete_theme(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    id: String,
) -> CmdResult<()> {
    state.host.delete_theme(scope, &id).await?;
    emit_status(&state).await;
    Ok(())
}

/// Creates a layer's themes directory and returns it, so the UI can offer to
/// open the folder on a machine that has never had one.
#[tauri::command]
pub async fn themes_dir(state: State<'_, Arc<AppState>>, scope: Scope) -> CmdResult<String> {
    state.host.themes_dir(scope).await
}

/// Repaints the native window behind the webview.
///
/// The stylesheet cannot reach this: it is the platform's own ground, what
/// shows for the frame before the document paints and in whatever the webview
/// does not cover. `paint_window` sets it at launch from the settings file, and
/// this is what moves it when a theme is picked while the app is running.
///
/// The colour comes from the frontend rather than being re-derived here,
/// because the frontend is the only place that knows the *resolved* answer — a
/// theme that names no ink inherits the stylesheet's, and the stylesheet is not
/// something Rust can read.
#[tauri::command]
pub fn set_window_background(window: tauri::WebviewWindow, color: String) -> CmdResult<()> {
    if let Some(parsed) = crate::parse_color(&color) {
        let _ = window.set_background_color(Some(parsed));
    }
    // A colour that will not parse costs a mismatched edge, which is not worth
    // failing a theme change over — the same call this sits beside has already
    // repainted everything the user can actually see.
    Ok(())
}

#[tauri::command]
pub async fn set_embedding_model(
    state: State<'_, Arc<AppState>>,
    model: String,
    provider: String,
) -> CmdResult<()> {
    state.host.set_embedding_model(&model, &provider).await;
    // The tool is registered by the local half of a reload, so without this a
    // model named here does not become a `search_code` until the next
    // workspace change — which reads as the setting not having taken. Only
    // that half: an embedding model changes nothing an MCP server is running
    // with.
    state.host.reload_local().await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn set_rerank(
    state: State<'_, Arc<AppState>>,
    model: String,
    provider: String,
) -> CmdResult<()> {
    state.host.set_rerank(&model, &provider).await;
    // Same reason `set_embedding_model` reloads, and the same half: the
    // reranker is attached to `search_code` when the tool is registered, so
    // without this it does not take hold until the next workspace change.
    state.host.reload_local().await;
    emit_status(&state).await;
    Ok(())
}

/// How far through a build is, as the UI draws it.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct IndexProgress {
    pub done: usize,
    pub total: usize,
}

/// Builds this workspace's semantic index now, rather than inside the first
/// turn that reaches for it.
///
/// The whole point is that it is not a turn: the first index of a repository
/// takes the better part of a minute, and paying it here means paying it
/// against a progress bar that someone chose to start, instead of inside a tool
/// call that has not returned.
///
/// A `Channel` for the same reason `send_message` uses one — delivery is
/// ordered and scoped to this call, so a second window building a different
/// workspace cannot interleave into this one's bar.
#[tauri::command]
pub async fn build_index(
    state: State<'_, Arc<AppState>>,
    on_progress: Channel<IndexProgress>,
) -> CmdResult<String> {
    struct ToChannel(Channel<IndexProgress>);

    #[async_trait::async_trait]
    impl taurus_host::IndexProgress for ToChannel {
        async fn embedding(&self, done: usize, total: usize) {
            // A dropped channel means the settings pane closed. The build is
            // still worth finishing — the index is the point, not the bar.
            let _ = self.0.send(IndexProgress { done, total });
        }
    }

    // Its own token rather than a session's: this belongs to no conversation,
    // and cancelling it must not cancel a turn that happens to be running.
    let cancel = CancellationToken::new();
    *state.index_build.lock().await = cancel.clone();

    state
        .host
        .build_index(cancel, Some(&ToChannel(on_progress)))
        .await
}

/// Stops a running index build. Safe to call when none is running.
#[tauri::command]
pub async fn stop_index_build(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.index_build.lock().await.cancel();
    Ok(())
}

/// Re-reads both config layers, rescans skills and agents, and reconnects MCP
/// servers.
///
/// Named for what it does. It was `reload_skills`, which promised less than it
/// delivered from the day agents were discovered by the same call: someone who
/// had edited an agent would not press it, and someone who pressed it would not
/// expect their agent edits to land.
#[tauri::command]
pub async fn reload_config(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.host.reload().await;
    // The broadest of these: every count, every server and every problem the
    // shell shows is re-read by that call.
    emit_status(&state).await;
    Ok(())
}

/// Re-reads the files a person edits in an editor: instructions, skills,
/// sub-agents, hooks.
///
/// What returning to the window calls. These are the four things somebody
/// writes in another application and then expects to find here, and coming back
/// from that application is the closest thing to an event their arrival has —
/// nothing watches those directories, deliberately.
///
/// The same gate a turn uses, so this is a `stat` per file when nothing moved
/// rather than a rescan on every alt-tab. Much narrower than [`reload_config`]
/// either way: that one also re-reads both provider layers and reconnects every
/// MCP server.
///
/// Refuses nothing, but the caller is expected not to ask mid-turn — see
/// `Host::refresh_config`.
#[tauri::command]
pub async fn rescan_library(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    // Runs on every return to the window, and a status is a dozen reads and a
    // git branch lookup. Pushed only when something on disk moved.
    if state.host.refresh_config().await {
        emit_status(&state).await;
    }
    Ok(())
}
