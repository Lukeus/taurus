//! Speech input: the microphone, and turning what it heard into text.
//!
//! Everything runs on this machine. Audio goes from the default input device
//! into memory, through whisper.cpp, and out as a string; nothing is written to
//! disk and nothing is sent anywhere. The one network call in the crate is the
//! model download in [`models`], which a person starts.
//!
//! Three parts, used in this order:
//!
//! - [`models`] — the speech models Taurus knows, pinned by size and checksum,
//!   and fetching one the first time it is wanted. Not bundled: a 60 MB file in
//!   every installer would be paid by everybody who never presses the mic.
//! - [`capture`] — a push-to-talk recording on a thread of its own, reporting a
//!   level while it runs and handing back 16 kHz mono when it stops.
//! - [`transcribe`] — whisper.cpp over that audio, with the workspace's names
//!   offered as a hint so a project's own words come back spelled right.
//!
//! [`Voice`] holds the state the three share: at most one recording, and the
//! model that was loaded last.

pub mod capture;
pub mod models;
mod resample;
pub mod transcribe;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

pub use capture::{input_device_name, Captured, Recording};
pub use models::{spec, ModelSpec, Models, DEFAULT_MODEL, MODELS};
pub use resample::{to_whisper_rate, WHISPER_RATE};
pub use transcribe::{clean, hint_for, Engine};

/// Everything that can go wrong between pressing the mic and reading the text.
///
/// Each message says what to do next, because each one is shown as it is —
/// under the composer, in the place somebody is looking when it fails.
#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error(
        "No microphone found. Plug one in or choose an input device in your \
         system's sound settings, then try again."
    )]
    NoDevice,
    #[error("{}", permission_help())]
    PermissionDenied,
    /// The microphone delivered exact zeros for the whole recording.
    ///
    /// Not quiet, which is what a muted room sounds like, but digital silence.
    /// macOS sends that rather than an error when an app isn't allowed to
    /// record, so this is the denied-permission case there.
    #[error(
        "The microphone sent nothing but silence. {} If it's allowed, check \
         that the right input device is selected and not muted.",
        permission_help()
    )]
    NoSignal,
    #[error(
        "Taurus didn't hear any speech. Hold the mic while you talk, or press \
         it once to start and again when you're done."
    )]
    NothingHeard,
    #[error("Couldn't use the microphone ({device}): {reason}")]
    Device { device: String, reason: String },
    #[error(
        "The microphone didn't open within 10 seconds. If your system is \
         asking whether Taurus may use the microphone, answer it and press the \
         mic again. Otherwise the system's audio service may be stuck: check \
         that other apps can play and record sound."
    )]
    Stalled,
    #[error("The microphone stopped working mid-recording: {0}. Try again.")]
    Lost(String),
    #[error("Taurus is already listening.")]
    Busy,
    #[error("Taurus isn't listening.")]
    NotListening,
    #[error(
        "\"{0}\" isn't a speech model Taurus knows. Pick one of: {list}.",
        list = models::known_ids()
    )]
    UnknownModel(String),
    #[error(
        "The speech model {0} isn't downloaded yet. Press the mic to download \
         it, or get it from Settings › Voice."
    )]
    ModelMissing(String),
    #[error(
        "Downloading {file} failed: {reason}. You can also download it \
         yourself from {url} and save it as {path}."
    )]
    Download {
        file: String,
        url: String,
        path: String,
        reason: String,
    },
    #[error("Canceled.")]
    Canceled,
    #[error(
        "Couldn't load the speech model at {path}: {reason}. Delete it in \
         Settings › Voice and download it again."
    )]
    Load { path: String, reason: String },
    #[error("Transcribing failed: {0}")]
    Transcribe(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Where to go to let Taurus use the microphone, on this platform.
fn permission_help() -> &'static str {
    if cfg!(target_os = "macos") {
        "Taurus may not be allowed to use the microphone: turn it on in System \
         Settings › Privacy & Security › Microphone, then quit and reopen \
         Taurus."
    } else if cfg!(windows) {
        "Windows may be blocking the microphone: turn on Settings › Privacy & \
         security › Microphone › \"Let desktop apps access your microphone\"."
    } else {
        "Your audio system may be refusing access: check that your user can \
         record audio and that no other app holds the device."
    }
}

/// The shared state behind the window's voice commands.
///
/// One recording at a time, because there is one microphone and one composer
/// for it to type into. The engine keeps the last model loaded, which is what
/// makes the second dictation start without reading 60 MB again.
pub struct Voice {
    models: Models,
    engine: Engine,
    recording: Mutex<Option<Recording>>,
}

impl Voice {
    /// `dir` is where models are kept. It's created when the first one is
    /// downloaded, not before.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            models: Models::new(dir),
            engine: Engine::default(),
            recording: Mutex::new(None),
        }
    }

    pub fn models(&self) -> &Models {
        &self.models
    }

    /// Opens the microphone and starts keeping what it hears.
    ///
    /// `on_level` is called about twenty times a second with the loudest
    /// sample since the last call, from 0 to 1, for as long as it runs.
    pub fn start(&self, on_level: impl Fn(f32) + Send + 'static) -> Result<(), VoiceError> {
        let mut slot = self.recording.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err(VoiceError::Busy);
        }
        *slot = Some(capture::start(on_level)?);
        Ok(())
    }

    /// Whether a recording is running.
    pub fn listening(&self) -> bool {
        self.recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Closes the microphone and returns what it heard.
    ///
    /// Blocks for as long as the capture thread takes to wind down, which is a
    /// few milliseconds; call it off the async runtime all the same.
    pub fn stop(&self) -> Result<Captured, VoiceError> {
        let recording = self
            .recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or(VoiceError::NotListening)?;
        recording.finish()
    }

    /// Closes the microphone and throws away what it heard. Safe to call when
    /// nothing is recording.
    pub fn cancel(&self) {
        let recording = self
            .recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(recording) = recording {
            recording.discard();
        }
    }

    /// Turns captured audio into text with the model `id`.
    ///
    /// Blocking, and CPU-heavy on machines without a GPU backend. `abort` set
    /// from another thread stops it at the next point whisper.cpp checks.
    pub fn transcribe(
        &self,
        id: &str,
        audio: &Captured,
        hint: &str,
        abort: Arc<AtomicBool>,
    ) -> Result<String, VoiceError> {
        let spec = spec(id)?;
        let path = self.models.ready(spec)?;
        self.engine.transcribe(spec, &path, audio, hint, abort)
    }
}
