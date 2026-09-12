//! MCP servers: the panel, the catalogue, sign-in and reconnecting.

use super::*;

/// Opens a layer's `mcp.json` in whatever the OS uses for it, creating it first
/// if it is not there yet.
///
/// Still here now the panel exists, and deliberately. The format is the one
/// Claude Desktop uses and entries get pasted between the two, so the file has
/// to stay the authority: anything the panel cannot express — a key from a
/// newer version of the format, a comment, a server mid-edit — is edited here.
/// Every write the panel makes preserves what it does not understand, so the two
/// routes can be used interchangeably.
#[tauri::command]
pub async fn open_mcp_config(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    scope: Scope,
) -> CmdResult<String> {
    let workspace = state.host.workspace().await;
    let path = taurus_host::config::ensure_mcp_file(scope, Some(&workspace))?;

    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Every configured server, merged across layers, with how it is doing.
#[tauri::command]
pub async fn list_mcp_servers(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<McpServerView>> {
    // The same wait `get_status` takes, for the same reason: opening the panel
    // during the first load would otherwise render an empty list over a
    // `mcp.json` full of servers.
    state.loaded().await;
    // The listing reads `mcp.json` again, and a file broken since the last
    // reload is news only if the window is told. Pushed only when the MCP
    // problems moved, so opening the panel does not recompute the status on
    // every open for nothing.
    let before = state
        .host
        .problems_from(&[taurus_host::ProblemSource::Mcp])
        .await;
    let servers = state.host.mcp_servers().await;
    if state
        .host
        .problems_from(&[taurus_host::ProblemSource::Mcp])
        .await
        != before
    {
        emit_status(&state).await;
    }
    Ok(servers)
}

/// Where the app looks for a stdio server's program, and what it took to get
/// there.
///
/// Shown in the panel rather than only logged. "Command not found" for a command
/// that plainly exists is the single most confusing thing this feature does, and
/// it is entirely explained by a PATH the user cannot otherwise see.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct McpEnvironment {
    /// The search directories, in order.
    pub path: Vec<String>,
    /// What the login shell contributed that the launcher did not. Empty when
    /// Taurus was started from a terminal, which is when it has nothing to add.
    pub added: Vec<String>,
    /// Why the shell was not asked, when it was not.
    #[ts(optional)]
    pub skipped: Option<String>,
}

#[tauri::command]
pub async fn mcp_environment() -> CmdResult<McpEnvironment> {
    // `adopt` ran at startup; this returns that same answer rather than probing
    // again, so the panel shows the PATH the servers were actually started with.
    let outcome = taurus_tools::login_path::adopt();
    Ok(McpEnvironment {
        path: taurus_tools::login_path::entries()
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        added: outcome.added.clone(),
        skipped: outcome.skipped.clone(),
    })
}

/// Writes one server into its layer's `mcp.json` and reconnects.
///
/// Reconnecting is part of the save rather than a second button. A panel that
/// saved and left the old connection running would be the bug this feature
/// started with — a server configured and not there — reproduced in the UI meant
/// to fix it.
///
/// `previous` is the entry being replaced, which the panel sends whenever it is
/// editing rather than adding. It is what a rename or a move between layers is
/// resolved against: the stored secrets are read from there, and it is removed
/// afterwards if the draft no longer lives at that name and scope.
#[tauri::command]
pub async fn save_mcp_server(
    state: State<'_, Arc<AppState>>,
    draft: McpServerDraft,
    previous: Option<McpServerRef>,
) -> CmdResult<Vec<McpServerView>> {
    let workspace = state.host.workspace().await;
    let source = previous.clone().unwrap_or_else(|| draft.origin());
    let server = draft.to_config(stored(&workspace, &source).as_ref());

    taurus_host::config::save_mcp_server(draft.scope, Some(&workspace), &draft.name, &server)?;

    // Only when it actually moved. Deleting unconditionally would make every
    // save a delete of the entry it had just written.
    if source.scope != draft.scope || source.name.trim() != draft.name.trim() {
        taurus_host::config::delete_mcp_server(source.scope, Some(&workspace), source.name.trim())?;
    }

    state.host.reload_mcp().await;
    // The panel is handed the listing directly; this is for the rail's badge,
    // which is showing the same servers from somewhere else on screen.
    emit_status(&state).await;
    Ok(state.host.mcp_servers().await)
}

/// One layer's stored entry for a server, for the secrets the panel was never
/// given.
///
/// Read from the layer that entry actually lives in rather than from the merged
/// view: a workspace server must not silently inherit the global server's token
/// because the two share a name.
pub(super) fn stored(
    workspace: &std::path::Path,
    entry: &McpServerRef,
) -> Option<taurus_mcp::ServerConfig> {
    taurus_host::config::scope_dir(entry.scope, Some(workspace))
        .and_then(|dir| taurus_mcp::load(&dir).ok())
        .and_then(|layer| layer.servers.get(entry.name.trim()).cloned())
}

#[tauri::command]
pub async fn delete_mcp_server(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    name: String,
) -> CmdResult<Vec<McpServerView>> {
    let workspace = state.host.workspace().await;
    taurus_host::config::delete_mcp_server(scope, Some(&workspace), &name)?;
    state.host.reload_mcp().await;
    emit_status(&state).await;
    Ok(state.host.mcp_servers().await)
}

#[tauri::command]
pub async fn set_mcp_server_disabled(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    name: String,
    disabled: bool,
) -> CmdResult<Vec<McpServerView>> {
    let workspace = state.host.workspace().await;
    taurus_host::config::set_mcp_server_disabled(scope, Some(&workspace), &name, disabled)?;
    state.host.reload_mcp().await;
    emit_status(&state).await;
    Ok(state.host.mcp_servers().await)
}

/// Connects to one entry, reports what it offers, and disconnects.
///
/// Takes the draft rather than a saved name, so an edit can be checked before it
/// is written. Nothing is registered and no live connection is touched.
#[tauri::command]
pub async fn test_mcp_server(
    state: State<'_, Arc<AppState>>,
    draft: McpServerDraft,
    previous: Option<McpServerRef>,
) -> CmdResult<Vec<String>> {
    let workspace = state.host.workspace().await;
    // A secret the form never had still has to reach the server being tested, or
    // testing an entry whose credential was not retyped would fail on the one
    // thing the panel deliberately never showed it. Resolved through `previous`
    // for the same reason a save is: a rename in the form must not lose the
    // token belonging to the entry it came from.
    let source = previous.unwrap_or_else(|| draft.origin());
    let server = draft.to_config(stored(&workspace, &source).as_ref());

    state.host.test_mcp_server(&draft.name, &server).await
}

/// Signs in to an MCP server that wants OAuth.
///
/// One command for the whole flow, and it does not return until the browser has
/// come back. The alternative — begin, hand the URL to the frontend, poll — puts
/// a half-finished authorization in the window's state where a reload would
/// strand it holding a port open.
///
/// The three steps in order: discover and register, which needs the network;
/// open a browser, which needs this layer, since `taurus-mcp` cannot reach the
/// desktop; and wait for the redirect on loopback. Nothing is written to
/// `mcp.json` — the credentials go to the keychain, keyed to the server's name.
///
/// Never called on its own behalf. A connection that opened a browser window
/// because a server answered 401 would be the application taking the screen in
/// response to something the user did not do; this runs when somebody presses
/// **Sign in**.
#[tauri::command]
pub async fn mcp_sign_in(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    name: String,
) -> CmdResult<Vec<McpServerView>> {
    use tauri_plugin_opener::OpenerExt;

    let sign_in = state.host.begin_mcp_sign_in(&name).await?;
    app.opener()
        .open_url(sign_in.authorization_url.clone(), None::<&str>)
        .map_err(|e| format!("could not open a browser to sign in: {e}"))?;
    sign_in.finish().await?;

    // Reconnected rather than left for the user to press Reconnect: the whole
    // point of signing in is the server working, and it cannot work until it is
    // reconnected with the credentials that did not exist a moment ago.
    state.host.reload_mcp().await;
    emit_status(&state).await;
    Ok(state.host.mcp_servers().await)
}

/// Forgets one server's sign-in.
///
/// Taurus's copy only. The grant still exists at the authorization server until
/// it is removed there, which the panel says rather than leaving somebody to
/// assume this revoked something.
#[tauri::command]
pub async fn mcp_sign_out(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> CmdResult<Vec<McpServerView>> {
    state.host.mcp_sign_out(&name)?;
    state.host.reload_mcp().await;
    emit_status(&state).await;
    Ok(state.host.mcp_servers().await)
}

/// Which of these programs are on the PATH this application inherited.
///
/// The catalogue warns "needs uvx" before anything is filled in, and that
/// warning has to be a fact rather than a guess: `uvx` being installed and
/// `uvx` being *reachable* are different questions, because a window opened
/// from the Dock inherits the launcher's PATH and not a shell's. Answered
/// through the same resolver that fills in `McpServerView::program`, so the
/// catalogue and the server list cannot disagree about the same binary.
///
/// Returns the subset that resolves, rather than a map. The caller asks about
/// two or three names and wants to know which are missing.
#[tauri::command]
pub async fn programs_on_path(names: Vec<String>) -> CmdResult<Vec<String>> {
    // Off the runtime: each name is a walk of every directory on the PATH.
    off_runtime(move || {
        Ok(names
            .into_iter()
            .filter(|name| taurus_tools::login_path::which(name.trim()).is_some())
            .collect())
    })
    .await
}

/// The servers the panel offers to add, and the ones it explains instead.
///
/// Shipped in the binary rather than fetched. See
/// [`taurus_mcp::catalog`] for why a list reviewed in a commit is the only
/// arrangement that answers the objection `draft_mcp_server` makes about
/// installing a package name nobody has read.
#[tauri::command]
pub async fn mcp_catalog() -> CmdResult<taurus_mcp::Catalog> {
    Ok(taurus_mcp::catalog())
}

/// Reconnects every MCP server without rescanning anything else.
///
/// Narrower than [`reload_config`] on purpose — see `Host::reload_mcp`.
#[tauri::command]
pub async fn reload_mcp(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<McpServerView>> {
    state.host.reload_mcp().await;
    emit_status(&state).await;
    Ok(state.host.mcp_servers().await)
}
