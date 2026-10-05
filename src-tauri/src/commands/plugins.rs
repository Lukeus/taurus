//! Plugins: listing them, installing them, and switching them on and off.
//! See `taurus_host::plugins`.

use super::*;

use taurus_host::{plugins, PluginSummary};

/// Every installed plugin, yours and this project's, whether or not it loads.
#[tauri::command]
pub async fn list_plugins(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<PluginSummary>> {
    let workspace = state.host.workspace().await;
    off_runtime(move || Ok(listed(&workspace))).await
}

/// Installs a plugin from a folder or a git URL. Off the runtime, because a
/// clone is network time.
#[tauri::command]
pub async fn add_plugin(
    state: State<'_, Arc<AppState>>,
    from: String,
    git_ref: Option<String>,
    project: bool,
) -> CmdResult<Vec<PluginSummary>> {
    let workspace = state.host.workspace().await;
    let ws = workspace.clone();
    off_runtime(move || {
        plugins::add(from.trim(), git_ref.as_deref(), scope(project), Some(&ws)).map(|_| ())
    })
    .await?;
    changed(&state, &workspace).await
}

/// Fetches a plugin again from where it was added from.
#[tauri::command]
pub async fn update_plugin(
    state: State<'_, Arc<AppState>>,
    name: String,
    project: bool,
) -> CmdResult<Vec<PluginSummary>> {
    let workspace = state.host.workspace().await;
    let ws = workspace.clone();
    off_runtime(move || plugins::update(&name, scope(project), Some(&ws)).map(|_| ())).await?;
    changed(&state, &workspace).await
}

/// Deletes an installed plugin.
#[tauri::command]
pub async fn remove_plugin(
    state: State<'_, Arc<AppState>>,
    name: String,
    project: bool,
) -> CmdResult<Vec<PluginSummary>> {
    let workspace = state.host.workspace().await;
    let ws = workspace.clone();
    off_runtime(move || plugins::remove(&name, scope(project), Some(&ws))).await?;
    changed(&state, &workspace).await
}

/// Switches a plugin on or off in your settings or the project's.
#[tauri::command]
pub async fn set_plugin_enabled(
    state: State<'_, Arc<AppState>>,
    name: String,
    project: bool,
    enabled: bool,
) -> CmdResult<Vec<PluginSummary>> {
    let workspace = state.host.workspace().await;
    let ws = workspace.clone();
    off_runtime(move || plugins::set_enabled(&name, scope(project), Some(&ws), enabled)).await?;
    changed(&state, &workspace).await
}

fn scope(project: bool) -> Scope {
    if project {
        Scope::Workspace
    } else {
        Scope::Global
    }
}

fn listed(workspace: &std::path::Path) -> Vec<PluginSummary> {
    plugins::installed(Some(workspace))
        .into_iter()
        .map(|plugin| plugin.summary)
        .collect()
}

/// After a change: everything a plugin can bring is read again, its MCP
/// servers started or stopped, the rail told, and the panel handed its list.
async fn changed(
    state: &State<'_, Arc<AppState>>,
    workspace: &std::path::Path,
) -> CmdResult<Vec<PluginSummary>> {
    state.host.reload().await;
    emit_status(state).await;
    let workspace = workspace.to_path_buf();
    off_runtime(move || Ok(listed(&workspace))).await
}
