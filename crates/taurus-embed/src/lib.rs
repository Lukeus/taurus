//! Text embeddings computed in this process, so semantic search needs no model
//! server running.
//!
//! The index embeds through [`taurus_provider::Provider::embed`], which until
//! this crate meant an Ollama or OpenAI-compatible endpoint. [`Local`] is a
//! provider that runs a small NomicBERT encoder here instead, from files
//! downloaded once (see [`catalog`] and [`models`]).
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
//! # What it costs
//!
//! It runs on the CPU. Embedding this repository (17,275 passages) took
//! about 29 minutes on an M4 Max, against 9 for Ollama on the GPU with the
//! same network — paid once per workspace, after which only changed files are
//! embedded. A query is one short string. The loaded model holds about 550 MB,
//! so [`Local`] lets it go after a few idle minutes and reads it again when
//! asked.
//!
//! # What a caller must do
//!
//! Prefix the text the way the model was trained: `taurus_index::prefixes`
//! knows how. This crate embeds what it is given.

pub mod catalog;
mod local;
pub mod models;

use std::path::Path;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::nomic_bert;

pub use catalog::{spec, Spec, DEFAULT_MODEL, MODELS};
pub use local::{Local, PROVIDER_ID};
pub use models::Models;

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error(
        "\"{name}\" isn't a model Taurus can run on this computer. The ones it can are: {}",
        catalog::known_ids()
    )]
    UnknownModel { name: String },
    #[error("{0} isn't downloaded yet. Download it under Settings → Search.")]
    ModelMissing(String),
    #[error("couldn't load {path}: {reason}. Delete the model under Settings → Search and download it again.")]
    Load { path: String, reason: String },
    #[error("couldn't download {file} from {url}: {reason}")]
    Download {
        file: String,
        url: String,
        reason: String,
    },
    #[error("the download was canceled")]
    Canceled,
    #[error("{0}")]
    Io(String),
    #[error("embedding failed: {0}")]
    Run(String),
}

/// A loaded model.
pub struct Embedder {
    net: nomic_bert::NomicBertModel,
    tokenizer: tokenizers::Tokenizer,
    spec: &'static Spec,
    device: Device,
}

/// How many passages go through the network at once. Inputs are sorted by
/// length first, so a batch pads to its own longest rather than the corpus's.
const BATCH: usize = 32;

impl Embedder {
    /// Loads `spec` from `dir`, which holds the files its catalog entry
    /// names.
    ///
    /// The weights are read into memory, not mapped: mapping is an `unsafe`
    /// call in candle, and this workspace forbids those in its own code.
    pub fn load(dir: &Path, spec: &'static Spec) -> Result<Embedder, EmbedError> {
        let device = Device::Cpu;
        let fail = |name: &str, reason: String| EmbedError::Load {
            path: dir.join(name).display().to_string(),
            reason,
        };
        let read =
            |name: &str| std::fs::read(dir.join(name)).map_err(|e| fail(name, e.to_string()));
        let config = String::from_utf8(read("config.json")?)
            .map_err(|e| fail("config.json", format!("it isn't text: {e}")))?;
        let config: nomic_bert::Config =
            serde_json::from_str(&config).map_err(|e| fail("config.json", e.to_string()))?;
        let weights = read("model.safetensors")?;
        let vb = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)
            .map_err(|e| fail("model.safetensors", e.to_string()))?;
        let net = nomic_bert::NomicBertModel::load(vb, &config)
            .map_err(|e| fail("model.safetensors", e.to_string()))?;

        let mut tokenizer = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| fail("tokenizer.json", e.to_string()))?;
        tokenizer.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: spec.max_tokens,
                ..Default::default()
            }))
            .map_err(|e| fail("tokenizer.json", e.to_string()))?;

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
    pub fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let run = |e: String| EmbedError::Run(e);
        let encodings = self
            .tokenizer
            .encode_batch(inputs.to_vec(), true)
            .map_err(|e| run(e.to_string()))?;
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
            let vectors = self.batch(texts).map_err(|e| run(e.to_string()))?;
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

        let hidden = self.net.forward(&ids, Some(&types), Some(&mask))?;
        let pooled = nomic_bert::mean_pooling(&hidden, &mask)?;
        nomic_bert::l2_normalize(&pooled)?.to_vec2::<f32>()
    }
}
