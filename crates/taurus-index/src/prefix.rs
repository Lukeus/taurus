//! The task prefixes an embedding model was trained with.
//!
//! Several retrieval models are trained on queries and documents that start
//! with a fixed phrase, and they rank noticeably worse without it. nomic-embed-
//! text, for one, wants `search_query: ` before a question and
//! `search_document: ` before a passage. Measured on this repository's gate
//! (`examples/retrieval`), adding them took the same Ollama model from an MRR
//! of 0.43 to 0.54, without changing anything else.
//!
//! The index is the one place that knows which text is a query and which is a
//! passage, so it adds them, whichever backend serves the model. Each entry
//! here is from the model's own card. A model that isn't listed gets no
//! prefix, which is what every model got before this existed.

/// What to put before each kind of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefixes {
    pub query: &'static str,
    pub document: &'static str,
}

const NONE: Prefixes = Prefixes {
    query: "",
    document: "",
};

/// The instruction BAAI's English v1.5 models, and mxbai-embed-large after
/// them, put before a query. Passages go in as they are.
const SENTENCE_QUERY: Prefixes = Prefixes {
    query: "Represent this sentence for searching relevant passages: ",
    document: "",
};

/// The prefixes for `model`, as a settings field or an Ollama tag names it.
///
/// Matched on the name with any namespace (`nomic-ai/`) and Ollama tag
/// (`:latest`, `:v1.5`) taken off, ignoring case, so the same model reached
/// through two backends gets the same prefixes.
pub fn prefixes(model: &str) -> Prefixes {
    let name = model.trim().to_ascii_lowercase();
    let name = name.rsplit('/').next().unwrap_or(&name);
    let name = name.split(':').next().unwrap_or(name);
    if name.starts_with("nomic-embed-text") {
        Prefixes {
            query: "search_query: ",
            document: "search_document: ",
        }
    } else if name == "coderankembed" {
        Prefixes {
            query: "Represent this query for searching relevant code: ",
            document: "",
        }
    } else if name == "mxbai-embed-large"
        || name == "mxbai-embed-large-v1"
        || (name.starts_with("bge-") && name.ends_with("-en-v1.5"))
    {
        SENTENCE_QUERY
    } else {
        NONE
    }
}

/// The name an index built by `model` is kept under.
///
/// Vectors made with a prefix are not comparable with vectors made without
/// one, so an index built before prefixes existed must not be searched with a
/// prefixed query. Keying it differently makes the old one read as another
/// model's, and the next refresh builds it again, once.
pub(crate) fn key(model: &str) -> String {
    if prefixes(model) == NONE {
        model.to_string()
    } else {
        format!("{model} (prefixed)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nomic_gets_its_prefixes_through_any_backend() {
        for name in [
            "nomic-embed-text",
            "nomic-embed-text:latest",
            "nomic-embed-text:v1.5",
            "nomic-embed-text-v1.5",
            "nomic-ai/nomic-embed-text-v1.5",
            " Nomic-Embed-Text ",
        ] {
            let p = prefixes(name);
            assert_eq!(p.query, "search_query: ", "{name}");
            assert_eq!(p.document, "search_document: ", "{name}");
        }
    }

    #[test]
    fn coderank_prefixes_only_the_query() {
        let p = prefixes("CodeRankEmbed");
        assert_eq!(
            p.query,
            "Represent this query for searching relevant code: "
        );
        assert_eq!(p.document, "");
    }

    #[test]
    fn the_bge_instruction_is_for_english_v1_5_only() {
        assert_eq!(prefixes("bge-small-en-v1.5"), SENTENCE_QUERY);
        assert_eq!(prefixes("BAAI/bge-large-en-v1.5"), SENTENCE_QUERY);
        assert_eq!(prefixes("mxbai-embed-large"), SENTENCE_QUERY);
        // bge-m3 is trained without one.
        assert_eq!(prefixes("bge-m3"), NONE);
    }

    #[test]
    fn an_unknown_model_gets_nothing_and_keeps_its_old_index() {
        assert_eq!(prefixes("all-minilm"), NONE);
        assert_eq!(key("all-minilm"), "all-minilm");
    }

    #[test]
    fn a_prefixed_model_is_kept_under_a_new_name() {
        assert_ne!(key("nomic-embed-text"), "nomic-embed-text");
    }
}
