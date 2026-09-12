//! The web search backend and its keys.

use super::*;

#[tauri::command]
pub async fn get_search_settings(state: State<'_, Arc<AppState>>) -> CmdResult<SearchSettings> {
    let file = state.host.global_search();

    let backends: Vec<SearchBackend> = file
        .backends
        .iter()
        .filter_map(|(id, entry)| {
            // An entry with no kind is a workspace override of something the
            // global layer never defined; there is nothing to edit here.
            let kind = entry.kind?;
            Some(SearchBackend {
                id: id.clone(),
                kind,
                base_url: entry
                    .base_url
                    .clone()
                    .or_else(|| kind.default_base_url().map(str::to_string))
                    .unwrap_or_default(),
                api_key_env: entry.api_key_env.clone(),
                max_results: entry.max_results,
                needs_key: kind.needs_key(),
            })
        })
        .collect();

    // Off the runtime: each status is a read of the OS keychain, which with a
    // locked keychain waits on its dialog.
    let asked: Vec<(String, Option<String>)> = backends
        .iter()
        .map(|b| (b.id.clone(), b.api_key_env.clone()))
        .collect();
    let key_statuses = off_runtime(move || {
        Ok(asked
            .into_iter()
            .map(|(id, variable)| {
                let status = taurus_host::config::search_key_status(&id, variable.as_deref());
                (id, status)
            })
            .collect())
    })
    .await?;

    Ok(SearchSettings {
        selected: file.backend.clone(),
        backends,
        key_statuses,
        active: state.host.search_active().await,
        problems: state
            .host
            .problems_from(&[taurus_host::ProblemSource::Search])
            .await,
    })
}

#[tauri::command]
pub async fn save_search_settings(
    state: State<'_, Arc<AppState>>,
    selected: Option<String>,
    backends: Vec<SearchBackend>,
) -> CmdResult<()> {
    let mut file = state.host.global_search();

    // Entries are updated in place rather than rebuilt, so fields the settings
    // screen does not know about survive a save by someone who hand-edited the
    // file. Losing those silently is how a config editor earns distrust.
    for backend in &backends {
        let entry = file.backends.entry(backend.id.clone()).or_default();
        entry.kind = Some(backend.kind);
        entry.base_url = Some(backend.base_url.clone()).filter(|u| !u.trim().is_empty());
        entry.api_key_env = backend.api_key_env.clone().filter(|v| !v.trim().is_empty());
        entry.max_results = backend.max_results;
    }
    file.backend = selected.filter(|id| !id.trim().is_empty());

    state.host.set_search(file).await;
    Ok(())
}

#[tauri::command]
pub async fn set_search_key(
    state: State<'_, Arc<AppState>>,
    backend_id: String,
    key: String,
) -> CmdResult<()> {
    state.host.set_search_key(&backend_id, &key).await
}

#[tauri::command]
pub async fn clear_search_key(
    state: State<'_, Arc<AppState>>,
    backend_id: String,
) -> CmdResult<()> {
    state.host.clear_search_key(&backend_id).await
}
