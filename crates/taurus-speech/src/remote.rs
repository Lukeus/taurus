//! Voices from a speech service: ElevenLabs, or anything shaped like OpenAI's
//! `/v1/audio/speech`.
//!
//! Both are asked for raw 16-bit PCM at 24 kHz, `pcm_24000` and `pcm`
//! respectively, and stream it. So there's no decoder in Taurus, and a
//! sentence starts playing when its first bytes arrive rather than when the
//! last one does.
//!
//! What's sent is the sentence and nothing else: no conversation, no file
//! names beyond what the reply itself said. Settings says so, because it's
//! still the reply's text going to a third party.

use std::time::Duration;

use futures::StreamExt;
use reqwest::{Response, StatusCode};

pub use reqwest::Url;

use crate::system::VoiceInfo;
use crate::SpeechError;

/// The rate both services are asked for.
pub const RATE: u32 = 24_000;

/// Where ElevenLabs lives.
pub const ELEVENLABS_URL: &str = "https://api.elevenlabs.io";

/// Where streamed audio goes, at [`RATE`], as it arrives.
pub type Sink<'a> = dyn FnMut(&[f32]) -> Result<(), SpeechError> + Send + 'a;

/// How long one sentence may take to arrive in full.
const SENTENCE_WITHIN: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElevenLabs {
    pub base_url: String,
    pub voice_id: String,
    pub model: String,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAi {
    /// Up to and including the version, `https://api.openai.com/v1`, the way
    /// `providers.json` writes it.
    pub base_url: String,
    pub model: String,
    pub voice: String,
    /// `None` for a local server that takes no key.
    pub key: Option<String>,
}

/// The voices OpenAI's speech models have. Suggestions: a server of the same
/// shape has its own, and Settings takes any name.
pub const OPENAI_VOICES: [&str; 11] = [
    "alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse",
];

pub fn openai_voices() -> Vec<VoiceInfo> {
    OPENAI_VOICES
        .iter()
        .map(|v| VoiceInfo {
            id: v.to_string(),
            name: v.to_string(),
            detail: String::new(),
        })
        .collect()
}

/// The voices on an ElevenLabs account: its own, and the premade ones.
pub async fn elevenlabs_voices(
    http: &reqwest::Client,
    base_url: &str,
    key: &str,
) -> Result<Vec<VoiceInfo>, SpeechError> {
    let url = join(base_url, &["v1", "voices"])?;
    let response = http
        .get(url.clone())
        .header("xi-api-key", key)
        .timeout(SENTENCE_WITHIN)
        .send()
        .await
        .map_err(|e| unreachable(&url, &e))?;
    if !response.status().is_success() {
        return Err(refused("ElevenLabs", None, response).await);
    }
    let body: serde_json::Value = response.json().await.map_err(|e| {
        SpeechError::Service(format!(
            "ElevenLabs sent a voice list Taurus can't read: {e}"
        ))
    })?;
    let voices = body["voices"].as_array().cloned().unwrap_or_default();
    Ok(voices
        .iter()
        .filter_map(|v| {
            let id = v["voice_id"].as_str()?;
            let labels = ["accent", "gender", "description"]
                .iter()
                .filter_map(|k| v["labels"][k].as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Some(VoiceInfo {
                id: id.to_string(),
                name: v["name"].as_str().unwrap_or(id).to_string(),
                detail: if labels.is_empty() {
                    v["category"].as_str().unwrap_or_default().to_string()
                } else {
                    labels
                },
            })
        })
        .collect())
}

/// Streams `text` in an ElevenLabs voice, handing audio to `play` as it
/// arrives, at [`RATE`].
pub async fn elevenlabs(
    http: &reqwest::Client,
    voice: &ElevenLabs,
    text: &str,
    play: &mut Sink<'_>,
) -> Result<(), SpeechError> {
    let mut url = join(
        &voice.base_url,
        &["v1", "text-to-speech", &voice.voice_id, "stream"],
    )?;
    url.query_pairs_mut()
        .append_pair("output_format", "pcm_24000");
    let response = http
        .post(url.clone())
        .header("xi-api-key", &voice.key)
        .json(&serde_json::json!({ "text": text, "model_id": voice.model }))
        .timeout(SENTENCE_WITHIN)
        .send()
        .await
        .map_err(|e| unreachable(&url, &e))?;
    if !response.status().is_success() {
        return Err(refused("ElevenLabs", Some(&voice.voice_id), response).await);
    }
    stream_pcm(response, &url, play).await
}

/// Streams `text` from an OpenAI-shaped `/audio/speech`, at [`RATE`].
pub async fn openai(
    http: &reqwest::Client,
    voice: &OpenAi,
    text: &str,
    play: &mut Sink<'_>,
) -> Result<(), SpeechError> {
    let url = join(&voice.base_url, &["audio", "speech"])?;
    let mut request = http.post(url.clone()).json(&serde_json::json!({
        "model": voice.model,
        "input": text,
        "voice": voice.voice,
        "response_format": "pcm",
    }));
    if let Some(key) = &voice.key {
        request = request.bearer_auth(key);
    }
    let response = request
        .timeout(SENTENCE_WITHIN)
        .send()
        .await
        .map_err(|e| unreachable(&url, &e))?;
    if !response.status().is_success() {
        return Err(refused(&service_name(&url), None, response).await);
    }
    stream_pcm(response, &url, play).await
}

async fn stream_pcm(response: Response, url: &Url, play: &mut Sink<'_>) -> Result<(), SpeechError> {
    let mut bytes = response.bytes_stream();
    // A chunk can end halfway through a sample.
    let mut odd: Option<u8> = None;
    while let Some(chunk) = bytes.next().await {
        let chunk = chunk.map_err(|e| unreachable(url, &e))?;
        let mut data = Vec::with_capacity(chunk.len() + 1);
        data.extend(odd.take());
        data.extend_from_slice(&chunk);
        if data.len() % 2 == 1 {
            odd = data.pop();
        }
        play(&crate::wav::from_i16_le(&data))?;
    }
    Ok(())
}

/// `base` with `segments` added to its path, whatever it ended in.
fn join(base: &str, segments: &[&str]) -> Result<Url, SpeechError> {
    let mut url = Url::parse(base.trim()).map_err(|e| {
        SpeechError::Service(format!(
            "\"{base}\" isn't a URL Taurus can use ({e}). Fix it in Settings › Voice."
        ))
    })?;
    url.path_segments_mut()
        .map_err(|_| SpeechError::Service(format!("\"{base}\" can't have a path added to it.")))?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}

/// What to call a service in a message: OpenAI by name, anything else by host.
fn service_name(url: &Url) -> String {
    match url.host_str() {
        Some("api.openai.com") => "OpenAI".to_string(),
        Some(host) => host.to_string(),
        None => url.to_string(),
    }
}

fn unreachable(url: &Url, e: &reqwest::Error) -> SpeechError {
    let host = url.host_str().unwrap_or("the speech service");
    let what = if e.is_timeout() {
        "it didn't answer in time".to_string()
    } else {
        // The innermost cause says the useful thing: "connection refused".
        let mut cause: &dyn std::error::Error = e;
        while let Some(next) = cause.source() {
            cause = next;
        }
        cause.to_string()
    };
    SpeechError::Service(format!(
        "Couldn't reach {host}: {what}. Check your connection, or the address in Settings › Voice."
    ))
}

/// A refusal as something to do about it.
async fn refused(service: &str, voice: Option<&str>, response: Response) -> SpeechError {
    let status = response.status();
    let url = response.url().clone();
    let body = response.text().await.unwrap_or_default();
    let said = said(&body).unwrap_or_else(|| {
        status
            .canonical_reason()
            .unwrap_or("no reason given")
            .to_string()
    });
    SpeechError::Service(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => format!(
            "{service} didn't accept the API key: {said}. Check the key in Settings › Voice."
        ),
        StatusCode::NOT_FOUND => match voice {
            Some(voice) => {
                format!("{service} has no voice {voice}: {said}. Pick another in Settings › Voice.")
            }
            None => format!(
                "{url} wasn't found: {said}. Check the address in Settings › Voice; most \
                 servers want it to end in /v1."
            ),
        },
        StatusCode::TOO_MANY_REQUESTS => {
            format!("{service} is limiting requests or the account is out of credit: {said}")
        }
        _ => format!("{service} answered {status}: {said}"),
    })
}

/// The message in an error body, from whichever of the usual places it's in.
fn said(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let found = [
        &json["detail"]["message"],
        &json["detail"],
        &json["error"]["message"],
        &json["error"],
        &json["message"],
    ]
    .into_iter()
    .find_map(|v| v.as_str().map(str::to_string));
    found.filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serves one request with `status` and `body` sent in `pieces`, and
    /// returns the request as text.
    async fn serve(
        status: &str,
        pieces: Vec<Vec<u8>>,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let status = status.to_string();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= head_end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let total: usize = pieces.iter().map(Vec::len).sum();
            let head = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {total}\r\nconnection: close\r\n\r\n"
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            for piece in pieces {
                socket.write_all(&piece).await.unwrap();
                socket.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            String::from_utf8_lossy(&request).to_string()
        });
        (base, handle)
    }

    type Heard = std::sync::Arc<std::sync::Mutex<Vec<f32>>>;

    fn collect() -> (Heard, impl FnMut(&[f32]) -> Result<(), SpeechError> + Send) {
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&got);
        (got, move |s: &[f32]| {
            sink.lock().unwrap().extend_from_slice(s);
            Ok(())
        })
    }

    #[tokio::test]
    async fn elevenlabs_streams_pcm_split_mid_sample() {
        let samples: Vec<u8> = [16384i16, -16384, 0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let (base, server) =
            serve("200 OK", vec![samples[..3].to_vec(), samples[3..].to_vec()]).await;
        let voice = ElevenLabs {
            base_url: base,
            voice_id: "abc123".into(),
            model: "eleven_flash_v2_5".into(),
            key: "xi-secret".into(),
        };
        let (got, mut play) = collect();
        elevenlabs(&reqwest::Client::new(), &voice, "Hello.", &mut play)
            .await
            .unwrap();
        assert_eq!(*got.lock().unwrap(), [0.5, -0.5, 0.0]);
        let request = server.await.unwrap();
        assert!(
            request.starts_with("POST /v1/text-to-speech/abc123/stream?output_format=pcm_24000 "),
            "{request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("xi-api-key: xi-secret"),
            "{request}"
        );
        assert!(
            request.contains(r#""model_id":"eleven_flash_v2_5""#),
            "{request}"
        );
    }

    #[tokio::test]
    async fn openai_shaped_asks_for_pcm_and_sends_the_key_only_when_there_is_one() {
        let (base, server) = serve("200 OK", vec![vec![0, 0]]).await;
        let voice = OpenAi {
            base_url: format!("{base}/v1/"),
            model: "kokoro".into(),
            voice: "af_bella".into(),
            key: None,
        };
        let (_, mut play) = collect();
        openai(&reqwest::Client::new(), &voice, "Hi.", &mut play)
            .await
            .unwrap();
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /v1/audio/speech "), "{request}");
        assert!(request.contains(r#""response_format":"pcm""#), "{request}");
        assert!(
            !request.to_ascii_lowercase().contains("authorization"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn a_refused_key_says_where_to_fix_it() {
        let body =
            br#"{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}"#.to_vec();
        let (base, _server) = serve("401 Unauthorized", vec![body]).await;
        let voice = ElevenLabs {
            base_url: base,
            voice_id: "v".into(),
            model: "m".into(),
            key: "bad".into(),
        };
        let (_, mut play) = collect();
        let err = elevenlabs(&reqwest::Client::new(), &voice, "Hi.", &mut play)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "ElevenLabs didn't accept the API key: Invalid API key. Check the key in Settings › Voice."
        );
    }

    #[tokio::test]
    async fn a_wrong_address_suggests_the_version() {
        let (base, _server) = serve(
            "404 Not Found",
            vec![b"{\"error\":{\"message\":\"nope\"}}".to_vec()],
        )
        .await;
        let voice = OpenAi {
            base_url: base,
            model: "m".into(),
            voice: "v".into(),
            key: Some("k".into()),
        };
        let (_, mut play) = collect();
        let err = openai(&reqwest::Client::new(), &voice, "Hi.", &mut play)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("/audio/speech wasn't found: nope"), "{err}");
        assert!(err.contains("end in /v1"), "{err}");
    }

    #[tokio::test]
    async fn nobody_listening_is_said_plainly() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        drop(listener);
        let voice = OpenAi {
            base_url: base,
            model: "m".into(),
            voice: "v".into(),
            key: None,
        };
        let (_, mut play) = collect();
        let err = openai(&reqwest::Client::new(), &voice, "Hi.", &mut play)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("Couldn't reach 127.0.0.1:"), "{err}");
    }

    #[tokio::test]
    async fn elevenlabs_voices_are_listed_with_their_labels() {
        let body = br#"{"voices":[{"voice_id":"a1","name":"Aria","category":"premade","labels":{"accent":"american","gender":"female"}},{"voice_id":"b2","name":"Mine","category":"cloned","labels":{}}]}"#.to_vec();
        let (base, _server) = serve("200 OK", vec![body]).await;
        let voices = elevenlabs_voices(&reqwest::Client::new(), &base, "k")
            .await
            .unwrap();
        assert_eq!(voices[0].detail, "american, female");
        assert_eq!(voices[1].detail, "cloned");
    }

    #[test]
    fn a_base_url_joins_with_or_without_a_slash() {
        assert_eq!(
            join("https://api.openai.com/v1", &["audio", "speech"])
                .unwrap()
                .as_str(),
            "https://api.openai.com/v1/audio/speech"
        );
        assert_eq!(
            join("https://api.openai.com/v1/", &["audio", "speech"])
                .unwrap()
                .as_str(),
            "https://api.openai.com/v1/audio/speech"
        );
        assert!(join("not a url", &["x"])
            .unwrap_err()
            .to_string()
            .contains("Settings › Voice"));
    }
}
