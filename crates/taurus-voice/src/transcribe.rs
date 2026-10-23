//! whisper.cpp over a recording, and tidying what it says.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::capture::Captured;
use crate::models::ModelSpec;
use crate::VoiceError;

/// Less speech than this and the recording isn't transcribed at all.
///
/// A quarter of a second is shorter than any word said on purpose and longer
/// than a click, a breath or a key being pressed near the microphone.
const MIN_VOICED_SECONDS: f32 = 0.25;

/// How much of the hint whisper.cpp is given, in characters.
///
/// Its prompt is capped at 224 tokens, and a hint that fills it crowds out the
/// model's own context from the audio. A few dozen names is enough to fix the
/// spelling of a project's words.
const HINT_CHARS: usize = 300;

/// The loaded model, kept between dictations.
///
/// Loading is fast once the file is in the page cache (about 60 ms for
/// base.en on an M-series Mac), but the first load of the day reads the whole
/// file, and a GPU backend compiles its kernels on load. Neither is worth
/// paying on every press.
#[derive(Default)]
pub struct Engine {
    loaded: Mutex<Option<(PathBuf, Arc<WhisperContext>)>>,
}

impl Engine {
    fn context(&self, path: &Path) -> Result<Arc<WhisperContext>, VoiceError> {
        static LOGGING: Once = Once::new();
        // whisper.cpp prints its model and backend details to stderr on every
        // load. Routed into `tracing`, where they're kept at debug level.
        LOGGING.call_once(whisper_rs::install_logging_hooks);

        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((held, context)) = loaded.as_ref() {
            if held == path {
                return Ok(Arc::clone(context));
            }
        }
        // Dropped first, so switching models never holds two in memory.
        *loaded = None;
        let path_str = path.to_str().ok_or_else(|| VoiceError::Load {
            path: path.display().to_string(),
            reason: "its path isn't valid UTF-8".into(),
        })?;
        let context =
            WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
                .map_err(|e| VoiceError::Load {
                    path: path.display().to_string(),
                    reason: e.to_string(),
                })?;
        let context = Arc::new(context);
        *loaded = Some((path.to_path_buf(), Arc::clone(&context)));
        Ok(context)
    }

    /// Transcribes `audio` with the model at `path`.
    pub fn transcribe(
        &self,
        spec: &ModelSpec,
        path: &Path,
        audio: &Captured,
        hint: &str,
        abort: Arc<AtomicBool>,
    ) -> Result<String, VoiceError> {
        if audio.peak == 0.0 {
            return Err(VoiceError::NoSignal);
        }
        if audio.voiced_seconds() < MIN_VOICED_SECONDS {
            return Err(VoiceError::NothingHeard);
        }

        let context = self.context(path)?;
        let mut state = context
            .create_state()
            .map_err(|e| VoiceError::Transcribe(e.to_string()))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(if spec.english_only { "en" } else { "auto" }));
        params.set_n_threads(threads());
        // Each dictation stands alone. Carrying text over between windows is
        // how a long recording starts repeating itself.
        params.set_no_context(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        if !hint.is_empty() {
            params.set_initial_prompt(hint);
        }
        // Checked by whisper.cpp between steps, so a long recording on a slow
        // machine can be given up on without waiting it out.
        //
        // Boxed here on purpose. whisper-rs (0.15 and 0.16) stores the closure
        // as a `Box<dyn FnMut() -> bool>` but its trampoline reads it back as
        // the closure's own type, so an unboxed closure is called through the
        // wrong layout and returns noise. whisper.cpp takes that as "abort"
        // and fails the encode with -6. When the closure *is* that box, the
        // two types are the same and the call is sound.
        let stopped = Arc::clone(&abort);
        let check: Box<dyn FnMut() -> bool> = Box::new(move || abort.load(Ordering::Relaxed));
        params.set_abort_callback_safe(check);

        if let Err(e) = state.full(params, &audio.samples) {
            // An abort surfaces as an ordinary failure code. The flag says
            // which it was.
            if stopped.load(Ordering::Relaxed) {
                return Err(VoiceError::Canceled);
            }
            return Err(VoiceError::Transcribe(e.to_string()));
        }

        let mut text = String::new();
        for segment in state.as_iter() {
            if let Ok(piece) = segment.to_str_lossy() {
                text.push_str(&piece);
            }
        }
        let text = clean(&text);
        if text.is_empty() {
            return Err(VoiceError::NothingHeard);
        }
        Ok(text)
    }
}

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8) as i32)
        .unwrap_or(4)
}

/// Tidies whisper's text for the composer.
///
/// Drops what whisper.cpp writes for sounds rather than words — `[BLANK_AUDIO]`,
/// `[MUSIC]`, `(keyboard clicking)` — and squeezes the spacing left behind.
/// Parenthesized text is dropped only when it's all a segment is, because
/// "(see the README)" said aloud is still something somebody said.
pub fn clean(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut depth = 0usize;
    for c in raw.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let trimmed = out.trim();
    let annotation = trimmed.starts_with('(')
        && trimmed.ends_with(')')
        && !trimmed[1..trimmed.len() - 1].contains(['(', ')']);
    if annotation || trimmed.starts_with('*') && trimmed.ends_with('*') && trimmed.len() > 1 {
        return String::new();
    }
    trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The words to prime whisper.cpp with: what this project calls things.
///
/// Whisper spells unfamiliar words the way they sound, so "taurus core" came
/// back as "TorisCore" in the measured run. Given the workspace's own names up
/// front, it reaches for those spellings instead. The folder name first, then
/// what's at its top level, which is where a project's vocabulary is densest:
/// crate and package names, the docs, the scripts.
pub fn hint_for(workspace: &str, names: impl IntoIterator<Item = String>) -> String {
    let mut hint = format!("Taurus, {workspace}");
    for name in names {
        if name.starts_with('.') || name.is_empty() {
            continue;
        }
        if hint.len() + name.len() + 2 > HINT_CHARS {
            break;
        }
        hint.push_str(", ");
        hint.push_str(&name);
    }
    hint.push('.');
    hint
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_annotations_are_dropped() {
        assert_eq!(clean(" [BLANK_AUDIO]"), "");
        assert_eq!(clean(" (keyboard clicking) "), "");
        assert_eq!(clean("*sigh*"), "");
        assert_eq!(clean(" Run the tests [MUSIC] now. "), "Run the tests now.");
    }

    #[test]
    fn spoken_parentheses_stay() {
        assert_eq!(
            clean(" Open it (the README) please."),
            "Open it (the README) please."
        );
    }

    #[test]
    fn whitespace_is_squeezed() {
        assert_eq!(clean("  one   two\nthree "), "one two three");
    }

    #[test]
    fn hint_leads_with_the_project_and_skips_hidden_names() {
        let hint = hint_for(
            "taurus-ai-shell",
            ["crates", ".git", "docs", "README.md"].map(String::from),
        );
        assert_eq!(hint, "Taurus, taurus-ai-shell, crates, docs, README.md.");
    }

    #[test]
    fn hint_is_bounded() {
        let names = (0..500).map(|n| format!("name{n}"));
        let hint = hint_for("w", names);
        assert!(hint.len() <= HINT_CHARS + 1, "{}", hint.len());
    }
}
