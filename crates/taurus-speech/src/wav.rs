//! Reading the WAV files the system voices write.
//!
//! Each platform's voice hands back a whole RIFF file, and none of them write
//! the same one. macOS's `say` puts a `JUNK` and an `FLLR` chunk before the
//! format. `espeak-ng --stdout` writes to a pipe it can't seek back on, so the
//! sizes in its header are placeholders. Windows writes a plain one. So this
//! walks the chunks rather than trusting offsets, and takes the data chunk as
//! running to the end of the file when its size says more than is there.

use crate::SpeechError;

/// Mono audio and its rate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub samples: Vec<f32>,
}

/// 16-bit little-endian mono, which is what both speech services send when
/// asked for `pcm`.
pub fn from_i16_le(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect()
}

/// A whole WAV file as mono floats.
pub fn read(bytes: &[u8]) -> Result<Pcm, SpeechError> {
    let bad = |why: &str| {
        SpeechError::Audio(format!(
            "the voice wrote a WAV file Taurus can't read: {why}"
        ))
    };
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("it isn't RIFF/WAVE"));
    }
    let mut at = 12;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let body = at + 8;
        if id == b"fmt " {
            if body + 16 > bytes.len() {
                return Err(bad("its format chunk is cut short"));
            }
            let u16_at = |n: usize| u16::from_le_bytes([bytes[body + n], bytes[body + n + 1]]);
            let rate = u32::from_le_bytes([
                bytes[body + 4],
                bytes[body + 5],
                bytes[body + 6],
                bytes[body + 7],
            ]);
            format = Some((u16_at(0), u16_at(2), rate, u16_at(14)));
        } else if id == b"data" {
            let (tag, channels, rate, bits) =
                format.ok_or_else(|| bad("its data comes before its format"))?;
            let end = body.saturating_add(size).min(bytes.len());
            let data = &bytes[body..end];
            let channels = channels.max(1) as usize;
            let mono: Vec<f32> = match (tag, bits) {
                // 0xFFFE is WAVE_FORMAT_EXTENSIBLE, which Windows uses for
                // plain integer PCM too.
                (1 | 0xFFFE, 16) => from_i16_le(data),
                (3, 32) => data
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect(),
                _ => return Err(bad(&format!("format {tag} at {bits} bits"))),
            };
            let samples = if channels == 1 {
                mono
            } else {
                mono.chunks(channels)
                    .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
                    .collect()
            };
            return Ok(Pcm { rate, samples });
        }
        // Chunks are padded to an even length.
        at = body.saturating_add(size).saturating_add(size & 1);
    }
    Err(bad("it has no audio in it"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A WAV file the way `espeak-ng --stdout` or `say` write one.
    pub(crate) fn wav(
        rate: u32,
        channels: u16,
        samples: &[i16],
        junk: bool,
        data_size: Option<u32>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        if junk {
            out.extend_from_slice(b"JUNK");
            out.extend_from_slice(&3u32.to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0]); // padded to even
        }
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2 * channels as u32).to_le_bytes());
        out.extend_from_slice(&(2 * channels).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        out.extend_from_slice(&data_size.unwrap_or(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&bytes);
        out
    }

    #[test]
    fn a_plain_file_reads() {
        let pcm = read(&wav(22_050, 1, &[0, 16384, -32768], false, None)).unwrap();
        assert_eq!(pcm.rate, 22_050);
        assert_eq!(pcm.samples, [0.0, 0.5, -1.0]);
    }

    #[test]
    fn chunks_before_the_format_are_walked_past() {
        let pcm = read(&wav(24_000, 1, &[100, 200], true, None)).unwrap();
        assert_eq!(pcm.samples.len(), 2);
    }

    #[test]
    fn a_placeholder_size_reads_to_the_end() {
        let pcm = read(&wav(22_050, 1, &[1, 2, 3, 4], false, Some(0x7fff_f000))).unwrap();
        assert_eq!(pcm.samples.len(), 4);
    }

    #[test]
    fn stereo_is_mixed_down() {
        let pcm = read(&wav(16_000, 2, &[16384, 0, -16384, -16384], false, None)).unwrap();
        assert_eq!(pcm.samples, [0.25, -0.5]);
    }

    #[test]
    fn something_else_is_refused_with_a_reason() {
        let err = read(b"ID3 not a wav at all").unwrap_err().to_string();
        assert!(err.contains("isn't RIFF/WAVE"), "{err}");
    }
}
