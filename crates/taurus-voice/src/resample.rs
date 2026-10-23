//! Bringing a microphone's sample rate down to the 16 kHz whisper.cpp reads.
//!
//! Microphones run at 44.1 or 48 kHz, and whisper wants exactly 16. Each
//! output sample is the average of the input samples it covers, which is a box
//! filter: crude next to a proper polyphase resampler, and plenty for speech,
//! whose content sits well under the 8 kHz this keeps. Without the averaging,
//! picking every third sample folds the top of the spectrum back down as
//! noise; with it, the measured transcripts came back word for word.

/// The rate whisper.cpp expects.
pub const WHISPER_RATE: u32 = 16_000;

/// `input` at `from` Hz, resampled to [`WHISPER_RATE`].
pub fn to_whisper_rate(input: &[f32], from: u32) -> Vec<f32> {
    if from == WHISPER_RATE || input.is_empty() || from == 0 {
        return input.to_vec();
    }
    let ratio = from as f64 / WHISPER_RATE as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let center = (i as f64 + 0.5) * ratio;
        if ratio > 1.0 {
            // Downsampling: average everything this output sample stands for.
            let start = ((center - ratio / 2.0).floor().max(0.0)) as usize;
            let end = ((center + ratio / 2.0).ceil() as usize).min(input.len());
            let span = &input[start..end.max(start + 1).min(input.len())];
            out.push(span.iter().sum::<f32>() / span.len() as f32);
        } else {
            // Upsampling, from an 8 kHz headset: interpolate between neighbors.
            let pos = center - 0.5;
            let base = pos.floor().max(0.0) as usize;
            let next = (base + 1).min(input.len() - 1);
            let frac = (pos - base as f64).clamp(0.0, 1.0) as f32;
            out.push(input[base] * (1.0 - frac) + input[next] * frac);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, hz: f32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|n| (2.0 * std::f32::consts::PI * hz * n as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn length_follows_the_ratio() {
        assert_eq!(to_whisper_rate(&vec![0.0; 48_000], 48_000).len(), 16_000);
        assert_eq!(to_whisper_rate(&vec![0.0; 44_100], 44_100).len(), 16_000);
        assert_eq!(to_whisper_rate(&vec![0.0; 8_000], 8_000).len(), 16_000);
        assert_eq!(
            to_whisper_rate(&vec![0.5; 16_000], 16_000),
            vec![0.5; 16_000]
        );
    }

    #[test]
    fn speech_band_survives() {
        // A 440 Hz tone keeps nearly all of its energy through the filter.
        let out = to_whisper_rate(&tone(48_000, 440.0, 1.0), 48_000);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(
            (rms - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02,
            "{rms}"
        );
    }

    #[test]
    fn content_above_the_new_nyquist_is_damped() {
        // 15 kHz can't exist at 16 kHz. Without the averaging it aliases to
        // 1 kHz at full strength; with it, most of it is gone.
        let out = to_whisper_rate(&tone(48_000, 15_000.0, 1.0), 48_000);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms < 0.35, "{rms}");
    }

    #[test]
    fn empty_and_zero_rate_are_harmless() {
        assert!(to_whisper_rate(&[], 48_000).is_empty());
        assert_eq!(to_whisper_rate(&[1.0], 0), vec![1.0]);
    }
}
