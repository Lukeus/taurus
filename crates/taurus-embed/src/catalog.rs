//! The models this crate runs, each pinned to the files that were measured.
//!
//! A short fixed list rather than any repository somebody names. Each entry is
//! pinned to one commit of its Hugging Face repository and to the size and
//! SHA-256 of every file used, so what lands on disk is exactly what
//! `examples/retrieval` scored — a moved branch or a tampered mirror fails the
//! checksum instead of being loaded.
//!
//! Two are offered, both NomicBERT with mean pooling. bge-small-en-v1.5 and
//! snowflake-arctic-embed-s were measured too (2026-11-07, this repository's
//! gate) and dropped: a quarter the download, and no better than the Ollama
//! model search already used (MRR 0.43 and 0.39 against 0.43).

/// One file of a model, as pinned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct File {
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

/// A model this crate can download and run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The name settings use. Also the model name the index is keyed on and
    /// looks prefixes up by, so it must stay what `taurus_index::prefixes`
    /// knows it as.
    pub id: &'static str,
    /// Where it is downloaded from, on Hugging Face.
    pub repo: &'static str,
    /// The commit every file is fetched from, so a model updated upstream
    /// can't change the vectors under an index built from it.
    pub revision: &'static str,
    pub files: [File; 3],
    pub dims: usize,
    /// Tokens past this are cut. The index's chunks are far shorter, so this
    /// only bounds a pathological input.
    pub max_tokens: usize,
    pub license: &'static str,
    /// One line for the picker: what you'd choose it for.
    pub summary: &'static str,
}

impl Spec {
    /// Every file's size together, for a progress bar.
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    pub fn url(&self, file: &File) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, file.name
        )
    }
}

/// The model search downloads when nothing else is chosen.
///
/// CodeRankEmbed is trained on code search, which is this index's whole job,
/// and it showed: on this repository's gate it ranked the right file first for
/// 10 of 15 questions, against 5 for nomic-embed-text with its prefixes and 3
/// for the Ollama setup search shipped with (MRR 0.75, 0.54, 0.43).
pub const DEFAULT_MODEL: &str = "CodeRankEmbed";

pub const MODELS: &[Spec] = &[
    Spec {
        id: "CodeRankEmbed",
        repo: "nomic-ai/CodeRankEmbed",
        revision: "3c4b60807d71f79b43f3c4363786d9493691f8b1",
        files: [
            File {
                name: "config.json",
                bytes: 1_525,
                sha256: "5ff856a41d0f53ef2d74520627d464bd75c2efd8f26f381bd528654895c29b6c",
            },
            File {
                name: "tokenizer.json",
                bytes: 711_649,
                sha256: "91f1def9b9391fdabe028cd3f3fcc4efd34e5d1f08c3bf2de513ebb5911a1854",
            },
            File {
                name: "model.safetensors",
                bytes: 546_938_168,
                sha256: "827529bcd58aef0d9082e66eeff7e7d53a02f62bd005f841a26b3d3e2fb17ebe",
            },
        ],
        dims: 768,
        max_tokens: 2048,
        license: "MIT",
        summary: "The default. Trained to find code from a question.",
    },
    Spec {
        id: "nomic-embed-text-v1.5",
        repo: "nomic-ai/nomic-embed-text-v1.5",
        revision: "e9b6763023c676ca8431644204f50c2b100d9aab",
        files: [
            File {
                name: "config.json",
                bytes: 2_538,
                sha256: "9ab00bd92cee80a569f708140b7b6c1661a65891ff3765b1519e181ba2f2c92b",
            },
            File {
                name: "tokenizer.json",
                bytes: 711_396,
                sha256: "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
            },
            File {
                name: "model.safetensors",
                bytes: 546_938_168,
                sha256: "9e7d262b1fe5ea350782829496efa831901b77486bbde1cea54a4c822d010d5c",
            },
        ],
        dims: 768,
        max_tokens: 2048,
        license: "Apache-2.0",
        summary: "General text. The same vectors as Ollama's nomic-embed-text.",
    },
];

/// The spec for `id`, if this crate runs it.
pub fn spec(id: &str) -> Option<&'static Spec> {
    MODELS.iter().find(|spec| spec.id == id.trim())
}

/// The known names, for an error that has to list them.
pub fn known_ids() -> String {
    MODELS.iter().map(|m| m.id).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_in_the_catalog() {
        assert!(spec(DEFAULT_MODEL).is_some());
    }

    #[test]
    fn every_file_is_pinned_to_a_commit_and_a_checksum() {
        for m in MODELS {
            assert_eq!(m.revision.len(), 40, "{}", m.id);
            for f in &m.files {
                assert_eq!(f.sha256.len(), 64, "{} {}", m.id, f.name);
                assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit()));
                assert!(m.url(f).contains(m.revision));
                assert!(m.url(f).ends_with(f.name));
            }
        }
    }

    #[test]
    fn ids_are_trimmed() {
        assert_eq!(spec(" CodeRankEmbed ").unwrap().id, "CodeRankEmbed");
        assert!(spec("bge-small-en-v1.5").is_none());
    }
}
