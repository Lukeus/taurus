//! Live check for speech output: the filter, a voice, and the speakers.
//!
//! ```text
//! cargo run -p taurus-speech --example speak -- "Hello. This is Taurus."
//! cargo run -p taurus-speech --example speak -- --file reply.md --dry      # sentences only
//! cargo run -p taurus-speech --example speak -- --list                     # system voices
//! cargo run -p taurus-speech --example speak -- --voice Samantha "Hi."
//! cargo run -p taurus-speech --example speak -- --stop-after 1.5 "…"       # must go quiet
//! ELEVENLABS_API_KEY=… cargo run -p taurus-speech --example speak -- --engine elevenlabs --list
//! ELEVENLABS_API_KEY=… cargo run -p taurus-speech --example speak -- --engine elevenlabs --voice <id> "Hi."
//! OPENAI_API_KEY=… cargo run -p taurus-speech --example speak -- --engine openai --voice coral "Hi."
//! cargo run -p taurus-speech --example speak -- --engine openai --base-url http://localhost:8880/v1 \
//!     --model kokoro --voice af_bella "Hi."
//! ```
//!
//! Prints each sentence as the filter hands it out, how long the first sound
//! took, and how long the whole thing took. `--stop-after` stops partway and
//! fails if the speaker still says it's speaking afterwards.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use taurus_speech::{ElevenLabs, Engine, OpenAi, Speaker, SpeechEvent, ELEVENLABS_URL};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let mut engine = "system".to_string();
    let mut voice: Option<String> = None;
    let mut model: Option<String> = None;
    let mut base_url: Option<String> = None;
    let mut text = String::new();
    let mut list = false;
    let mut dry = false;
    let mut stop_after: Option<f32> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--engine" => engine = value()?,
            "--voice" => voice = Some(value()?),
            "--model" => model = Some(value()?),
            "--base-url" => base_url = Some(value()?),
            "--file" => {
                let path = value()?;
                text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
            }
            "--stop-after" => {
                stop_after = Some(value()?.parse().map_err(|_| "--stop-after takes seconds")?)
            }
            "--list" => list = true,
            "--dry" => dry = true,
            _ => text = arg,
        }
    }

    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let engine = match engine.as_str() {
        "system" => Engine::System { voice },
        "elevenlabs" => Engine::ElevenLabs(ElevenLabs {
            base_url: base_url.unwrap_or_else(|| ELEVENLABS_URL.into()),
            voice_id: voice.unwrap_or_default(),
            model: model.unwrap_or_else(|| "eleven_flash_v2_5".into()),
            key: env("ELEVENLABS_API_KEY").ok_or("set ELEVENLABS_API_KEY")?,
        }),
        "openai" => Engine::OpenAi(OpenAi {
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".into()),
            model: model.unwrap_or_else(|| "gpt-4o-mini-tts".into()),
            voice: voice.unwrap_or_else(|| "alloy".into()),
            key: env("OPENAI_API_KEY"),
        }),
        other => {
            return Err(format!(
                "unknown engine {other}: system, elevenlabs or openai"
            ))
        }
    };

    if list {
        let voices = engine
            .voices(&reqwest::Client::new())
            .await
            .map_err(|e| e.to_string())?;
        for v in &voices {
            println!("{:<40} {:<24} {}", v.id, v.name, v.detail);
        }
        println!("{} voices", voices.len());
        return Ok(());
    }
    if text.trim().is_empty() {
        return Err("nothing to say: pass some text or --file".into());
    }

    let sentences = taurus_speech::sentences(&text);
    for (n, s) in sentences.iter().enumerate() {
        println!("{:>3}  {s}", n + 1);
    }
    if dry {
        return Ok(());
    }

    let events: Arc<Mutex<Vec<(Duration, SpeechEvent)>>> = Arc::default();
    let started = Instant::now();
    let log = Arc::clone(&events);
    let speaker = Speaker::new(move |e| log.lock().unwrap().push((started.elapsed(), e)));
    speaker.say(Arc::new(engine), &text);

    // The first sound is when the player first holds audio, which the
    // speaker doesn't report; polling for it is close enough for a check.
    let mut first_sound: Option<Duration> = None;
    loop {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if first_sound.is_none() && speaker_has_audio(&speaker) {
            first_sound = Some(started.elapsed());
        }
        if let Some(after) = stop_after {
            if started.elapsed().as_secs_f32() >= after {
                speaker.stop();
                tokio::time::sleep(Duration::from_millis(300)).await;
                if speaker.speaking() {
                    return Err("still speaking 300 ms after stop".into());
                }
                println!("stopped at {after:.1}s and went quiet");
                break;
            }
        }
        let failed = events.lock().unwrap().iter().find_map(|(_, e)| match e {
            SpeechEvent::Failed(why) => Some(why.clone()),
            _ => None,
        });
        if let Some(why) = failed {
            return Err(why);
        }
        if started.elapsed() > Duration::from_millis(300) && !speaker.speaking() {
            break;
        }
    }
    if let Some(first) = first_sound {
        println!("first sound after {} ms", first.as_millis());
    }
    println!("done after {:.1} s", started.elapsed().as_secs_f32());
    for (at, event) in events.lock().unwrap().iter() {
        println!("  {:>6} ms  {event:?}", at.as_millis());
    }
    Ok(())
}

fn speaker_has_audio(speaker: &Speaker) -> bool {
    speaker.buffered() > 0.0
}
