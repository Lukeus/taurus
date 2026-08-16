//! An image, attached and answered — end to end against a real model.
//!
//! Every provider adapter could already *send* an image, and the unit tests
//! cover the checks on the way in. What neither proves is the whole path:
//! base64 from a frontend, through [`taurus_host::attach`], into a
//! `ContentBlock::Image`, out through an adapter's own encoding, and back as an
//! answer that could only have come from looking.
//!
//! ```sh
//! cargo run -p taurus-host --example vision -- gemma4:12b
//! cargo run -p taurus-host --example vision -- llama3.2:latest   # refused, and why
//! ```
//!
//! The picture is 96×96, red over blue. Two facts rather than one, so a model
//! that guesses a colour has to guess both and get the order right.

use std::sync::Arc;

use taurus_host::attach::{to_blocks, Attachment};
use taurus_provider::{ChatRequest, Message, Provider, Role, StreamEvent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A 96×96 PNG: the top half red, the bottom half blue.
///
/// Inline rather than generated, because what is under test is the path an
/// image takes, not this repository's ability to write a PNG encoder.
const RED_OVER_BLUE: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAGAAAABgCAIAAABt+uBvAAAAkklEQVR42u3QQQkAAAgEMKNcFPun\
MIoN/AuDJVhNwqEUCBIkSJAgQYIEIUiQIEGCBAkShCBBggQJEiRIEIIECRIkSJAgQYIQJEiQIEGC\
BAlCkCBBggQ9CUoPB0GCBAkSJEiQIEEIEiRIkCBBggQhSJAgQYIECRKEIEGCBAkSJEiQIAQJEiRI\
kCBBghAkSJAgQU8snW7Cd79zyN4AAAAASUVORK5CYII=";

#[tokio::main]
async fn main() {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "gemma4:12b".into());
    let base = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
    let provider: Arc<dyn Provider> = Arc::new(taurus_provider_ollama::OllamaProvider::new(base));

    let capabilities = provider
        .capabilities(&model)
        .await
        .unwrap_or_else(|e| panic!("could not probe {model}: {e}"));
    println!("{model}: vision={}", capabilities.vision);

    let attachment = Attachment {
        mime_type: "image/png".into(),
        // The base64 is split across source lines for readability; the wire
        // format has no newlines in it.
        data: RED_OVER_BLUE.replace(['\n', ' '], ""),
    };

    // The gate a frontend hits before a turn starts. On a model that cannot
    // see, this is the whole run — and saying so clearly is the point.
    let mut content = match to_blocks(std::slice::from_ref(&attachment), &capabilities) {
        Ok(blocks) => blocks,
        Err(refusal) => {
            println!("\nrefused before the turn started:\n  {refusal}");
            return;
        }
    };
    content.push(taurus_provider::ContentBlock::text(
        "This image has two horizontal bands. Name the colour of the top band and the colour of \
         the bottom band, in that order. Answer in under ten words.",
    ));

    println!("  {} content blocks, image first", content.len());

    let (tx, mut rx) = mpsc::channel(64);
    let request = ChatRequest {
        model: model.clone(),
        system: None,
        messages: vec![Message::new(Role::User, content)],
        tools: Vec::new(),
        temperature: Some(0.0),
        max_tokens: None,
        stop_sequences: Vec::new(),
    };

    let streaming = tokio::spawn(async move {
        let mut answer = String::new();
        while let Some(event) = rx.recv().await {
            if let StreamEvent::TextDelta { text } = event {
                answer.push_str(&text);
            }
        }
        answer
    });

    let stop = provider
        .stream(request, tx, CancellationToken::new())
        .await
        .unwrap_or_else(|e| panic!("the turn failed: {e}"));
    let answer = streaming.await.unwrap();

    println!("\nanswer ({stop:?}):\n  {}", answer.trim());

    // The check worth making: both colours, in the right order. A model that
    // received no image at all still answers confidently, so "it replied" is
    // not evidence of anything.
    let said = answer.to_lowercase();
    let red = said.find("red");
    let blue = said.find("blue");
    match (red, blue) {
        (Some(red), Some(blue)) if red < blue => println!("\nsaw it: red above blue"),
        (Some(_), Some(_)) => println!("\nboth colours named, wrong way round"),
        _ => println!("\nthe image did not get through, or the model could not read it"),
    }
}
