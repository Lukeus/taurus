//! Text embeddings computed in this process, so semantic search needs no model
//! server running.
//!
//! The index embeds through [`taurus_provider::Provider::embed`], which until
//! this crate meant an Ollama or OpenAI-compatible endpoint. This runs a small
//! encoder model directly: BERT for bge-small and arctic-embed, NomicBERT for
//! nomic-embed-text and CodeRankEmbed.
//!
//! # Why candle
//!
//! It is pure Rust. onnxruntime is faster on CPU, but it is a C++ library that
//! ships as a downloaded prebuilt, and that download has no build for Intel
//! macOS or musl Linux — the same wall the Kokoro voice spike hit. candle's
//! kernels are `gemm`; on macOS it links Apple's Accelerate, which is part of
//! the OS. The tokenizer is Hugging Face's, built with `fancy-regex` and
//! nothing else: its defaults pull Oniguruma (C) and a C++ suffix array.
//!
//! # What a caller must do
//!
//! Prefix the text the way the model was trained. Several of these models
//! expect a task prefix on queries, documents or both, and the index knows
//! which text is which; this crate does not. [`Spec::query_prefix`] and
//! [`Spec::document_prefix`] say what each model wants.

use std::path::Path;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::{bert, nomic_bert};

/// The network a model is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    Bert,
    NomicBert,
}

/// How one vector is made from a passage's token vectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pooling {
    /// The first token's vector. What bge and arctic-embed are trained for.
    Cls,
    /// The mean over real tokens, padding left out. What nomic is trained for.
    Mean,
}

/// A model this crate can run, and how.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// The name settings use.
    pub id: &'static str,
    /// Where it is downloaded from, on Hugging Face.
    pub repo: &'static str,
    /// The commit downloaded, so a model updated upstream can't change the
    /// vectors under an index built from it.
    pub revision: &'static str,
    pub arch: Arch,
    pub pooling: Pooling,
    pub dims: usize,
    /// Tokens past this are cut. The model's own limit, or less where
    /// attention's cost makes the rest not worth paying for.
    pub max_tokens: usize,
    pub query_prefix: &'static str,
    pub document_prefix: &'static str,
}

/// Every model this crate runs.
pub const MODELS: &[Spec] = &[
    Spec {
        id: "bge-small-en-v1.5",
        repo: "BAAI/bge-small-en-v1.5",
        revision: "main",
        arch: Arch::Bert,
        pooling: Pooling::Cls,
        dims: 384,
        max_tokens: 512,
        query_prefix: "Represent this sentence for searching relevant passages: ",
        document_prefix: "",
    },
    Spec {
        id: "snowflake-arctic-embed-s",
        repo: "Snowflake/snowflake-arctic-embed-s",
        revision: "main",
        arch: Arch::Bert,
        pooling: Pooling::Cls,
        dims: 384,
        max_tokens: 512,
        query_prefix: "Represent this sentence for searching relevant passages: ",
        document_prefix: "",
    },
    Spec {
        id: "nomic-embed-text-v1.5",
        repo: "nomic-ai/nomic-embed-text-v1.5",
        revision: "main",
        arch: Arch::NomicBert,
        pooling: Pooling::Mean,
        dims: 768,
        max_tokens: 2048,
        query_prefix: "search_query: ",
        document_prefix: "search_document: ",
    },
    Spec {
        id: "CodeRankEmbed",
        repo: "nomic-ai/CodeRankEmbed",
        revision: "main",
        arch: Arch::NomicBert,
        pooling: Pooling::Mean,
        dims: 768,
        max_tokens: 2048,
        query_prefix: "Represent this query for searching relevant code: ",
        document_prefix: "",
    },
];

/// The spec for `id`, if this crate runs it.
pub fn spec(id: &str) -> Option<&'static Spec> {
    MODELS.iter().find(|spec| spec.id == id)
}

enum Net {
    Bert(bert::BertModel),
    Nomic(nomic_bert::NomicBertModel),
}

/// A loaded model.
pub struct Embedder {
    net: Net,
    tokenizer: tokenizers::Tokenizer,
    spec: &'static Spec,
    device: Device,
}

/// How many passages go through the network at once. Inputs are sorted by
/// length first, so a batch pads to its own longest rather than the corpus's.
const BATCH: usize = 32;

impl Embedder {
    /// Loads `spec` from `dir`, which holds the model's `config.json`,
    /// `tokenizer.json` and `model.safetensors`.
    ///
    /// The weights are read into memory, not mapped: mapping is an `unsafe`
    /// call in candle, and this workspace forbids those in its own code.
    pub fn load(dir: &Path, spec: &'static Spec) -> Result<Embedder, String> {
        let device = Device::Cpu;
        let read = |name: &str| {
            std::fs::read(dir.join(name)).map_err(|e| format!("{}: {e}", dir.join(name).display()))
        };
        let config = String::from_utf8(read("config.json")?)
            .map_err(|e| format!("config.json is not text: {e}"))?;
        let weights = read("model.safetensors")?;
        let vb = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)
            .map_err(|e| format!("model.safetensors: {e}"))?;
        let net = match spec.arch {
            Arch::Bert => {
                let config: bert::Config =
                    serde_json::from_str(&config).map_err(|e| format!("config.json: {e}"))?;
                Net::Bert(bert::BertModel::load(vb, &config).map_err(|e| e.to_string())?)
            }
            Arch::NomicBert => {
                let config: nomic_bert::Config =
                    serde_json::from_str(&config).map_err(|e| format!("config.json: {e}"))?;
                Net::Nomic(
                    nomic_bert::NomicBertModel::load(vb, &config).map_err(|e| e.to_string())?,
                )
            }
        };

        let mut tokenizer = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| format!("tokenizer.json: {e}"))?;
        tokenizer.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: spec.max_tokens,
                ..Default::default()
            }))
            .map_err(|e| format!("tokenizer.json: {e}"))?;

        Ok(Embedder {
            net,
            tokenizer,
            spec,
            device,
        })
    }

    pub fn spec(&self) -> &'static Spec {
        self.spec
    }

    /// One unit-length vector per input, in input order. Inputs are embedded
    /// as given: prefixing is the caller's. See the crate header.
    pub fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let encodings = self
            .tokenizer
            .encode_batch(inputs.to_vec(), true)
            .map_err(|e| e.to_string())?;
        // Shortest first, so each batch pads to a length near its own.
        let mut order: Vec<usize> = (0..inputs.len()).collect();
        order.sort_by_key(|&i| {
            encodings[i]
                .get_attention_mask()
                .iter()
                .filter(|&&m| m == 1)
                .count()
        });

        let mut out: Vec<Vec<f32>> = vec![Vec::new(); inputs.len()];
        for batch in order.chunks(BATCH) {
            let texts: Vec<String> = batch.iter().map(|&i| inputs[i].clone()).collect();
            let vectors = self.batch(texts).map_err(|e| e.to_string())?;
            for (&i, vector) in batch.iter().zip(vectors) {
                out[i] = vector;
            }
        }
        Ok(out)
    }

    fn batch(&self, texts: Vec<String>) -> candle_core::Result<Vec<Vec<f32>>> {
        let encodings = self
            .tokenizer
            .encode_batch(texts, true)
            .map_err(candle_core::Error::wrap)?;
        let rows = encodings.len();
        let width = encodings.first().map_or(0, |e| e.get_ids().len());
        let ids: Vec<u32> = encodings
            .iter()
            .flat_map(|e| e.get_ids().to_vec())
            .collect();
        let mask: Vec<u32> = encodings
            .iter()
            .flat_map(|e| e.get_attention_mask().to_vec())
            .collect();
        let ids = Tensor::from_vec(ids, (rows, width), &self.device)?;
        let mask = Tensor::from_vec(mask, (rows, width), &self.device)?;
        let types = ids.zeros_like()?;

        let hidden = match &self.net {
            Net::Bert(net) => net.forward(&ids, &types, Some(&mask))?,
            Net::Nomic(net) => net.forward(&ids, Some(&types), Some(&mask))?,
        };
        let pooled = match self.spec.pooling {
            Pooling::Cls => hidden.get_on_dim(1, 0)?,
            Pooling::Mean => nomic_bert::mean_pooling(&hidden, &mask)?,
        };
        nomic_bert::l2_normalize(&pooled)?.to_vec2::<f32>()
    }
}
