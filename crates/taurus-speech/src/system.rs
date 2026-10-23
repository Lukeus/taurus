//! The voices that come with the operating system.
//!
//! Nothing to download, no account and nothing sent anywhere, which is why
//! they're the default. Each platform's is reached the way that needs nothing
//! linked in or installed by Taurus:
//!
//! - **macOS**: `say`, writing a WAV file. Every voice in System Settings ›
//!   Accessibility › Spoken Content is there, including downloaded Enhanced
//!   and Premium ones. About half a second per sentence goes on starting the
//!   process, which the next sentence's synthesis overlaps with this one's
//!   playback.
//! - **Windows**: the WinRT speech synthesizer, the voices in Settings › Time
//!   & language › Speech.
//! - **Linux**: `espeak-ng`, or `espeak`, writing to its stdout. Robotic next
//!   to the others, but it's what distributions package. Taurus doesn't link
//!   it: it's GPL, and a program on `PATH` is a different matter from one in
//!   the binary.

use serde::Serialize;
use ts_rs::TS;

use crate::wav::{self, Pcm};
use crate::SpeechError;

/// One voice, as Settings lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct VoiceInfo {
    /// What to pass back to choose it.
    pub id: String,
    pub name: String,
    /// A language tag or a description, whichever the source has.
    pub detail: String,
}

/// The system's voices.
pub async fn voices() -> Result<Vec<VoiceInfo>, SpeechError> {
    imp::voices().await
}

/// `text` in `voice`, or the system's default voice when that's `None`.
pub async fn synthesize(text: &str, voice: Option<&str>) -> Result<Pcm, SpeechError> {
    imp::synthesize(text, voice).await
}

#[cfg(target_os = "macos")]
mod imp {
    use std::process::Stdio;

    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    use super::*;

    pub async fn voices() -> Result<Vec<VoiceInfo>, SpeechError> {
        let out = Command::new("say")
            .args(["-v", "?"])
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| SpeechError::System(format!("couldn't run say: {e}")))?;
        Ok(parse_say_voices(&String::from_utf8_lossy(&out.stdout)))
    }

    pub async fn synthesize(text: &str, voice: Option<&str>) -> Result<Pcm, SpeechError> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("speech.wav");
        let mut command = Command::new("say");
        if let Some(voice) = voice {
            command.args(["-v", voice]);
        }
        // From stdin, so a sentence that starts with a dash isn't an option.
        let mut child = command
            .arg("-o")
            .arg(&path)
            .args(["--data-format=LEI16@24000", "-f", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| SpeechError::System(format!("couldn't run say: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes()).await?;
        }
        let out = child.wait_with_output().await?;
        if !out.status.success() {
            let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(match voice {
                // `say` reports an unknown voice as "Opening output file
                // failed: fmt?", which points at the wrong thing entirely.
                Some(voice) => SpeechError::System(format!(
                    "macOS couldn't speak with the voice \"{voice}\" ({said}). Pick another \
                     in Settings › Voice, or download it in System Settings › Accessibility › \
                     Spoken Content."
                )),
                None => SpeechError::System(format!("say failed: {said}")),
            });
        }
        wav::read(&tokio::fs::read(&path).await?)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::process::Stdio;

    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    use super::*;

    const PROGRAMS: [&str; 2] = ["espeak-ng", "espeak"];

    fn missing() -> SpeechError {
        SpeechError::System(
            "Linux has no built-in voice Taurus can use. Install espeak-ng (for example \
             `sudo apt install espeak-ng`), or choose ElevenLabs or an OpenAI-compatible \
             service in Settings › Voice."
                .into(),
        )
    }

    /// The first of [`PROGRAMS`] that runs, and what it printed.
    async fn run(args: &[&str], input: Option<&str>) -> Result<std::process::Output, SpeechError> {
        for program in PROGRAMS {
            let spawned = Command::new(program)
                .args(args)
                .stdin(if input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn();
            let mut child = match spawned {
                Ok(child) => child,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(SpeechError::System(format!("couldn't run {program}: {e}"))),
            };
            if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
                stdin.write_all(text.as_bytes()).await?;
            }
            let out = child.wait_with_output().await?;
            if !out.status.success() {
                return Err(SpeechError::System(format!(
                    "{program} failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            return Ok(out);
        }
        Err(missing())
    }

    pub async fn voices() -> Result<Vec<VoiceInfo>, SpeechError> {
        let out = run(&["--voices"], None).await?;
        Ok(parse_espeak_voices(&String::from_utf8_lossy(&out.stdout)))
    }

    pub async fn synthesize(text: &str, voice: Option<&str>) -> Result<Pcm, SpeechError> {
        let mut args = vec!["--stdout"];
        if let Some(voice) = voice {
            args.extend(["-v", voice]);
        }
        let out = run(&args, Some(text)).await?;
        wav::read(&out.stdout)
    }
}

#[cfg(windows)]
mod imp {
    use windows::core::HSTRING;
    use windows::Media::SpeechSynthesis::SpeechSynthesizer;
    use windows::Storage::Streams::DataReader;

    use super::*;

    fn failed(doing: &str, e: windows::core::Error) -> SpeechError {
        SpeechError::System(format!("Windows couldn't {doing}: {e}"))
    }

    pub async fn voices() -> Result<Vec<VoiceInfo>, SpeechError> {
        tokio::task::spawn_blocking(|| {
            let all = SpeechSynthesizer::AllVoices().map_err(|e| failed("list its voices", e))?;
            let mut out = Vec::new();
            for voice in all {
                out.push(VoiceInfo {
                    id: voice.Id().map(|s| s.to_string()).unwrap_or_default(),
                    name: voice
                        .DisplayName()
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    detail: voice.Language().map(|s| s.to_string()).unwrap_or_default(),
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| SpeechError::System(e.to_string()))?
    }

    pub async fn synthesize(text: &str, voice: Option<&str>) -> Result<Pcm, SpeechError> {
        let text = text.to_string();
        let voice = voice.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let synth = SpeechSynthesizer::new().map_err(|e| failed("start its speech synthesizer", e))?;
            if let Some(id) = voice {
                let all = SpeechSynthesizer::AllVoices().map_err(|e| failed("list its voices", e))?;
                let found = all
                    .into_iter()
                    .find(|v| v.Id().map(|s| s.to_string() == id).unwrap_or(false))
                    .ok_or_else(|| {
                        SpeechError::System(format!(
                            "Windows has no voice \"{id}\" any more. Pick another in Settings › Voice."
                        ))
                    })?;
                synth.SetVoice(&found).map_err(|e| failed("choose the voice", e))?;
            }
            let stream = synth
                .SynthesizeTextToStreamAsync(&HSTRING::from(text.as_str()))
                .and_then(|op| op.join())
                .map_err(|e| failed("speak", e))?;
            let size = stream.Size().map_err(|e| failed("read the speech", e))? as u32;
            let input = stream.GetInputStreamAt(0).map_err(|e| failed("read the speech", e))?;
            let reader = DataReader::CreateDataReader(&input).map_err(|e| failed("read the speech", e))?;
            reader
                .LoadAsync(size)
                .and_then(|op| op.join())
                .map_err(|e| failed("read the speech", e))?;
            let mut bytes = vec![0u8; size as usize];
            reader.ReadBytes(&mut bytes).map_err(|e| failed("read the speech", e))?;
            wav::read(&bytes)
        })
        .await
        .map_err(|e| SpeechError::System(e.to_string()))?
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    use super::*;

    fn missing() -> SpeechError {
        SpeechError::System(
            "This system has no built-in voice Taurus can use. Choose ElevenLabs or an \
             OpenAI-compatible service in Settings › Voice."
                .into(),
        )
    }

    pub async fn voices() -> Result<Vec<VoiceInfo>, SpeechError> {
        Err(missing())
    }

    pub async fn synthesize(_text: &str, _voice: Option<&str>) -> Result<Pcm, SpeechError> {
        Err(missing())
    }
}

/// `say -v ?`: a name padded to a column, a locale, then `#` and a sample.
/// Names can have spaces and brackets in them: `Eddy (English (US))`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_say_voices(listing: &str) -> Vec<VoiceInfo> {
    listing
        .lines()
        .filter_map(|line| {
            let (left, _) = line.split_once('#')?;
            let left = left.trim_end();
            let (name, locale) = left.rsplit_once(char::is_whitespace)?;
            let name = name.trim();
            (!name.is_empty()).then(|| VoiceInfo {
                id: name.to_string(),
                name: name.to_string(),
                detail: locale.to_string(),
            })
        })
        .collect()
}

/// `espeak-ng --voices`: a header, then `Pty Language Age/Gender VoiceName
/// File Other Languages`. The language is what `-v` takes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_espeak_voices(listing: &str) -> Vec<VoiceInfo> {
    listing
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let language = fields.get(1)?;
            let name = fields.get(3)?;
            Some(VoiceInfo {
                id: language.to_string(),
                name: name.replace('_', " "),
                detail: language.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn say_voices_keep_names_with_brackets() {
        let listing = "Albert              en_US    # Hello! My name is Albert.\n\
                       Eddy (English (US)) en_US    # Hello! My name is Eddy.\n\
                       not a voice line\n";
        assert_eq!(
            parse_say_voices(listing),
            [
                VoiceInfo {
                    id: "Albert".into(),
                    name: "Albert".into(),
                    detail: "en_US".into()
                },
                VoiceInfo {
                    id: "Eddy (English (US))".into(),
                    name: "Eddy (English (US))".into(),
                    detail: "en_US".into()
                },
            ]
        );
    }

    #[test]
    fn espeak_voices_are_chosen_by_language() {
        let listing = "Pty Language       Age/Gender VoiceName          File                 Other Languages\n \
                        5  af              --/M      Afrikaans          gmw/af\n \
                        2  en-us           --/M      English_(America)  gmw/en-US            (en 3)\n";
        let voices = parse_espeak_voices(listing);
        assert_eq!(voices.len(), 2);
        assert_eq!(voices[1].id, "en-us");
        assert_eq!(voices[1].name, "English (America)");
    }
}
