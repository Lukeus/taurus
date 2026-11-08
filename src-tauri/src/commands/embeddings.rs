//! The embedding models Taurus runs itself, for search with no model server.
//!
//! The one thing here that reaches the network is [`embedding_download`], and
//! the window only calls it when somebody presses Download. See
//! `taurus_embed` for what each piece does.

use super::*;

use taurus_embed::EmbedError;

/// Where a model download is filed in [`AppState::stoppable`].
const EMBEDDING_DOWNLOAD: &str = "embedding-download";

/// One model, as Settings › Search shows it.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct EmbeddingModel {
    pub id: String,
    pub summary: String,
    pub license: String,
    #[ts(type = "number")]
    pub bytes: u64,
    pub installed: bool,
}

/// What Settings › Search needs to offer the built-in engine.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct EmbeddingModels {
    /// What `embedding_provider` is set to for this engine.
    pub provider: String,
    /// The one to offer first.
    pub default_model: String,
    pub models: Vec<EmbeddingModel>,
    /// Where downloaded models are kept, for Settings to show.
    pub folder: String,
}

/// How far a download is.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct EmbeddingDownload {
    #[ts(type = "number")]
    pub done: u64,
    #[ts(type = "number")]
    pub total: u64,
}

fn text(e: EmbedError) -> String {
    e.to_string()
}

fn spec(model: &str) -> Result<&'static taurus_embed::Spec, String> {
    taurus_embed::spec(model).ok_or_else(|| {
        text(EmbedError::UnknownModel {
            name: model.trim().to_string(),
        })
    })
}

#[tauri::command]
pub async fn embedding_models(state: State<'_, Arc<AppState>>) -> CmdResult<EmbeddingModels> {
    let local = Arc::clone(state.host.local_embeddings());
    // Stat calls only, but on whatever disk `~/.taurus` is on.
    off_runtime(move || {
        Ok(EmbeddingModels {
            provider: taurus_embed::PROVIDER_ID.to_string(),
            default_model: taurus_embed::DEFAULT_MODEL.to_string(),
            models: taurus_embed::MODELS
                .iter()
                .map(|m| EmbeddingModel {
                    id: m.id.to_string(),
                    summary: m.summary.to_string(),
                    license: m.license.to_string(),
                    bytes: m.bytes(),
                    installed: local.files().installed(m),
                })
                .collect(),
            folder: local.files().dir().display().to_string(),
        })
    })
    .await
}

/// Downloads a model, reporting progress, and resolves once every file is
/// checked and in place.
///
/// Reloads afterwards, because `search_code` is registered only once its
/// model is ready: without this, search set to the model being downloaded
/// would stay off until the next workspace change.
#[tauri::command]
pub async fn embedding_download(
    state: State<'_, Arc<AppState>>,
    model: String,
    on_progress: Channel<EmbeddingDownload>,
) -> CmdResult<()> {
    let spec = spec(&model)?;
    // One at a time; a second press replaces the first.
    let running = state.stoppable.start(EMBEDDING_DOWNLOAD);
    info!(model = spec.id, "downloading an embedding model");
    let result = state
        .host
        .local_embeddings()
        .files()
        .download(spec, &running.cancel, |done, total| {
            // A closed channel means Settings closed. Still worth finishing.
            let _ = on_progress.send(EmbeddingDownload { done, total });
        })
        .await;
    match result {
        Ok(()) => {
            info!(model = spec.id, "embedding model downloaded and verified");
            state.host.reload_local().await;
            emit_status(&state).await;
            Ok(())
        }
        Err(e) => {
            if !matches!(e, EmbedError::Canceled) {
                error!(model = spec.id, error = %e, "embedding model download failed");
            }
            Err(text(e))
        }
    }
}

#[tauri::command]
pub fn stop_embedding_download(state: State<'_, Arc<AppState>>) {
    state.stoppable.stop(EMBEDDING_DOWNLOAD);
}

/// Deletes a model's files. Reloads, so search set to it goes off and says
/// why rather than failing at its next query.
#[tauri::command]
pub async fn delete_embedding_model(
    state: State<'_, Arc<AppState>>,
    model: String,
) -> CmdResult<()> {
    let spec = spec(&model)?;
    let local = Arc::clone(state.host.local_embeddings());
    off_runtime(move || local.files().delete(spec).map_err(text)).await?;
    state.host.reload_local().await;
    emit_status(&state).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_default_is_offered() {
        assert!(taurus_embed::spec(taurus_embed::DEFAULT_MODEL).is_some());
    }

    #[test]
    fn the_search_tab_names_the_same_engine_and_default() {
        // The tab saves these before it has asked for the catalog, so it
        // carries its own copies. A rename here has to reach them.
        let tab = include_str!("../../../src/components/settings/SearchTab.tsx");
        let builtin = format!("export const BUILTIN = \"{}\";", taurus_embed::PROVIDER_ID);
        let default = format!(
            "export const DEFAULT_LOCAL_MODEL = \"{}\";",
            taurus_embed::DEFAULT_MODEL
        );
        assert!(tab.contains(&builtin), "{builtin}");
        assert!(tab.contains(&default), "{default}");
    }
}
