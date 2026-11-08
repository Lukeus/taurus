//! [`Local`]: the in-process models as a [`Provider`], so the index and the
//! search tool use them exactly as they use Ollama.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use taurus_provider::{
    Capabilities, ChatRequest, ModelInfo, Provider, ProviderError, StopReason, StreamEvent,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{catalog, EmbedError, Embedder, Models};

/// What settings call this engine, in `embedding_provider`. Not `local`:
/// a provider somebody configured for LM Studio may well be called that.
pub const PROVIDER_ID: &str = "builtin";

/// How long a loaded model is kept with nothing asking for it. Loading is
/// about half a second; holding it is about 550 MB.
const IDLE: Duration = Duration::from_secs(5 * 60);

struct Loaded {
    embedder: Arc<Embedder>,
    used: Instant,
}

/// The downloaded models, run here.
///
/// One model is loaded at a time, on first use, and dropped after [`IDLE`].
/// Embedding runs on a blocking thread, one request at a time: the network
/// already uses every core, so a second request beside it would only make
/// both slower and double the memory for activations.
pub struct Local {
    models: Models,
    loaded: Arc<Mutex<Option<Loaded>>>,
    running: tokio::sync::Semaphore,
}

impl Local {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            models: Models::new(dir),
            loaded: Arc::default(),
            running: tokio::sync::Semaphore::new(1),
        }
    }

    /// The downloaded files: what to download, delete, or check for.
    pub fn files(&self) -> &Models {
        &self.models
    }

    /// The model, loading it if it isn't already. Blocking.
    fn embedder(
        models_dir: &std::path::Path,
        loaded: &Mutex<Option<Loaded>>,
        spec: &'static catalog::Spec,
    ) -> Result<Arc<Embedder>, EmbedError> {
        let mut slot = loaded.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(held) = slot.as_mut().filter(|h| h.embedder.spec().id == spec.id) {
            held.used = Instant::now();
            return Ok(Arc::clone(&held.embedder));
        }
        // The other model, if one is held, goes first: two at once is a
        // gigabyte for a moment nobody needs.
        *slot = None;
        let started = Instant::now();
        let embedder = Arc::new(Embedder::load(&models_dir.join(spec.id), spec)?);
        tracing::info!(
            model = spec.id,
            took_ms = started.elapsed().as_millis() as u64,
            "loaded an embedding model"
        );
        *slot = Some(Loaded {
            embedder: Arc::clone(&embedder),
            used: Instant::now(),
        });
        Ok(embedder)
    }

    /// Drops the model once it has gone [`IDLE`] unused. Started after each
    /// request; the one that finds it idle drops it, the rest find it used.
    fn release_when_idle(&self) {
        let loaded = Arc::downgrade(&self.loaded);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            tokio::time::sleep(IDLE).await;
            let Some(loaded) = loaded.upgrade() else {
                return;
            };
            let mut slot = loaded.lock().unwrap_or_else(|e| e.into_inner());
            if slot.as_ref().is_some_and(|h| h.used.elapsed() >= IDLE) {
                *slot = None;
                tracing::info!("let an idle embedding model go");
            }
        });
    }
}

fn refuse(e: EmbedError) -> ProviderError {
    ProviderError::Protocol(e.to_string())
}

#[async_trait]
impl Provider for Local {
    fn id(&self) -> &str {
        PROVIDER_ID
    }

    /// The downloaded models. One that isn't downloaded can't be run, so it
    /// isn't offered as though it could.
    async fn models(&self) -> taurus_provider::Result<Vec<ModelInfo>> {
        Ok(catalog::MODELS
            .iter()
            .filter(|spec| self.models.installed(spec))
            .map(|spec| ModelInfo {
                id: spec.id.to_string(),
                display_name: spec.id.to_string(),
                context_length: None,
            })
            .collect())
    }

    async fn capabilities(&self, _: &str) -> taurus_provider::Result<Capabilities> {
        Ok(Capabilities::default())
    }

    async fn stream(
        &self,
        _: ChatRequest,
        _: mpsc::Sender<StreamEvent>,
        _: CancellationToken,
    ) -> taurus_provider::Result<StopReason> {
        Err(ProviderError::Protocol(
            "the models on this computer only make embeddings for search; they can't hold a \
             conversation"
                .into(),
        ))
    }

    async fn embed(
        &self,
        model: &str,
        inputs: &[String],
    ) -> taurus_provider::Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let spec = catalog::spec(model).ok_or_else(|| {
            refuse(EmbedError::UnknownModel {
                name: model.trim().to_string(),
            })
        })?;
        self.models.ready(spec).map_err(refuse)?;

        let _turn = self
            .running
            .acquire()
            .await
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        let dir = self.models.dir().to_path_buf();
        let loaded = Arc::clone(&self.loaded);
        let inputs = inputs.to_vec();
        let vectors = tokio::task::spawn_blocking(move || {
            let embedder = Local::embedder(&dir, &loaded, spec)?;
            embedder.embed(&inputs)
        })
        .await
        .map_err(|e| ProviderError::Protocol(format!("embedding failed to run: {e}")))?
        .map_err(refuse)?;
        self.release_when_idle();
        Ok(vectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> (Local, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Local::new(dir.path().to_path_buf()), dir)
    }

    #[tokio::test]
    async fn an_unknown_model_names_the_ones_there_are() {
        let (local, _dir) = local();
        let err = local.embed("bge-m3", &["x".into()]).await.unwrap_err();
        let text = err.to_string();
        assert!(text.contains("\"bge-m3\""), "{text}");
        assert!(text.contains("CodeRankEmbed"), "{text}");
    }

    #[tokio::test]
    async fn a_model_not_downloaded_says_where_to_get_it() {
        let (local, _dir) = local();
        let err = local
            .embed(catalog::DEFAULT_MODEL, &["x".into()])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Settings → Search"), "{err}");
    }

    #[tokio::test]
    async fn nothing_to_embed_needs_no_model() {
        let (local, _dir) = local();
        assert!(local.embed("anything", &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn only_downloaded_models_are_listed() {
        let (local, _dir) = local();
        assert!(local.models().await.unwrap().is_empty());
    }

    /// Runs the real model, when `TAURUS_EMBED_MODELS` names a folder it was
    /// downloaded to. Checks the property search depends on: a question lands
    /// nearer the passage that answers it than one that doesn't.
    #[tokio::test]
    #[ignore = "needs a downloaded model; set TAURUS_EMBED_MODELS"]
    async fn a_question_lands_nearest_its_answer() {
        let dir = std::env::var("TAURUS_EMBED_MODELS").expect("TAURUS_EMBED_MODELS");
        let local = Local::new(dir.into());
        let vectors = local
            .embed(
                catalog::DEFAULT_MODEL,
                &[
                    "Represent this query for searching relevant code: read a file into a string"
                        .into(),
                    "fn read(path: &Path) -> io::Result<String> { std::fs::read_to_string(path) }"
                        .into(),
                    "fn add(a: i32, b: i32) -> i32 { a + b }".into(),
                ],
            )
            .await
            .unwrap();
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        assert_eq!(vectors[0].len(), 768);
        assert!(dot(&vectors[0], &vectors[1]) > dot(&vectors[0], &vectors[2]));
    }
}
