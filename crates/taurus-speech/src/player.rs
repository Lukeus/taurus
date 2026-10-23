//! Playing speech through the default output device.
//!
//! Every engine ends here, whatever it is: the system voice's WAV file and a
//! service's streamed PCM alike are turned into mono floats at the device's
//! rate and queued. One queue is what makes stopping one thing to do, and
//! what lets the next sentence be synthesized while this one plays.
//!
//! The stream lives on a thread of its own, for the same reason the
//! microphone's does: a cpal stream isn't `Send` everywhere. The thread closes
//! the device a few seconds after the queue runs dry, so an app that spoke
//! once doesn't hold the audio hardware awake for the rest of the day.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample};

use crate::SpeechError;

/// How long opening the device may take. See the microphone's `OPEN_WITHIN`
/// in `taurus-voice`: a stuck audio service holds playback as well.
const OPEN_WITHIN: Duration = Duration::from_secs(10);

/// How long the device stays open with nothing to play.
const CLOSE_AFTER: Duration = Duration::from_secs(3);

#[derive(Clone, Default)]
pub struct Player {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    /// Mono, at `rate`.
    queue: VecDeque<f32>,
    /// The open device's rate, while one is open.
    rate: Option<u32>,
    /// When something was last queued or played.
    active: Option<Instant>,
    /// Why the stream died, if it did. Reported on the next push.
    failed: Option<String>,
}

impl Player {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the default output device if it isn't open, and returns its rate.
    pub fn open(&self) -> Result<u32, SpeechError> {
        {
            let mut state = self.lock();
            if let Some(reason) = state.failed.take() {
                return Err(SpeechError::Lost(reason));
            }
            if let Some(rate) = state.rate {
                state.active = Some(Instant::now());
                return Ok(rate);
            }
        }
        let (ready_tx, ready_rx) = mpsc::channel();
        let shared = Arc::clone(&self.shared);
        std::thread::Builder::new()
            .name("taurus-speech-output".into())
            .spawn(move || run(shared, ready_tx))
            .map_err(SpeechError::Io)?;
        match ready_rx.recv_timeout(OPEN_WITHIN) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(SpeechError::Stalled),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(SpeechError::Lost("the playback thread crashed".into()))
            }
        }
    }

    /// Queues audio already at the rate [`Self::open`] returned. Reopens the
    /// device first if it closed in the meantime, which a service that went
    /// quiet for longer than [`CLOSE_AFTER`] mid-sentence can cause.
    pub fn play(&self, samples: &[f32]) -> Result<(), SpeechError> {
        if samples.is_empty() {
            return Ok(());
        }
        if self.lock().rate.is_none() {
            self.open()?;
        }
        let mut state = self.lock();
        state.queue.extend(samples.iter().copied());
        state.active = Some(Instant::now());
        Ok(())
    }

    /// Drops everything queued. What the device is playing right now stops
    /// within one buffer.
    pub fn clear(&self) {
        self.lock().queue.clear();
        self.shared.wake.notify_all();
    }

    /// How much is queued, in seconds.
    pub fn buffered(&self) -> f32 {
        let state = self.lock();
        match state.rate {
            Some(rate) if rate > 0 => state.queue.len() as f32 / rate as f32,
            _ => 0.0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn run(shared: Arc<Shared>, ready: mpsc::Sender<Result<u32, SpeechError>>) {
    let (stream, rate) = match open(&shared) {
        Ok(opened) => opened,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    if let Err(e) = stream.play() {
        let _ = ready.send(Err(stream_error("start", &e)));
        return;
    }
    {
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.rate = Some(rate);
        state.active = Some(Instant::now());
    }
    if ready.send(Ok(rate)).is_err() {
        // Whoever asked gave up waiting. Close again rather than sit open.
        shared.state.lock().unwrap_or_else(|e| e.into_inner()).rate = None;
        return;
    }

    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let quiet =
            state.queue.is_empty() && state.active.is_none_or(|at| at.elapsed() >= CLOSE_AFTER);
        if quiet || state.failed.is_some() {
            // Under the lock, so a push either sees the device open and is
            // played, or sees it closed and opens it again.
            state.rate = None;
            break;
        }
        state = shared
            .wake
            .wait_timeout(state, Duration::from_millis(250))
            .map(|(guard, _)| guard)
            .unwrap_or_else(|e| e.into_inner().0);
    }
    drop(state);
    drop(stream);
}

fn open(shared: &Arc<Shared>) -> Result<(cpal::Stream, u32), SpeechError> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(SpeechError::NoDevice)?;
    let config = device
        .default_output_config()
        .map_err(|e| stream_error("read the settings of", &e))?;
    let rate = config.sample_rate();
    let channels = config.channels().max(1) as usize;
    let format = config.sample_format();
    let config: cpal::StreamConfig = config.into();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, shared, channels),
        SampleFormat::I16 => build::<i16>(&device, &config, shared, channels),
        SampleFormat::U16 => build::<u16>(&device, &config, shared, channels),
        SampleFormat::I32 => build::<i32>(&device, &config, shared, channels),
        SampleFormat::I8 => build::<i8>(&device, &config, shared, channels),
        SampleFormat::U8 => build::<u8>(&device, &config, shared, channels),
        SampleFormat::F64 => build::<f64>(&device, &config, shared, channels),
        other => {
            return Err(SpeechError::Device(format!(
                "it plays {other} samples, which Taurus can't write"
            )))
        }
    }
    .map_err(|e| stream_error("open", &e))?;
    Ok((stream, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: &Arc<Shared>,
    channels: usize,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let data = Arc::clone(shared);
    let errors = Arc::clone(shared);
    device.build_output_stream::<T, _, _>(
        *config,
        move |out: &mut [T], _| fill(&data, out, channels),
        move |err| match err.kind() {
            ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {
                tracing::debug!(%err, "speaker glitch");
            }
            _ => {
                tracing::warn!(%err, "speaker stream failed");
                let mut state = errors.state.lock().unwrap_or_else(|e| e.into_inner());
                state.failed.get_or_insert_with(|| err.to_string());
                state.queue.clear();
                errors.wake.notify_all();
            }
        },
        None,
    )
}

/// The audio callback: the next samples, the same on every channel, and
/// silence when there's nothing queued.
fn fill<T>(shared: &Shared, out: &mut [T], channels: usize)
where
    T: SizedSample + FromSample<f32>,
{
    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    let mut played = false;
    for frame in out.chunks_mut(channels) {
        let sample = match state.queue.pop_front() {
            Some(s) => {
                played = true;
                s
            }
            None => 0.0,
        };
        let sample = T::from_sample(sample.clamp(-1.0, 1.0));
        frame.iter_mut().for_each(|slot| *slot = sample);
    }
    if played {
        state.active = Some(Instant::now());
    }
}

fn stream_error(doing: &str, e: &cpal::Error) -> SpeechError {
    match e.kind() {
        ErrorKind::DeviceNotAvailable | ErrorKind::HostUnavailable => SpeechError::NoDevice,
        ErrorKind::DeviceBusy => SpeechError::Device("another app is using it".into()),
        _ => SpeechError::Device(format!("couldn't {doing} it: {e}")),
    }
}

/// Linear interpolation from one rate to another, a piece at a time.
///
/// A service streams a sentence in pieces of whatever size the network
/// delivered, so the position between input samples carries over from one
/// piece to the next. Speech is all this is for, and from 22 or 24 kHz up to
/// a device's 44.1 or 48 it's inaudible next to a proper filter.
#[derive(Debug)]
pub struct Resampler {
    step: f64,
    /// Where the next output sample falls, in input samples, counted from
    /// `last` at zero.
    pos: f64,
    last: f32,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self {
            step: from.max(1) as f64 / to.max(1) as f64,
            pos: 0.0,
            last: 0.0,
        }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if (self.step - 1.0).abs() < f64::EPSILON {
            return input.to_vec();
        }
        let n = input.len();
        let mut out = Vec::with_capacity((n as f64 / self.step) as usize + 1);
        while self.pos < n as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = if i == 0 { self.last } else { input[i - 1] };
            let b = input[i];
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.pos -= n as f64;
        if let Some(&last) = input.last() {
            self.last = last;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_rate_is_untouched() {
        let mut r = Resampler::new(24_000, 24_000);
        assert_eq!(r.process(&[0.1, 0.2]), [0.1, 0.2]);
    }

    #[test]
    fn doubling_the_rate_interpolates_between_samples() {
        let mut r = Resampler::new(24_000, 48_000);
        let out = r.process(&[1.0, 1.0, 0.0]);
        assert_eq!(out, [0.0, 0.5, 1.0, 1.0, 1.0, 0.5]);
    }

    #[test]
    fn pieces_come_out_the_same_as_the_whole() {
        let input: Vec<f32> = (0..1000).map(|n| (n as f32 / 7.0).sin()).collect();
        let whole = Resampler::new(22_050, 48_000).process(&input);
        let mut pieces = Resampler::new(22_050, 48_000);
        let mut joined = Vec::new();
        for piece in input.chunks(37) {
            joined.extend(pieces.process(piece));
        }
        assert_eq!(whole.len(), joined.len());
        for (a, b) in whole.iter().zip(&joined) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn the_callback_plays_mono_on_every_channel_then_silence() {
        let shared = Shared::default();
        shared.state.lock().unwrap().queue.extend([0.5, -0.5]);
        let mut out = [9.0f32; 6];
        fill(&shared, &mut out, 2);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5, 0.0, 0.0]);
    }

    #[test]
    fn integer_devices_get_scaled_samples() {
        let shared = Shared::default();
        shared.state.lock().unwrap().queue.extend([1.0, -1.0]);
        let mut out = [0i16; 2];
        fill(&shared, &mut out, 1);
        assert_eq!(out, [i16::MAX, i16::MIN]);
    }
}
