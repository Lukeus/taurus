//! Speech output: reading replies aloud.
//!
//! Four parts, in the order a reply goes through them:
//!
//! - [`speakable`] — what of a reply to say. Prose, a sentence at a time, as
//!   it streams; never code, tables or markup.
//! - [`system`] and [`remote`] — the voice that says it. The system's own by
//!   default, or ElevenLabs, or anything shaped like OpenAI's
//!   `/v1/audio/speech`.
//! - [`player`] — the speakers. Every voice's audio ends up in one queue, so
//!   stopping is one thing to do and the next sentence is synthesized while
//!   this one plays.
//!
//! [`Speaker`] holds them together. It runs on a thread of its own with its
//! own small runtime, so it works the same under the app, the CLI, or a test.

pub mod player;
pub mod remote;
pub mod speakable;
pub mod system;
pub mod wav;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use player::{Player, Resampler};
pub use remote::{ElevenLabs, OpenAi, ELEVENLABS_URL, OPENAI_VOICES};
pub use speakable::{sentences, Speakable};
pub use system::VoiceInfo;

/// Everything that can stop a reply being heard. Each says what to do about
/// it, because each is shown as it is.
#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    #[error(
        "No speakers or headphones found. Connect some, or choose an output device in \
         your system's sound settings."
    )]
    NoDevice,
    #[error(
        "The speakers didn't open within 10 seconds. The system's audio service may be \
         stuck: check that other apps can play sound."
    )]
    Stalled,
    #[error("Couldn't play through the default output device: {0}")]
    Device(String),
    #[error("The speakers stopped working: {0}. Try again.")]
    Lost(String),
    #[error("{0}")]
    System(String),
    #[error("{0}")]
    Service(String),
    #[error("{0}")]
    Audio(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// A voice to speak with, resolved: everything it needs, the key included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Engine {
    /// The system's voice, by the id [`system::voices`] gives, or its default.
    System {
        voice: Option<String>,
    },
    ElevenLabs(ElevenLabs),
    OpenAi(OpenAi),
}

impl Engine {
    /// The voices this engine offers, for a picker.
    pub async fn voices(&self, http: &reqwest::Client) -> Result<Vec<VoiceInfo>, SpeechError> {
        match self {
            Self::System { .. } => system::voices().await,
            Self::ElevenLabs(v) => remote::elevenlabs_voices(http, &v.base_url, &v.key).await,
            Self::OpenAi(_) => Ok(remote::openai_voices()),
        }
    }

    /// `text` as audio, handed to `play` as it's ready along with its rate.
    async fn synthesize(
        &self,
        http: &reqwest::Client,
        text: &str,
        play: &mut Play<'_>,
    ) -> Result<(), SpeechError> {
        match self {
            Self::System { voice } => {
                let pcm = system::synthesize(text, voice.as_deref()).await?;
                play(&pcm.samples, pcm.rate)
            }
            Self::ElevenLabs(v) => {
                remote::elevenlabs(http, v, text, &mut |s| play(s, remote::RATE)).await
            }
            Self::OpenAi(v) => remote::openai(http, v, text, &mut |s| play(s, remote::RATE)).await,
        }
    }
}

/// Where synthesized audio goes: samples, and the rate they're at.
type Play<'a> = dyn FnMut(&[f32], u32) -> Result<(), SpeechError> + Send + 'a;

/// What the speaker reports, from its own thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpeechEvent {
    /// Started or stopped making sound, counting what's queued to be said.
    Speaking(bool),
    /// Gave up on what it was saying. Everything queued was dropped.
    Failed(String),
}

/// How far synthesis runs ahead of playback. Enough that a sentence is ready
/// when the last one ends, and no more: a service charges per character, and
/// a stop shouldn't have paid for the rest of the reply.
const LOOKAHEAD: f32 = 6.0;

struct Job {
    epoch: u64,
    engine: Arc<Engine>,
    text: String,
}

struct Inner {
    player: Player,
    http: reqwest::Client,
    filter: Mutex<Speakable>,
    /// Bumped by every stop. A job from an older epoch is dropped unsaid.
    epoch: AtomicU64,
    cancel: Mutex<CancellationToken>,
    /// Jobs sent and not yet finished.
    queued: AtomicUsize,
    speaking: AtomicBool,
    on_event: Box<dyn Fn(SpeechEvent) + Send + Sync>,
}

impl Inner {
    fn current(&self, epoch: u64) -> bool {
        self.epoch.load(Ordering::SeqCst) == epoch
    }

    /// Reports a change in whether there's anything to hear.
    fn update(&self) {
        let now = self.queued.load(Ordering::SeqCst) > 0 || self.player.buffered() > 0.0;
        if self.speaking.swap(now, Ordering::SeqCst) != now {
            (self.on_event)(SpeechEvent::Speaking(now));
        }
    }

    fn stop(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let old = std::mem::replace(
            &mut *self.cancel.lock().unwrap_or_else(|e| e.into_inner()),
            CancellationToken::new(),
        );
        old.cancel();
        *self.filter.lock().unwrap_or_else(|e| e.into_inner()) = Speakable::new();
        self.player.clear();
    }
}

/// Reads text aloud, one sentence after another, until told to stop.
#[derive(Clone)]
pub struct Speaker {
    inner: Arc<Inner>,
    jobs: mpsc::UnboundedSender<Job>,
}

impl Speaker {
    /// Starts the speaker's thread. `on_event` is called from it.
    pub fn new(on_event: impl Fn(SpeechEvent) + Send + Sync + 'static) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        let inner = Arc::new(Inner {
            player: Player::new(),
            http,
            filter: Mutex::new(Speakable::new()),
            epoch: AtomicU64::new(0),
            cancel: Mutex::new(CancellationToken::new()),
            queued: AtomicUsize::new(0),
            speaking: AtomicBool::new(false),
            on_event: Box::new(on_event),
        });
        let (jobs, rx) = mpsc::unbounded_channel();
        let worker = Arc::clone(&inner);
        let spawned = std::thread::Builder::new()
            .name("taurus-speech".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(work(worker, rx)),
                    Err(e) => tracing::error!(error = %e, "speech runtime didn't start"),
                }
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "speech thread didn't start");
        }
        Self { inner, jobs }
    }

    /// The HTTP client the speaker uses, for listing a service's voices.
    pub fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }

    /// Stops whatever is being said and reads `text` from the start.
    pub fn say(&self, engine: Arc<Engine>, text: &str) {
        self.stop();
        for sentence in sentences(text) {
            self.send(&engine, sentence);
        }
    }

    /// The next piece of a reply that's still arriving.
    pub fn feed(&self, engine: Arc<Engine>, text: &str) {
        let ready = self
            .inner
            .filter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(text);
        for sentence in ready {
            self.send(&engine, sentence);
        }
    }

    /// The reply is over, or has paused for a tool call: say what's held back.
    pub fn flush(&self, engine: Arc<Engine>) {
        let ready = self
            .inner
            .filter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finish();
        for sentence in ready {
            self.send(&engine, sentence);
        }
    }

    /// Stops talking and forgets everything queued. Safe when silent.
    pub fn stop(&self) {
        self.inner.stop();
        self.inner.update();
    }

    /// How much audio is waiting to be played, in seconds.
    pub fn buffered(&self) -> f32 {
        self.inner.player.buffered()
    }

    /// Whether anything is being said or is queued to be.
    pub fn speaking(&self) -> bool {
        self.inner.speaking.load(Ordering::SeqCst)
    }

    fn send(&self, engine: &Arc<Engine>, text: String) {
        let job = Job {
            epoch: self.inner.epoch.load(Ordering::SeqCst),
            engine: Arc::clone(engine),
            text,
        };
        self.inner.queued.fetch_add(1, Ordering::SeqCst);
        if self.jobs.send(job).is_err() {
            self.inner.queued.fetch_sub(1, Ordering::SeqCst);
        }
        self.inner.update();
    }
}

async fn work(inner: Arc<Inner>, mut jobs: mpsc::UnboundedReceiver<Job>) {
    loop {
        // Woken by a job, or every so often to notice playback ending.
        let job = match tokio::time::timeout(Duration::from_millis(100), jobs.recv()).await {
            Ok(Some(job)) => job,
            Ok(None) => return,
            Err(_) => {
                inner.update();
                continue;
            }
        };
        run(&inner, job).await;
        inner.queued.fetch_sub(1, Ordering::SeqCst);
        inner.update();
    }
}

async fn run(inner: &Inner, job: Job) {
    while inner.current(job.epoch) && inner.player.buffered() > LOOKAHEAD {
        tokio::time::sleep(Duration::from_millis(100)).await;
        inner.update();
    }
    if !inner.current(job.epoch) {
        return;
    }
    let cancel = inner
        .cancel
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let player = inner.player.clone();
    let mut resampler: Option<(u32, Resampler, u32)> = None;
    let epoch = job.epoch;
    let mut play = |samples: &[f32], rate: u32| -> Result<(), SpeechError> {
        if !inner.current(epoch) {
            return Ok(());
        }
        let device = player.open()?;
        // Kept across a sentence's pieces, and made again if the rate on
        // either side changes: a different voice, or a different device.
        if !matches!(&resampler, Some((from, _, to)) if *from == rate && *to == device) {
            resampler = Some((rate, Resampler::new(rate, device), device));
        }
        let Some((_, resample, _)) = resampler.as_mut() else {
            return Ok(());
        };
        player.play(&resample.process(samples))
    };
    let result = tokio::select! {
        result = job.engine.synthesize(&inner.http, &job.text, &mut play) => result,
        _ = cancel.cancelled() => Ok(()),
    };
    if let Err(e) = result {
        if inner.current(epoch) {
            tracing::warn!(error = %e, "speech failed");
            inner.stop();
            (inner.on_event)(SpeechEvent::Failed(e.to_string()));
        }
    }
}
