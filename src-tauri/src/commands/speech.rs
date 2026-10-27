//! Reading replies aloud: the voice settings, their keys, and the speaker.
//!
//! The window decides what to read: the turn it sent, as it streams, when
//! read-aloud is on, or a reply whose Read aloud button was pressed. This
//! side decides how. See `taurus_speech` for the filter and the voices.

use super::*;

use taurus_host::config::{SpeechEngine, SpeechSettings};
use taurus_speech::{ElevenLabs, Engine, OpenAi, VoiceInfo, ELEVENLABS_URL};

/// Whether the speaker is talking, and why it stopped when it stopped by
/// failing.
pub const EVENT_SPEECH: &str = "taurus://speech";

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct SpeechState {
    pub speaking: bool,
    #[ts(optional)]
    pub failed: Option<String>,
}

/// What Settings › Voice needs to draw the speech section.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct SpeechStatus {
    pub settings: SpeechSettings,
    /// Where each service's key is coming from.
    pub key_statuses: Vec<(SpeechEngine, KeyStatus)>,
    pub speaking: bool,
}

/// The speaker's events as the window's.
pub fn speech_event(app: &tauri::AppHandle, event: taurus_speech::SpeechEvent) {
    let state = match event {
        taurus_speech::SpeechEvent::Speaking(speaking) => SpeechState {
            speaking,
            failed: None,
        },
        taurus_speech::SpeechEvent::Failed(why) => SpeechState {
            speaking: false,
            failed: Some(why),
        },
    };
    if let Err(e) = app.emit(EVENT_SPEECH, &state) {
        tracing::warn!(error = %e, "could not tell the window about speech");
    }
}

#[tauri::command]
pub async fn speech_status(state: State<'_, Arc<AppState>>) -> CmdResult<SpeechStatus> {
    Ok(SpeechStatus {
        settings: state.host.settings().await.speech,
        key_statuses: state.host.speech_key_statuses().await,
        speaking: state.speaker.speaking(),
    })
}

#[tauri::command]
pub async fn save_speech_settings(
    state: State<'_, Arc<AppState>>,
    settings: SpeechSettings,
) -> CmdResult<()> {
    let settings = tidy(settings)?;
    state.host.set_speech(settings).await?;
    forget_engine(&state);
    emit_status(&state).await;
    Ok(())
}

/// Trims what was typed, puts back the defaults for what was emptied, and
/// refuses an address that isn't one, naming the field.
fn tidy(mut settings: SpeechSettings) -> CmdResult<SpeechSettings> {
    let defaults = SpeechSettings::default();
    let trimmed = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());

    settings.system.voice = settings.system.voice.as_deref().and_then(trimmed);

    let eleven = &mut settings.elevenlabs;
    eleven.voice_id = eleven.voice_id.as_deref().and_then(trimmed);
    eleven.model = trimmed(&eleven.model).unwrap_or(defaults.elevenlabs.model);
    eleven.api_key_env = eleven.api_key_env.as_deref().and_then(trimmed);

    let openai = &mut settings.openai;
    openai.base_url = trimmed(&openai.base_url).unwrap_or(defaults.openai.base_url);
    openai.model = trimmed(&openai.model).unwrap_or(defaults.openai.model);
    openai.voice = trimmed(&openai.voice).unwrap_or(defaults.openai.voice);
    openai.api_key_env = openai.api_key_env.as_deref().and_then(trimmed);
    match taurus_speech::remote::Url::parse(&openai.base_url) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => {}
        _ => {
            return Err(format!(
                "\"{}\" isn't an address Taurus can send speech to. It should look like \
                 https://api.openai.com/v1 or http://localhost:8880/v1.",
                openai.base_url
            ))
        }
    }
    Ok(settings)
}

#[tauri::command]
pub async fn set_speech_key(
    state: State<'_, Arc<AppState>>,
    engine: SpeechEngine,
    key: String,
) -> CmdResult<()> {
    off_runtime(move || Host::set_speech_key(engine, key.trim())).await?;
    forget_engine(&state);
    Ok(())
}

#[tauri::command]
pub async fn clear_speech_key(
    state: State<'_, Arc<AppState>>,
    engine: SpeechEngine,
) -> CmdResult<()> {
    off_runtime(move || Host::clear_speech_key(engine)).await?;
    forget_engine(&state);
    Ok(())
}

/// The voices `engine` offers. For ElevenLabs that's a request to the
/// account, so it needs the key.
#[tauri::command]
pub async fn speech_voices(
    state: State<'_, Arc<AppState>>,
    engine: SpeechEngine,
) -> CmdResult<Vec<VoiceInfo>> {
    let speech = state.host.settings().await.speech;
    let engine = match engine {
        SpeechEngine::System => Engine::System { voice: None },
        SpeechEngine::ElevenLabs => {
            let variable = speech.elevenlabs.api_key_env.clone();
            let key = off_runtime(move || elevenlabs_key(variable.as_deref())).await?;
            Engine::ElevenLabs(ElevenLabs {
                base_url: ELEVENLABS_URL.into(),
                voice_id: String::new(),
                model: speech.elevenlabs.model,
                key,
            })
        }
        SpeechEngine::OpenAi => return Ok(taurus_speech::remote::openai_voices()),
    };
    engine
        .voices(state.speaker.http())
        .await
        .map_err(|e| e.to_string())
}

/// Stops whatever is being said and reads `text` from the start.
#[tauri::command]
pub async fn speech_say(state: State<'_, Arc<AppState>>, text: String) -> CmdResult<()> {
    let engine = engine(&state).await?;
    state.speaker.say(engine, &text);
    Ok(())
}

/// The next piece of a reply as it streams. The window sends these one at a
/// time, in order: two in flight at once could arrive swapped.
#[tauri::command]
pub async fn speech_feed(state: State<'_, Arc<AppState>>, text: String) -> CmdResult<()> {
    let engine = engine(&state).await?;
    state.speaker.feed(engine, &text);
    Ok(())
}

/// The reply has finished, or stopped for a tool call: say what's held back.
#[tauri::command]
pub async fn speech_flush(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    let engine = engine(&state).await?;
    state.speaker.flush(engine);
    Ok(())
}

#[tauri::command]
pub fn speech_stop(state: State<'_, Arc<AppState>>) {
    state.speaker.stop();
}

/// The voice to speak with, resolved once and kept until the settings or a
/// key change. Resolving reads the credential store, and a reply's every
/// sentence going back to the keychain would be a dialog per sentence on a
/// build macOS hasn't been told to trust.
async fn engine(state: &AppState) -> CmdResult<Arc<Engine>> {
    if let Some(engine) = state
        .speech_engine
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return Ok(engine);
    }
    let speech = state.host.settings().await.speech;
    let engine = Arc::new(off_runtime(move || resolve(&speech)).await?);
    *state
        .speech_engine
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&engine));
    Ok(engine)
}

fn forget_engine(state: &AppState) {
    state
        .speech_engine
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
}

/// The settings as a voice that can speak, or what's missing.
fn resolve(speech: &SpeechSettings) -> CmdResult<Engine> {
    Ok(match speech.engine {
        SpeechEngine::System => Engine::System {
            voice: speech.system.voice.clone(),
        },
        SpeechEngine::ElevenLabs => {
            let eleven = &speech.elevenlabs;
            let voice_id = eleven
                .voice_id
                .clone()
                .ok_or("Pick an ElevenLabs voice in Settings › Voice before reading aloud.")?;
            Engine::ElevenLabs(ElevenLabs {
                base_url: ELEVENLABS_URL.into(),
                voice_id,
                model: eleven.model.clone(),
                key: elevenlabs_key(eleven.api_key_env.as_deref())?,
            })
        }
        SpeechEngine::OpenAi => {
            let openai = &speech.openai;
            Engine::OpenAi(OpenAi {
                base_url: openai.base_url.clone(),
                model: openai.model.clone(),
                voice: openai.voice.clone(),
                key: Host::speech_key(SpeechEngine::OpenAi, openai.api_key_env.as_deref()),
            })
        }
    })
}

fn elevenlabs_key(variable: Option<&str>) -> CmdResult<String> {
    Host::speech_key(SpeechEngine::ElevenLabs, variable).ok_or_else(|| match variable {
        Some(variable) => {
            format!("ElevenLabs needs an API key. Add one in Settings › Voice, or set ${variable}.")
        }
        None => "ElevenLabs needs an API key. Add one in Settings › Voice.".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emptied_fields_go_back_to_their_defaults() {
        let mut settings = SpeechSettings::default();
        settings.openai.base_url = "  ".into();
        settings.openai.voice = String::new();
        settings.elevenlabs.voice_id = Some(" ".into());
        settings.system.voice = Some("  Samantha ".into());
        let tidied = tidy(settings).unwrap();
        assert_eq!(tidied.openai.base_url, "https://api.openai.com/v1");
        assert_eq!(tidied.openai.voice, "alloy");
        assert_eq!(tidied.elevenlabs.voice_id, None);
        assert_eq!(tidied.system.voice.as_deref(), Some("Samantha"));
    }

    #[test]
    fn an_address_that_is_not_one_is_refused_by_name() {
        let mut settings = SpeechSettings::default();
        settings.openai.base_url = "localhost:8880".into();
        let err = tidy(settings).unwrap_err();
        assert!(err.contains("\"localhost:8880\""), "{err}");
        assert!(err.contains("http://localhost:8880/v1"), "{err}");
    }

    #[test]
    fn elevenlabs_without_a_voice_says_where_to_pick_one() {
        let settings = SpeechSettings {
            engine: SpeechEngine::ElevenLabs,
            ..SpeechSettings::default()
        };
        let err = resolve(&settings).unwrap_err();
        assert!(err.contains("Pick an ElevenLabs voice"), "{err}");
    }

    #[test]
    fn the_system_voice_needs_nothing() {
        let mut settings = SpeechSettings::default();
        settings.system.voice = Some("Samantha".into());
        assert_eq!(
            resolve(&settings).unwrap(),
            Engine::System {
                voice: Some("Samantha".into())
            }
        );
    }
}
