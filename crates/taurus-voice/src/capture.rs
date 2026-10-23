//! Recording from the default microphone, push-to-talk.
//!
//! The stream lives on a thread of its own and never leaves it. A cpal stream
//! isn't `Send` on every platform, and holding it on a dedicated thread is
//! what lets the caller be async code that moves between workers. The thread
//! does three things: opens the device and says whether that worked, reports
//! the level while it records, and hands the audio back when told to stop.
//!
//! Only while asked. There's no always-on listening and no wake word; the
//! microphone is open between a press and a release, and the audio exists in
//! memory until it's transcribed.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, Sample, SampleFormat, SizedSample};

use crate::resample::{to_whisper_rate, WHISPER_RATE};
use crate::VoiceError;

/// The longest recording kept. Past this the microphone stays open but what
/// it hears is dropped, and [`Captured::truncated`] says so.
///
/// Five minutes is longer than anybody dictates a message, and bounds memory
/// at about 60 MB of 48 kHz floats.
pub const MAX_SECONDS: u32 = 300;

/// How often the level is reported.
const LEVEL_EVERY: Duration = Duration::from_millis(50);

/// How long opening the device may take before giving up on it.
///
/// Opening normally takes milliseconds. It can block indefinitely inside the
/// platform's audio service instead: on macOS a stuck `coreaudiod` holds
/// every caller, recording and playback alike, and nothing returns until it
/// recovers. A press that hangs the composer would be worse than one that
/// says so.
const OPEN_WITHIN: Duration = Duration::from_secs(10);

/// What a recording heard, ready for whisper.cpp.
#[derive(Clone, Debug, Default)]
pub struct Captured {
    /// Mono, at 16 kHz.
    pub samples: Vec<f32>,
    /// The loudest sample, from 0 to 1, before resampling.
    pub peak: f32,
    /// Whether the recording ran past [`MAX_SECONDS`] and lost its end.
    pub truncated: bool,
}

impl Captured {
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / WHISPER_RATE as f32
    }

    /// How much of the recording is louder than a quiet room, in seconds.
    ///
    /// Counted in 30 ms frames against a fixed floor. Not a voice detector, and
    /// not meant to be: it's the cheap check that stops whisper.cpp being
    /// handed silence, which it answers with a confident "Thank you." more
    /// often than with nothing.
    pub fn voiced_seconds(&self) -> f32 {
        const FRAME: usize = WHISPER_RATE as usize * 30 / 1000;
        const FLOOR: f32 = 0.01;
        let voiced = self
            .samples
            .chunks(FRAME)
            .filter(|frame| {
                let energy = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
                energy.sqrt() > FLOOR
            })
            .count();
        voiced as f32 * 0.03
    }
}

/// The name of the microphone a recording would use, if there is one.
pub fn input_device_name() -> Option<String> {
    let device = cpal::default_host().default_input_device()?;
    device
        .description()
        .ok()
        .map(|d| d.name().to_string())
        .or_else(|| device.id().ok().map(|id| id.to_string()))
}

/// A recording in progress. Dropping it stops the microphone and discards the
/// audio, the same as [`Recording::discard`].
pub struct Recording {
    stop: Option<mpsc::Sender<bool>>,
    thread: Option<JoinHandle<Result<Captured, VoiceError>>>,
}

impl Recording {
    /// Stops the microphone and returns what it heard.
    pub fn finish(mut self) -> Result<Captured, VoiceError> {
        self.end(true)
    }

    /// Stops the microphone and throws the audio away.
    pub fn discard(mut self) {
        let _ = self.end(false);
    }

    fn end(&mut self, keep: bool) -> Result<Captured, VoiceError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(keep);
        }
        match self.thread.take() {
            Some(thread) => thread
                .join()
                .unwrap_or_else(|_| Err(VoiceError::Lost("the recording thread crashed".into()))),
            None => Err(VoiceError::NotListening),
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.end(false);
        }
    }
}

/// Opens the default microphone and starts recording.
///
/// Returns once the device is open and running, or with the reason it
/// couldn't be — so a missing microphone or a refused permission is an error
/// from the press, not from the release.
pub fn start(on_level: impl Fn(f32) + Send + 'static) -> Result<Recording, VoiceError> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), VoiceError>>();
    let (stop_tx, stop_rx) = mpsc::channel::<bool>();

    let thread = std::thread::Builder::new()
        .name("taurus-voice-capture".into())
        .spawn(move || run(ready_tx, stop_rx, on_level))
        .map_err(VoiceError::Io)?;

    match ready_rx.recv_timeout(OPEN_WITHIN) {
        Ok(Ok(())) => Ok(Recording {
            stop: Some(stop_tx),
            thread: Some(thread),
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        // Left running, and harmless: if the device does open later, the
        // thread finds `stop_tx` gone and closes it straight away.
        Err(mpsc::RecvTimeoutError::Timeout) => Err(VoiceError::Stalled),
        // The thread ended without saying anything, which only a panic does.
        Err(mpsc::RecvTimeoutError::Disconnected) => match thread.join() {
            Ok(Err(e)) => Err(e),
            _ => Err(VoiceError::Lost("the recording thread crashed".into())),
        },
    }
}

/// Everything the audio callback shares with the thread that owns it.
struct Shared {
    samples: Mutex<Vec<f32>>,
    /// The loudest sample since the level was last read, as `f32` bits.
    level: AtomicU32,
    peak: AtomicU32,
    truncated: AtomicBool,
    /// The first error that ended the stream, if one did.
    failed: Mutex<Option<String>>,
    channels: usize,
    limit: usize,
}

fn run(
    ready: mpsc::Sender<Result<(), VoiceError>>,
    stop: mpsc::Receiver<bool>,
    on_level: impl Fn(f32),
) -> Result<Captured, VoiceError> {
    let opened = open();
    let (stream, shared, rate) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            let _ = ready.send(Err(e));
            return Err(VoiceError::NotListening);
        }
    };
    if let Err(e) = stream.play() {
        let _ = ready.send(Err(stream_error("play", &e)));
        return Err(VoiceError::NotListening);
    }
    let _ = ready.send(Ok(()));

    let keep = loop {
        match stop.recv_timeout(LEVEL_EVERY) {
            Ok(keep) => break keep,
            // The handle was dropped without a word: treat it as a discard.
            Err(RecvTimeoutError::Disconnected) => break false,
            Err(RecvTimeoutError::Timeout) => {
                on_level(f32::from_bits(shared.level.swap(0, Ordering::Relaxed)));
            }
        }
    };
    drop(stream);

    if !keep {
        return Ok(Captured::default());
    }
    if let Some(reason) = shared
        .failed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        return Err(VoiceError::Lost(reason));
    }
    let samples = std::mem::take(&mut *shared.samples.lock().unwrap_or_else(|e| e.into_inner()));
    Ok(Captured {
        samples: to_whisper_rate(&samples, rate),
        peak: f32::from_bits(shared.peak.load(Ordering::Relaxed)),
        truncated: shared.truncated.load(Ordering::Relaxed),
    })
}

type Opened = (cpal::Stream, Arc<Shared>, u32);

fn open() -> Result<Opened, VoiceError> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or(VoiceError::NoDevice)?;
    let config = device
        .default_input_config()
        .map_err(|e| stream_error("read the settings of", &e))?;
    let rate = config.sample_rate();
    let channels = config.channels().max(1) as usize;
    let shared = Arc::new(Shared {
        samples: Mutex::new(Vec::with_capacity(rate as usize * 30)),
        level: AtomicU32::new(0),
        peak: AtomicU32::new(0),
        truncated: AtomicBool::new(false),
        failed: Mutex::new(None),
        channels,
        limit: rate as usize * MAX_SECONDS as usize,
    });

    let format = config.sample_format();
    let config: cpal::StreamConfig = config.into();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, &shared),
        SampleFormat::I16 => build::<i16>(&device, &config, &shared),
        SampleFormat::U16 => build::<u16>(&device, &config, &shared),
        SampleFormat::I32 => build::<i32>(&device, &config, &shared),
        SampleFormat::I8 => build::<i8>(&device, &config, &shared),
        SampleFormat::U8 => build::<u8>(&device, &config, &shared),
        SampleFormat::F64 => build::<f64>(&device, &config, &shared),
        other => {
            return Err(VoiceError::Device {
                device: name_of(&device),
                reason: format!("it records in {other}, which Taurus can't read"),
            })
        }
    }
    .map_err(|e| stream_error("open", &e))?;
    Ok((stream, shared, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let data = Arc::clone(shared);
    let errors = Arc::clone(shared);
    device.build_input_stream::<T, _, _>(
        *config,
        move |input: &[T], _| take(&data, input),
        move |err| {
            // Glitches are reported and survived. Only an error that ends the
            // stream is worth failing the recording over.
            match err.kind() {
                ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {
                    tracing::debug!(%err, "microphone glitch");
                }
                _ => {
                    tracing::warn!(%err, "microphone stream failed");
                    let mut failed = errors.failed.lock().unwrap_or_else(|e| e.into_inner());
                    failed.get_or_insert_with(|| err.to_string());
                }
            }
        },
        None,
    )
}

/// The audio callback: mix to mono, keep it, and note how loud it was.
fn take<T>(shared: &Shared, input: &[T])
where
    T: Sample,
    f32: FromSample<T>,
{
    let mut loudest = 0f32;
    let mut mono = Vec::with_capacity(input.len() / shared.channels + 1);
    for frame in input.chunks(shared.channels) {
        let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
        let sample = sum / frame.len() as f32;
        loudest = loudest.max(sample.abs());
        mono.push(sample);
    }
    // Levels are non-negative, and non-negative floats order the same way as
    // their bits, so `fetch_max` on the bits is a max on the values.
    let bits = loudest.min(1.0).to_bits();
    shared.level.fetch_max(bits, Ordering::Relaxed);
    shared.peak.fetch_max(bits, Ordering::Relaxed);

    let mut samples = shared.samples.lock().unwrap_or_else(|e| e.into_inner());
    let room = shared.limit.saturating_sub(samples.len());
    if mono.len() > room {
        shared.truncated.store(true, Ordering::Relaxed);
    }
    samples.extend(mono.into_iter().take(room));
}

fn name_of(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "the default input device".to_string())
}

/// A cpal error as the error the person should see.
fn stream_error(doing: &str, e: &cpal::Error) -> VoiceError {
    match e.kind() {
        ErrorKind::PermissionDenied => VoiceError::PermissionDenied,
        ErrorKind::DeviceNotAvailable | ErrorKind::HostUnavailable => VoiceError::NoDevice,
        ErrorKind::DeviceBusy => VoiceError::Device {
            device: "the default input device".into(),
            reason: "another app is using it".into(),
        },
        _ => VoiceError::Device {
            device: "the default input device".into(),
            reason: format!("couldn't {doing} it: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(channels: usize, limit: usize) -> Shared {
        Shared {
            samples: Mutex::new(Vec::new()),
            level: AtomicU32::new(0),
            peak: AtomicU32::new(0),
            truncated: AtomicBool::new(false),
            failed: Mutex::new(None),
            channels,
            limit,
        }
    }

    #[test]
    fn stereo_is_mixed_to_mono() {
        let s = shared(2, 100);
        take(&s, &[0.5f32, -0.5, 1.0, 0.0]);
        assert_eq!(*s.samples.lock().unwrap(), vec![0.0, 0.5]);
    }

    #[test]
    fn integer_samples_are_scaled() {
        let s = shared(1, 100);
        take(&s, &[i16::MAX, 0, i16::MIN]);
        let got = s.samples.lock().unwrap().clone();
        assert!((got[0] - 1.0).abs() < 1e-3 && got[1] == 0.0 && (got[2] + 1.0).abs() < 1e-3);
    }

    #[test]
    fn level_is_the_loudest_since_last_read() {
        let s = shared(1, 100);
        take(&s, &[0.1f32, -0.6, 0.2]);
        take(&s, &[0.3f32]);
        assert_eq!(f32::from_bits(s.level.swap(0, Ordering::Relaxed)), 0.6);
        take(&s, &[0.2f32]);
        assert_eq!(f32::from_bits(s.level.load(Ordering::Relaxed)), 0.2);
        assert_eq!(f32::from_bits(s.peak.load(Ordering::Relaxed)), 0.6);
    }

    #[test]
    fn past_the_limit_is_dropped_and_said() {
        let s = shared(1, 3);
        take(&s, &[0.1f32, 0.1]);
        assert!(!s.truncated.load(Ordering::Relaxed));
        take(&s, &[0.2f32, 0.2]);
        assert_eq!(s.samples.lock().unwrap().len(), 3);
        assert!(s.truncated.load(Ordering::Relaxed));
    }

    #[test]
    fn silence_has_no_voiced_seconds() {
        let quiet = Captured {
            samples: vec![0.001; WHISPER_RATE as usize * 2],
            ..Default::default()
        };
        assert_eq!(quiet.voiced_seconds(), 0.0);
        assert!((quiet.seconds() - 2.0).abs() < 1e-6);

        let mut talking = vec![0.0; WHISPER_RATE as usize];
        for (n, s) in talking
            .iter_mut()
            .enumerate()
            .take(WHISPER_RATE as usize / 2)
        {
            *s = (n as f32 / 10.0).sin() * 0.3;
        }
        let half = Captured {
            samples: talking,
            ..Default::default()
        };
        assert!(
            (half.voiced_seconds() - 0.5).abs() < 0.05,
            "{}",
            half.voiced_seconds()
        );
    }
}
