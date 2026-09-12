//! Model providers and their keys.

use super::*;

/// The global provider layer, for the settings editor.
///
/// Deliberately not `AppStatus::providers`, which is the effective list with
/// this workspace's overrides folded in. An editor that saved that back would
/// write one project's settings into every project's config.
#[tauri::command]
pub async fn list_global_providers(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<ProviderConfig>> {
    Ok(state.host.global_providers().await)
}

/// Where each provider's API key comes from.
///
/// Status only — the key itself is never sent to the frontend. A secret handed
/// to the webview lives in JavaScript memory and in whatever the DOM does with
/// it, and there is nothing the settings screen needs it for: the field is a
/// place to type a new key, not to review the old one.
#[tauri::command]
pub async fn list_key_statuses(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<(String, KeyStatus)>> {
    Ok(state.host.key_statuses().await)
}

#[tauri::command]
pub async fn keychain_available() -> CmdResult<bool> {
    Ok(Host::keychain_available())
}

#[tauri::command]
pub async fn set_provider_key(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
    key: String,
) -> CmdResult<()> {
    state.host.set_provider_key(&provider_id, &key).await?;
    Ok(())
}

#[tauri::command]
pub async fn clear_provider_key(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
) -> CmdResult<()> {
    state.host.clear_provider_key(&provider_id).await?;
    Ok(())
}

/// One search backend as the settings screen edits it.
///
/// Flattened out of `SearchFile`'s map so the frontend gets a list it can
/// render in a stable order, with the id alongside the entry rather than as a
/// key it has to carry separately.
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchBackend {
    pub id: String,
    pub kind: BackendKind,
    /// Empty when the config does not say and the kind has no default — which
    /// is only SearXNG, where guessing would turn a blank into a refused
    /// connection.
    pub base_url: String,
    pub api_key_env: Option<String>,
    pub max_results: Option<u8>,
    /// False for SearXNG, so the UI can leave the key field out entirely.
    pub needs_key: bool,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct SearchSettings {
    /// The selected backend id, or null when search is off — which is the
    /// default, and not a problem.
    pub selected: Option<String>,
    pub backends: Vec<SearchBackend>,
    /// Where each backend's key comes from, by id. Status only; the key itself
    /// never crosses into the frontend.
    pub key_statuses: Vec<(String, KeyStatus)>,
    /// True when `web_search` is actually registered. A backend can be selected
    /// and still not run, and saying "on" then would be a lie.
    pub active: bool,
    pub problems: Vec<Problem>,
}

#[tauri::command]
pub async fn save_providers(
    state: State<'_, Arc<AppState>>,
    providers: Vec<ProviderConfig>,
) -> CmdResult<()> {
    if providers.is_empty() {
        return Err("at least one provider must be configured".into());
    }
    state.host.set_providers(providers).await;
    emit_status(&state).await;
    Ok(())
}
