//! The languages a multilingual model can be told to expect.
//!
//! By default the multilingual models work out which language they're hearing
//! before they transcribe it. That's a pass of its own, and an expensive one:
//! timed on an M4 Max, it cost large-v3-turbo about 350 ms on the GPU and 4–5 s
//! on the CPU, on every dictation. Somebody who only ever speaks one language
//! can name it and skip that pass.
//!
//! The list is whisper.cpp's own, read from the library, so it can't drift from
//! what the model accepts. The `.en` models ignore all of this: they only hear
//! English.

use whisper_rs::{get_lang_id, get_lang_max_id, get_lang_str, get_lang_str_full};

use crate::VoiceError;

/// The setting's value for "work it out".
pub const AUTO: &str = "auto";

/// One language, as the picker shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Language {
    /// What settings store, and what whisper.cpp is given: `en`, `de`, `ja`.
    pub code: &'static str,
    /// What the picker shows: `English`, `German`, `Japanese`.
    pub name: String,
}

/// Every language whisper.cpp knows, by name.
pub fn languages() -> Vec<Language> {
    let mut all: Vec<Language> = (0..=get_lang_max_id())
        .filter_map(|id| {
            Some(Language {
                code: get_lang_str(id)?,
                name: title(get_lang_str_full(id)?),
            })
        })
        .collect();
    all.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

/// The code whisper.cpp is given for a setting's value: [`AUTO`] for an empty
/// one, and otherwise the language's code, whether it was written as a code or
/// a name, in any case.
pub fn language(value: &str) -> Result<&'static str, VoiceError> {
    let value = value.trim().to_lowercase();
    if value.is_empty() || value == AUTO {
        return Ok(AUTO);
    }
    get_lang_id(&value)
        .and_then(get_lang_str)
        .ok_or(VoiceError::UnknownLanguage(value))
}

/// whisper.cpp's names are lower case: "english", "haitian creole".
fn title(name: &str) -> String {
    name.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        })
        .collect::<Vec<String>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_auto_mean_detect() {
        assert_eq!(language("").unwrap(), AUTO);
        assert_eq!(language("  ").unwrap(), AUTO);
        assert_eq!(language("Auto").unwrap(), AUTO);
    }

    #[test]
    fn codes_and_names_both_come_back_as_the_code() {
        assert_eq!(language("en").unwrap(), "en");
        assert_eq!(language(" DE ").unwrap(), "de");
        assert_eq!(language("Japanese").unwrap(), "ja");
    }

    #[test]
    fn an_unknown_language_is_refused_with_the_way_out() {
        let err = language("klingon").unwrap_err().to_string();
        assert!(err.contains("\"klingon\""), "{err}");
        assert!(err.contains("auto"), "{err}");
    }

    #[test]
    fn the_list_is_whisper_cpps_sorted_by_name() {
        let all = languages();
        assert!(all.len() > 90, "{}", all.len());
        assert!(all.windows(2).all(|w| w[0].name <= w[1].name));
        let english = all.iter().find(|l| l.code == "en").unwrap();
        assert_eq!(english.name, "English");
        // Every code in the list is one `language` accepts as itself.
        for l in &all {
            assert_eq!(language(l.code).unwrap(), l.code);
        }
    }

    #[test]
    fn multi_word_names_are_titled_word_by_word() {
        assert_eq!(title("haitian creole"), "Haitian Creole");
        assert_eq!(title("english"), "English");
    }
}
