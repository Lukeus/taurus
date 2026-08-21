//! What every backend here will accept as a picture, and how to tell.
//!
//! Shared rather than duplicated because there are now two ways an image gets
//! into a conversation — pasted by the person, or handed back by a tool — and
//! they have to agree about what an image is. Two copies of this would drift,
//! and the drift would show up as a screenshot that works when you paste it and
//! fails when a tool returns it, which is a bug nobody would think to look for
//! here.
//!
//! The messages stay with the callers. The rules are the same for both paths;
//! the audience is not, and "scale it down or crop to the part that matters" is
//! advice for a person while a tool needs to be told what its own output did
//! wrong.

use base64::Engine;

/// Largest single image, decoded.
///
/// Anthropic refuses past 5 MB and the others are looser, so this is the
/// binding constraint rather than a house rule — a limit picked here that let
/// something through would only move the failure to somewhere it reads worse.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Formats every backend here accepts.
///
/// The intersection, not the union. Gemini would take HEIC and Anthropic would
/// not, and a format that works until the day someone switches provider is
/// worse than one that never worked — the failure arrives detached from the
/// change that caused it.
pub const ACCEPTED: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];

/// Whether this is one of the four, however the caller capitalized it.
pub fn is_accepted(mime_type: &str) -> bool {
    ACCEPTED.contains(&mime_type.trim().to_ascii_lowercase().as_str())
}

/// The format the bytes actually are, for the four this accepts.
///
/// Deliberately answers `None` rather than guessing: an unrecognized header is
/// only ever used to *skip* the comparison, never to refuse. A magic-number
/// table with a gap in it would otherwise turn a perfectly good picture away
/// for a reason nobody could act on.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    if bytes.starts_with(PNG) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    // RIFF....WEBP — the size field sits between the two markers.
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// What is wrong with an image, in terms of the picture rather than the
/// request body.
///
/// An enum rather than a string because the two callers phrase the same
/// finding for different readers — a person holding a screenshot, and a model
/// that has just produced one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// The declared type is not one of [`ACCEPTED`].
    UnknownFormat,
    /// The base64 does not decode.
    NotBase64,
    /// Nothing there.
    Empty,
    /// Decoded size, past [`MAX_IMAGE_BYTES`].
    TooLarge { bytes: usize },
    /// The header says one thing and the declared type says another.
    Mismatch { actual: &'static str },
}

/// Everything checkable about an image without looking at the picture.
///
/// Answers with the decoded length on success, which is the number both callers
/// want next — one to report a budget, the other to decide whether to keep it.
pub fn check(mime_type: &str, data: &str) -> Result<usize, Rejected> {
    let declared = mime_type.trim().to_ascii_lowercase();
    if !ACCEPTED.contains(&declared.as_str()) {
        return Err(Rejected::UnknownFormat);
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| Rejected::NotBase64)?;

    if bytes.is_empty() {
        return Err(Rejected::Empty);
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(Rejected::TooLarge { bytes: bytes.len() });
    }

    // The bytes are the authority. A caller guesses the type from a clipboard
    // flavour, a file extension, or whatever an MCP server put in the field,
    // and all three are wrong often enough to matter — a `.png` that is really
    // a JPEG reaches the provider, is rejected there, and comes back as a wire
    // error about a field name.
    match sniff(&bytes) {
        Some(actual) if actual != declared => Err(Rejected::Mismatch { actual }),
        _ => Ok(bytes.len()),
    }
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

    #[test]
    fn a_real_png_declared_as_one_passes_and_reports_its_size() {
        let bytes = png_bytes();
        assert_eq!(check("image/png", &encode(&bytes)), Ok(bytes.len()));
    }

    #[test]
    fn the_declared_type_is_read_however_it_was_capitalized() {
        assert!(check("Image/PNG", &encode(&png_bytes())).is_ok());
        assert!(is_accepted(" image/JPEG "));
    }

    #[test]
    fn the_bytes_outrank_what_the_sender_called_them() {
        // The case this exists for: a screenshot saved as .png that is really a
        // JPEG. Caught here, it is one sentence; caught by the provider, it is
        // a wire error naming a field.
        let jpeg = encode(&[0xff, 0xd8, 0xff, 0x00, 0x11]);
        assert_eq!(
            check("image/png", &jpeg),
            Err(Rejected::Mismatch {
                actual: "image/jpeg"
            })
        );
    }

    #[test]
    fn a_header_nobody_recognizes_is_allowed_through() {
        // Sniffing only ever skips the comparison. A format on the accepted
        // list whose magic number is not in the table above must not be refused
        // for a reason the sender could do nothing about.
        assert!(check("image/webp", &encode(b"not really a webp at all")).is_ok());
    }

    #[test]
    fn each_way_of_being_wrong_is_reported_as_itself() {
        assert_eq!(check("image/heic", "abcd"), Err(Rejected::UnknownFormat));
        assert_eq!(check("image/png", "not base64!"), Err(Rejected::NotBase64));
        assert_eq!(check("image/png", ""), Err(Rejected::Empty));
        let huge = encode(&vec![0u8; MAX_IMAGE_BYTES + 1]);
        assert!(matches!(
            check("image/png", &huge),
            Err(Rejected::TooLarge { .. })
        ));
    }

    #[test]
    fn every_accepted_format_can_be_recognized_from_its_header() {
        // Not a round trip for its own sake: `check` compares `sniff` against
        // the declared type, so a format on the list that sniffs as something
        // else would refuse every correctly-declared image of that kind.
        assert_eq!(sniff(&png_bytes()), Some("image/png"));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a...."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBP...."), Some("image/webp"));
        assert_eq!(sniff(b"plain text"), None);
    }
}
