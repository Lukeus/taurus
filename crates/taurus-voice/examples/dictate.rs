//! Live check for speech input: the download, the microphone, and whisper.cpp.
//!
//! ```text
//! cargo run --release -p taurus-voice --example dictate                 # 5 s from the mic
//! cargo run --release -p taurus-voice --example dictate -- --seconds 10
//! cargo run --release -p taurus-voice --example dictate -- --wav speech.wav
//! cargo run --release -p taurus-voice --example dictate -- --model small.en --hint "Taurus, taurus-core"
//! cargo run --release -p taurus-voice --example dictate -- --wav speech.wav --abort   # must cancel
//! ```
//!
//! Uses the same model directory as the app (`~/.taurus/models/whisper`, or
//! under `TAURUS_HOME`), and downloads the model there first if it's missing.
//! `--wav` takes 16-bit PCM WAV at any rate and channel count; on macOS,
//! `say -o s.aiff "…" && afconvert -f WAVE -d LEI16@16000 s.aiff s.wav` makes
//! one. Release mode matters: a debug build of the resampler is slow enough to
//! skew the numbers.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use taurus_voice::{Captured, Voice, VoiceError, DEFAULT_MODEL};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), VoiceError> {
    let mut model = DEFAULT_MODEL.to_string();
    let mut seconds = 5u64;
    let mut wav: Option<PathBuf> = None;
    let mut hint = "Taurus.".to_string();
    let mut abort = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => model = args.next().unwrap_or_default(),
            "--seconds" => seconds = args.next().and_then(|s| s.parse().ok()).unwrap_or(5),
            "--wav" => wav = args.next().map(PathBuf::from),
            "--hint" => hint = args.next().unwrap_or_default(),
            // Raises the abort flag before transcribing, which must end in
            // "canceled" rather than in text or a whisper error code.
            "--abort" => abort = true,
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    let home = std::env::var_os("TAURUS_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".taurus")))
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".taurus")))
        .expect("no home directory");
    let voice = Voice::new(home.join("models").join("whisper"));
    let spec = taurus_voice::spec(&model)?;

    if !voice.models().installed(spec) {
        println!(
            "downloading {} ({} MB) from {}",
            spec.id,
            spec.bytes / 1_000_000,
            spec.url()
        );
        let started = Instant::now();
        let last = std::cell::Cell::new(0);
        voice
            .models()
            .download(spec, &CancellationToken::new(), |done, total| {
                let pct = done * 100 / total.max(1);
                if pct >= last.get() + 10 {
                    last.set(pct);
                    println!("  {pct}%");
                }
            })
            .await?;
        println!("downloaded and verified in {:.1?}", started.elapsed());
    }

    let audio = match wav {
        Some(path) => read_wav(&path)?,
        None => {
            println!(
                "microphone: {}",
                taurus_voice::input_device_name().unwrap_or("none".into())
            );
            println!("talk for {seconds} seconds…");
            let loudest = Arc::new(std::sync::Mutex::new(0f32));
            let seen = Arc::clone(&loudest);
            voice.start(move |level| {
                let mut l = seen.lock().unwrap();
                *l = l.max(level);
            })?;
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            let audio = voice.stop()?;
            println!(
                "loudest level reported while recording: {:.3}",
                *loudest.lock().unwrap()
            );
            audio
        }
    };
    println!(
        "audio: {:.1} s, {:.1} s voiced, peak {:.3}{}",
        audio.seconds(),
        audio.voiced_seconds(),
        audio.peak,
        if audio.truncated { ", truncated" } else { "" }
    );

    // Twice: the first includes loading the model, the second is what every
    // dictation after it costs.
    for run in ["first (loads the model)", "second (model loaded)"] {
        let started = Instant::now();
        let text = voice.transcribe(&model, &audio, &hint, Arc::new(AtomicBool::new(abort)))?;
        println!("{run}: {:.0?}", started.elapsed());
        println!("  {text}");
    }
    Ok(())
}

/// A 16-bit PCM WAV, mixed to mono and brought to 16 kHz.
fn read_wav(path: &std::path::Path) -> Result<Captured, VoiceError> {
    let bytes = std::fs::read(path)?;
    let bad = |why: &str| VoiceError::Transcribe(format!("{}: {why}", path.display()));
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("not a WAV file"));
    }
    let (mut channels, mut rate, mut data) = (0usize, 0u32, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + len).min(bytes.len())];
        match id {
            b"fmt " => {
                if u16::from_le_bytes([body[0], body[1]]) != 1
                    || u16::from_le_bytes([body[14], body[15]]) != 16
                {
                    return Err(bad("only 16-bit PCM is read here"));
                }
                channels = u16::from_le_bytes([body[2], body[3]]) as usize;
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at += 8 + len + (len & 1);
    }
    let data = data.ok_or_else(|| bad("no data chunk"))?;
    if channels == 0 {
        return Err(bad("no fmt chunk"));
    }
    let mono: Vec<f32> = data
        .chunks_exact(2 * channels)
        .map(|frame| {
            frame
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
                .sum::<f32>()
                / channels as f32
        })
        .collect();
    let peak = mono.iter().fold(0f32, |m, s| m.max(s.abs()));
    Ok(Captured {
        samples: taurus_voice::to_whisper_rate(&mono, rate),
        peak,
        truncated: false,
    })
}
