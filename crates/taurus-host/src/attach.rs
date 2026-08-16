//! Images on the way in.
//!
//! Every provider adapter has been able to *send* an image since the normalized
//! types were written — `ContentBlock::Image` maps onto Ollama's `images`
//! array, OpenAI's `image_url`, Anthropic's `source`, and Gemini's
//! `inline_data` without loss. What none of them had was a way for one to
//! arrive. This is that way.
//!
//! It is one function, and almost all of it is refusal. An image that reaches a
//! provider and is rejected there costs a round trip and comes back as a wire
//! error naming a field, which is the least useful moment for a person holding
//! a screenshot to be told their screenshot is the wrong shape. Everything
//! checkable is checked here instead, before the turn starts, in words about
//! the picture rather than about the request body.
//!
//! # What is checked
//!
//! The model can see at all, the format is one every backend accepts, the bytes
//! are really that format, and there are not too many or too large. The order
//! matters: the vision check comes first, because on a model that cannot see,
//! every other complaint is beside the point.

use base64::Engine;
use taurus_provider::{Capabilities, ContentBlock};

/// Largest single image, decoded.
///
/// Anthropic refuses past 5 MB and the others are looser, so this is the
/// binding constraint rather than a house rule — a limit picked here that let
/// something through would only move the failure to somewhere it reads worse.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Images one message may carry.
///
/// Small on purpose. [`taurus_core::session::estimate_block`] budgets an image
/// at a flat 1000 tokens because its real cost has nothing to do with its
/// base64 length, and this harness is built for models with an 8k window — four
/// images is already half of one. Past that the pictures crowd out the
/// conversation they were sent to illustrate.
pub const MAX_IMAGES: usize = 4;

/// Formats every backend here accepts.
///
/// The intersection, not the union. Gemini would take HEIC and Anthropic would
/// not, and a format that works until the day someone switches provider is
/// worse than one that never worked — the failure arrives detached from the
/// change that caused it.
const ACCEPTED: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];

/// One image as a frontend hands it over.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Attachment {
    /// What the sender believes this is. Checked against the bytes — see
    /// [`sniff`].
    pub mime_type: String,
    /// Base64, no `data:` prefix. The same encoding
    /// [`ContentBlock::Image`] carries, so nothing is re-encoded on the way
    /// through.
    pub data: String,
}

/// Turns attachments into content blocks, or says why not.
///
/// The blocks come back in order and are meant to precede the user's text: a
/// model reads "what is wrong with this?" better after the picture than before
/// it, and every adapter here preserves block order.
pub fn to_blocks(
    attachments: &[Attachment],
    capabilities: &Capabilities,
) -> Result<Vec<ContentBlock>, String> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }

    // First, because on a model that cannot see, every other complaint about
    // the image is beside the point.
    if !capabilities.vision {
        return Err(format!(
            "This model cannot read images. Switch to one that can — on Ollama that is a \
             vision model such as qwen3-vl or llava — or send {} as text.",
            if attachments.len() == 1 {
                "the image"
            } else {
                "the images"
            }
        ));
    }

    if attachments.len() > MAX_IMAGES {
        return Err(format!(
            "{} images is more than one message can carry ({MAX_IMAGES} at most). Each one costs \
             about as much of the context window as a page of text.",
            attachments.len()
        ));
    }

    attachments
        .iter()
        .enumerate()
        .map(|(n, attachment)| block(n + 1, attachment))
        .collect()
}

fn block(position: usize, attachment: &Attachment) -> Result<ContentBlock, String> {
    let declared = attachment.mime_type.trim().to_ascii_lowercase();
    if !ACCEPTED.contains(&declared.as_str()) {
        return Err(format!(
            "Image {position} is {}, which no model here accepts. Use PNG, JPEG, WebP, or GIF.",
            if declared.is_empty() {
                "of no stated type".into()
            } else {
                declared
            }
        ));
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(attachment.data.as_bytes())
        .map_err(|e| format!("Image {position} is not valid base64 ({e})."))?;

    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "Image {position} is {:.1} MB, past the {} MB limit. Scale it down or crop to the part \
             that matters.",
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_IMAGE_BYTES / (1024 * 1024)
        ));
    }
    if bytes.is_empty() {
        return Err(format!("Image {position} is empty."));
    }

    // The bytes are the authority. A frontend guesses the type from a clipboard
    // flavour or a file extension, and both are wrong often enough to matter —
    // a `.png` that is really a JPEG reaches the provider, is rejected there,
    // and comes back as a wire error about a field name.
    match sniff(&bytes) {
        Some(actual) if actual != declared => {
            return Err(format!(
                "Image {position} says it is {declared} but the bytes are {actual}. Re-export it, \
                 or send it with the right type."
            ))
        }
        // Not sniffable and not refused: the accepted list already bounds this
        // to four formats, and a magic-number table that grew a gap would
        // reject a perfectly good picture for a reason nobody could act on.
        _ => {}
    }

    Ok(ContentBlock::Image {
        mime_type: declared,
        data: attachment.data.clone(),
    })
}

/// The format the bytes actually are, for the four this accepts.
///
/// Deliberately answers `None` rather than guessing: an unrecognized header is
/// only ever used to *skip* the comparison, never to refuse.
fn sniff(bytes: &[u8]) -> Option<String> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    if bytes.starts_with(PNG) {
        return Some("image/png".into());
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg".into());
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif".into());
    }
    // RIFF....WEBP — the size field sits between the two markers.
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn png_bytes() -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(b"and then some pixels");
        bytes
    }

    fn png() -> Attachment {
        Attachment {
            mime_type: "image/png".into(),
            data: encode(&png_bytes()),
        }
    }

    fn seeing() -> Capabilities {
        Capabilities {
            vision: true,
            ..Capabilities::default()
        }
    }

    #[test]
    fn a_valid_png_becomes_an_image_block_unchanged() {
        // Nothing is re-encoded on the way through: the base64 a frontend sends
        // is the base64 the provider gets.
        let attachment = png();
        let blocks = to_blocks(std::slice::from_ref(&attachment), &seeing()).unwrap();
        match &blocks[..] {
            [ContentBlock::Image { mime_type, data }] => {
                assert_eq!(mime_type, "image/png");
                assert_eq!(data, &attachment.data);
            }
            other => panic!("expected one image block, got {other:?}"),
        }
    }

    #[test]
    fn a_model_that_cannot_see_is_the_first_thing_said() {
        // Every other complaint about the picture is beside the point, and the
        // message has to name the fix rather than the fault.
        let error = to_blocks(&[png()], &Capabilities::default()).unwrap_err();
        assert!(error.contains("cannot read images"), "{error}");
        assert!(
            error.contains("qwen3-vl") || error.contains("llava"),
            "{error}"
        );
    }

    #[test]
    fn no_attachments_is_not_a_failure_even_on_a_blind_model() {
        // Every ordinary turn takes this path. Checking vision before checking
        // whether there is anything to check would refuse plain text.
        assert!(to_blocks(&[], &Capabilities::default()).unwrap().is_empty());
    }

    #[test]
    fn a_file_that_lies_about_its_type_is_caught_here_rather_than_on_the_wire() {
        // A clipboard flavour and a file extension are both wrong often enough
        // to matter. Left alone, this reaches the provider and comes back as an
        // error naming a field in the request body.
        let lying = Attachment {
            mime_type: "image/png".into(),
            data: encode(&[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]),
        };
        let error = to_blocks(&[lying], &seeing()).unwrap_err();
        assert!(error.contains("says it is image/png"), "{error}");
        assert!(error.contains("image/jpeg"), "{error}");
    }

    #[test]
    fn an_unsniffable_format_on_the_accepted_list_is_allowed_through() {
        // The magic-number table is used to refuse a mismatch, never to demand
        // a match. A gap in it must not reject a perfectly good picture for a
        // reason nobody could act on.
        let odd = Attachment {
            mime_type: "image/webp".into(),
            data: encode(b"not really a webp header but claimed as one"),
        };
        assert!(to_blocks(&[odd], &seeing()).is_ok());
    }

    #[test]
    fn a_webp_is_recognized_past_its_size_field() {
        // RIFF....WEBP: the four bytes between the markers are a length, so a
        // naive `starts_with` on the whole signature would never match.
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&[0x24, 0x00, 0x00, 0x00]);
        bytes.extend_from_slice(b"WEBPVP8 ");
        assert_eq!(sniff(&bytes).as_deref(), Some("image/webp"));
    }

    #[test]
    fn a_format_no_backend_takes_is_refused_by_name() {
        let heic = Attachment {
            mime_type: "image/heic".into(),
            data: encode(b"whatever"),
        };
        let error = to_blocks(&[heic], &seeing()).unwrap_err();
        assert!(error.contains("image/heic"), "{error}");
        assert!(error.contains("PNG, JPEG, WebP, or GIF"), "{error}");
    }

    #[test]
    fn an_image_past_the_size_limit_says_how_big_it_is() {
        // "Too large" without the number leaves someone guessing how much to
        // crop.
        let huge = Attachment {
            mime_type: "image/png".into(),
            data: encode(&vec![0u8; MAX_IMAGE_BYTES + 1024]),
        };
        let error = to_blocks(&[huge], &seeing()).unwrap_err();
        assert!(error.contains("5.0 MB"), "{error}");
    }

    #[test]
    fn more_images_than_the_cap_is_refused_before_any_are_decoded() {
        let many: Vec<Attachment> = (0..MAX_IMAGES + 1).map(|_| png()).collect();
        let error = to_blocks(&many, &seeing()).unwrap_err();
        assert!(error.contains("context window"), "{error}");
    }

    #[test]
    fn base64_that_is_not_base64_names_the_image_it_came_from() {
        let broken = Attachment {
            mime_type: "image/png".into(),
            data: "!!! not base64 !!!".into(),
        };
        let error = to_blocks(&[png(), broken], &seeing()).unwrap_err();
        assert!(error.contains("Image 2"), "{error}");
    }

    #[test]
    fn a_type_is_matched_however_it_was_capitalized() {
        // Clipboard flavours arrive as `image/PNG` on some platforms, and
        // refusing that would be a rejection nobody could act on.
        let shouty = Attachment {
            mime_type: "  IMAGE/PNG ".into(),
            data: encode(&png_bytes()),
        };
        let blocks = to_blocks(&[shouty], &seeing()).unwrap();
        assert!(matches!(
            &blocks[0],
            ContentBlock::Image { mime_type, .. } if mime_type == "image/png"
        ));
    }
}
