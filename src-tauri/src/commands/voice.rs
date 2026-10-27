//! The composer's mic: speech models, recording, and transcription.
//!
//! All of it is on this machine. The one thing here that reaches the network
//! is [`voice_download`], and the window only calls that after asking. See
//! `taurus_voice` for what each piece does.

use super::*;

use taurus_voice::VoiceError;

/// Where a model download is filed in [`AppState::stoppable`].
const VOICE_DOWNLOAD: &str = "voice-download";

/// One speech model, as Settings and the mic's first-use prompt show it.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct VoiceModel {
    pub id: String,
    pub summary: String,
    #[ts(type = "number")]
    pub bytes: u64,
    pub english_only: bool,
    pub installed: bool,
}

/// What the mic needs to know before it's pressed.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct VoiceStatus {
    /// The model settings name. Always one of `models`: a name the app
    /// doesn't know is refused when it's saved, and a hand-edited one falls
    /// back to the default here so the mic still has something to offer.
    pub model: String,
    pub models: Vec<VoiceModel>,
    /// The default input device, or `None` when there isn't one or the
    /// system's audio service didn't answer in time.
    #[ts(optional)]
    pub microphone: Option<String>,
    /// Where downloaded models are kept, for Settings to show.
    pub folder: String,
}

/// How far a download is.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct VoiceDownload {
    #[ts(type = "number")]
    pub done: u64,
    #[ts(type = "number")]
    pub total: u64,
}

fn text(e: VoiceError) -> String {
    e.to_string()
}

/// The model settings name, or the default when that name isn't one the app
/// knows. See [`VoiceStatus::model`].
async fn chosen_model(state: &AppState) -> &'static taurus_voice::ModelSpec {
    let named = state.host.settings().await.voice_model;
    taurus_voice::spec(&named).unwrap_or_else(|_| {
        tracing::warn!(model = %named, "settings name an unknown voice model; using the default");
        taurus_voice::spec(taurus_voice::DEFAULT_MODEL).expect("the default is in the catalog")
    })
}

#[tauri::command]
pub async fn voice_status(state: State<'_, Arc<AppState>>) -> CmdResult<VoiceStatus> {
    let chosen = chosen_model(&state).await;
    let voice = state.voice.clone();
    let models = taurus_voice::MODELS
        .iter()
        .map(|m| VoiceModel {
            id: m.id.to_string(),
            summary: m.summary.to_string(),
            bytes: m.bytes,
            english_only: m.english_only,
            installed: voice.models().installed(m),
        })
        .collect();
    // Asking for the device name goes through the platform's audio service,
    // which can hang (see `taurus_voice::capture`'s open timeout). A status
    // that never returns would take Settings down with it, so this one gives
    // up and reports no microphone.
    let microphone = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::task::spawn_blocking(taurus_voice::input_device_name),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .flatten();
    Ok(VoiceStatus {
        model: chosen.id.to_string(),
        models,
        microphone,
        folder: voice.models().dir().display().to_string(),
    })
}

#[tauri::command]
pub async fn set_voice_model(state: State<'_, Arc<AppState>>, model: String) -> CmdResult<()> {
    let spec = taurus_voice::spec(&model).map_err(text)?;
    state.host.set_voice_model(spec.id).await?;
    emit_status(&state).await;
    Ok(())
}

/// Downloads a speech model, reporting progress, and resolves once it's
/// checked and in place.
#[tauri::command]
pub async fn voice_download(
    state: State<'_, Arc<AppState>>,
    model: String,
    on_progress: Channel<VoiceDownload>,
) -> CmdResult<()> {
    let spec = taurus_voice::spec(&model).map_err(text)?;
    // One at a time. A second press replaces the first, which is what the
    // picker wants if somebody changes their mind mid-download.
    let running = state.stoppable.start(VOICE_DOWNLOAD);
    info!(model = spec.id, "downloading a speech model");
    let result = state
        .voice
        .models()
        .download(spec, &running.cancel, |done, total| {
            // A closed channel means the prompt went away. The download is
            // still worth finishing; it's what the next press needs.
            let _ = on_progress.send(VoiceDownload { done, total });
        })
        .await;
    match result {
        Ok(()) => {
            info!(model = spec.id, "speech model downloaded and verified");
            Ok(())
        }
        Err(e) => {
            if !matches!(e, VoiceError::Canceled) {
                error!(model = spec.id, error = %e, "speech model download failed");
            }
            Err(text(e))
        }
    }
}

#[tauri::command]
pub fn stop_voice_download(state: State<'_, Arc<AppState>>) {
    state.stoppable.stop(VOICE_DOWNLOAD);
}

#[tauri::command]
pub async fn delete_voice_model(state: State<'_, Arc<AppState>>, model: String) -> CmdResult<()> {
    let spec = taurus_voice::spec(&model).map_err(text)?;
    let voice = state.voice.clone();
    off_runtime(move || voice.models().delete(spec).map_err(text)).await
}

/// Opens the microphone. `on_level` gets the input level, 0 to 1, about
/// twenty times a second until [`voice_stop`] or [`voice_cancel`].
///
/// Refuses before opening anything when the model isn't downloaded, so a
/// press never records something there's no way to transcribe.
#[tauri::command]
pub async fn voice_start(state: State<'_, Arc<AppState>>, on_level: Channel<f32>) -> CmdResult<()> {
    let spec = chosen_model(&state).await;
    state.voice.models().ready(spec).map_err(text)?;
    let voice = state.voice.clone();
    // Off the runtime: opening a device is normally instant but can block for
    // as long as the platform's audio service does, up to the capture's own
    // timeout.
    off_runtime(move || {
        voice
            .start(move |level| {
                let _ = on_level.send(level);
            })
            .map_err(text)
    })
    .await?;
    info!("listening");
    Ok(())
}

/// Closes the microphone and returns what was said.
#[tauri::command]
pub async fn voice_stop(state: State<'_, Arc<AppState>>) -> CmdResult<String> {
    let spec = chosen_model(&state).await;
    let workspace = state.host.workspace().await;
    let abort = Arc::new(AtomicBool::new(false));
    *state.voice_abort.lock().unwrap_or_else(|e| e.into_inner()) = Arc::clone(&abort);
    let voice = state.voice.clone();
    off_runtime(move || {
        let audio = voice.stop().map_err(text)?;
        if audio.truncated {
            tracing::warn!("dictation ran past the recording limit; its end was dropped");
        }
        let hint = hint(&workspace);
        let started = std::time::Instant::now();
        let heard = voice.transcribe(spec.id, &audio, &hint, abort);
        info!(
            seconds = audio.seconds(),
            took_ms = started.elapsed().as_millis() as u64,
            ok = heard.is_ok(),
            "transcribed a dictation"
        );
        heard.map_err(text)
    })
    .await
}

/// Closes the microphone and drops what it heard, or stops a transcription
/// that's under way. Safe to call when neither is happening.
#[tauri::command]
pub fn voice_cancel(state: State<'_, Arc<AppState>>) {
    state.voice.cancel();
    state
        .voice_abort
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .store(true, Ordering::Relaxed);
}

/// The words to prime the model with for this workspace: its name and what's
/// at its top level. See `taurus_voice::hint_for`.
fn hint(workspace: &std::path::Path) -> String {
    let name = workspace
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut names: Vec<String> = std::fs::read_dir(workspace)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.len() <= 40)
                .collect()
        })
        .unwrap_or_default();
    // Sorted so the same folder always primes the same way.
    names.sort();
    taurus_voice::hint_for(&name, names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_default_is_the_catalog_default() {
        // Two crates name the default, and neither depends on the other.
        assert_eq!(
            taurus_host::config::DEFAULT_VOICE_MODEL,
            taurus_voice::DEFAULT_MODEL
        );
    }

    #[test]
    fn the_hint_names_the_workspace_and_its_top_level() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("taurus-ai-shell");
        std::fs::create_dir_all(ws.join("crates")).unwrap();
        std::fs::write(ws.join("README.md"), "").unwrap();
        std::fs::write(ws.join(".gitignore"), "").unwrap();
        assert_eq!(hint(&ws), "Taurus, taurus-ai-shell, README.md, crates.");
    }
}
