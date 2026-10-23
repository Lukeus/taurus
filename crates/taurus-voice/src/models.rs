//! The speech models Taurus can download, and the files on disk.
//!
//! A short fixed list rather than any URL somebody types. Each entry is pinned
//! to one revision of the whisper.cpp model repository and to the size and
//! SHA-256 of its file, so what lands on disk is exactly the file that was
//! measured — a moved tag or a tampered mirror fails the checksum instead of
//! being loaded into a C++ parser.
//!
//! All four are quantized (`q5_1`): about a third the size of the full-precision
//! files, at an accuracy difference too small to notice in dictation.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::VoiceError;

/// One model Taurus knows how to fetch and run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelSpec {
    /// What settings and the UI call it.
    pub id: &'static str,
    /// The file name, both upstream and on disk.
    pub file: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    /// English-only models are smaller and more accurate in English, and
    /// can't transcribe anything else. The multilingual one detects the
    /// language as it goes.
    pub english_only: bool,
    /// One line for the picker: what you'd choose it for.
    pub summary: &'static str,
}

/// The model the mic downloads when nothing else is chosen.
///
/// base.en is the measured middle: 60 MB, and a six-second sentence
/// transcribed in under a tenth of a second on an M-series Mac. tiny.en
/// misses more words than dictation can afford, and small.en is three times
/// the download for a difference most people won't hear.
pub const DEFAULT_MODEL: &str = "base.en";

/// The revision of `ggerganov/whisper.cpp` every file is fetched from.
const REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "tiny.en",
        file: "ggml-tiny.en-q5_1.bin",
        bytes: 32_166_155,
        sha256: "c77c5766f1cef09b6b7d47f21b546cbddd4157886b3b5d6d4f709e91e66c7c2b",
        english_only: true,
        summary: "Smallest and fastest. Misses more words.",
    },
    ModelSpec {
        id: "base.en",
        file: "ggml-base.en-q5_1.bin",
        bytes: 59_721_011,
        sha256: "4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f",
        english_only: true,
        summary: "The default. Fast and accurate for English.",
    },
    ModelSpec {
        id: "small.en",
        file: "ggml-small.en-q5_1.bin",
        bytes: 190_098_681,
        sha256: "bfdff4894dcb76bbf647d56263ea2a96645423f1669176f4844a1bf8e478ad30",
        english_only: true,
        summary: "More accurate English. Slower without a GPU.",
    },
    ModelSpec {
        id: "base",
        file: "ggml-base-q5_1.bin",
        bytes: 59_707_625,
        sha256: "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
        english_only: false,
        summary: "Other languages. Detects which one you're speaking.",
    },
];

/// Looks a model up by the name settings use.
pub fn spec(id: &str) -> Result<&'static ModelSpec, VoiceError> {
    MODELS
        .iter()
        .find(|m| m.id == id.trim())
        .ok_or_else(|| VoiceError::UnknownModel(id.trim().to_string()))
}

/// The known names, for an error that has to list them.
pub(crate) fn known_ids() -> String {
    MODELS.iter().map(|m| m.id).collect::<Vec<_>>().join(", ")
}

impl ModelSpec {
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/{REVISION}/{}",
            self.file
        )
    }
}

/// The directory models live in, and what's in it.
pub struct Models {
    dir: PathBuf,
}

impl Models {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, spec: &ModelSpec) -> PathBuf {
        self.dir.join(spec.file)
    }

    /// Whether the model is on disk and the size it should be.
    ///
    /// Size, not checksum: hashing 60 MB every time the Settings screen opens
    /// would be noticeable, and the download already checked it. A file of the
    /// right size that's been altered since is somebody's deliberate act, and
    /// loading it fails with a message that says to download it again.
    pub fn installed(&self, spec: &ModelSpec) -> bool {
        std::fs::metadata(self.path(spec)).is_ok_and(|m| m.len() == spec.bytes)
    }

    /// The model's path, or why it can't be used.
    pub fn ready(&self, spec: &ModelSpec) -> Result<PathBuf, VoiceError> {
        let path = self.path(spec);
        match std::fs::metadata(&path) {
            Ok(m) if m.len() == spec.bytes => Ok(path),
            // A file of the wrong size is a download somebody finished by hand
            // and got wrong, or a truncated copy. Naming it is more use than
            // calling it missing.
            Ok(m) => Err(VoiceError::Load {
                path: path.display().to_string(),
                reason: format!("it's {} bytes, and should be {}", m.len(), spec.bytes),
            }),
            Err(_) => Err(VoiceError::ModelMissing(spec.id.to_string())),
        }
    }

    /// Deletes the model's file. Not an error if it isn't there.
    pub fn delete(&self, spec: &ModelSpec) -> Result<(), VoiceError> {
        match std::fs::remove_file(self.path(spec)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Fetches the model, checks it, and puts it in place.
    ///
    /// Written to a `.part` file beside the destination and renamed only once
    /// the size and checksum match, so a canceled or failed download never
    /// leaves something [`Models::installed`] would mistake for a model.
    ///
    /// `progress` gets `(done, total)` in bytes, at most a couple of hundred
    /// times over the whole download.
    pub async fn download(
        &self,
        spec: &ModelSpec,
        cancel: &CancellationToken,
        progress: impl Fn(u64, u64),
    ) -> Result<(), VoiceError> {
        let url = spec.url();
        let dest = self.path(spec);
        let part = dest.with_extension("bin.part");
        let fail = |reason: String| VoiceError::Download {
            file: spec.file.to_string(),
            url: url.clone(),
            path: dest.display().to_string(),
            reason,
        };

        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|e| fail(format!("couldn't create {}: {e}", self.dir.display())))?;

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            // Between reads, not for the whole body: a slow connection that
            // keeps delivering is fine, one that stops for this long isn't.
            .read_timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| fail(e.to_string()))?;
        let response = tokio::select! {
            r = client.get(&url).send() => r.map_err(|e| fail(describe(&e)))?,
            () = cancel.cancelled() => return Err(VoiceError::Canceled),
        };
        if !response.status().is_success() {
            return Err(fail(format!("the server answered {}", response.status())));
        }

        let result = async {
            let mut file = tokio::fs::File::create(&part)
                .await
                .map_err(|e| fail(format!("couldn't write {}: {e}", part.display())))?;
            let mut hasher = Sha256::new();
            let mut done = 0u64;
            let mut reported = 0u64;
            let step = (spec.bytes / 200).max(1);
            let mut body = response.bytes_stream();
            progress(0, spec.bytes);
            loop {
                let chunk = tokio::select! {
                    c = body.next() => c,
                    () = cancel.cancelled() => return Err(VoiceError::Canceled),
                };
                let Some(chunk) = chunk else { break };
                let chunk = chunk.map_err(|e| fail(describe(&e)))?;
                done += chunk.len() as u64;
                // Checked as it arrives, so a server sending far more than the
                // pinned size is stopped rather than allowed to fill the disk.
                if done > spec.bytes {
                    return Err(fail(format!(
                        "the server sent more than the expected {} bytes",
                        spec.bytes
                    )));
                }
                hasher.update(&chunk);
                file.write_all(&chunk)
                    .await
                    .map_err(|e| fail(format!("couldn't write {}: {e}", part.display())))?;
                if done - reported >= step {
                    reported = done;
                    progress(done, spec.bytes);
                }
            }
            file.flush()
                .await
                .map_err(|e| fail(format!("couldn't write {}: {e}", part.display())))?;
            drop(file);

            if done != spec.bytes {
                return Err(fail(format!(
                    "it stopped after {done} of {} bytes",
                    spec.bytes
                )));
            }
            let digest = to_hex(&hasher.finalize());
            if digest != spec.sha256 {
                return Err(fail(format!(
                    "the file's checksum is {digest}, not the expected {}",
                    spec.sha256
                )));
            }
            tokio::fs::rename(&part, &dest)
                .await
                .map_err(|e| fail(format!("couldn't move it into place: {e}")))?;
            progress(done, spec.bytes);
            Ok(())
        }
        .await;

        if result.is_err() {
            let _ = tokio::fs::remove_file(&part).await;
        }
        result
    }
}

/// A reqwest error as one readable clause.
///
/// Its `Display` is "error sending request for url (…)", which names the URL
/// the message already names and hides the cause, which is the useful part.
fn describe(e: &reqwest::Error) -> String {
    let mut text = if e.is_timeout() {
        "the connection timed out".to_string()
    } else if e.is_connect() {
        "couldn't connect".to_string()
    } else {
        e.to_string()
    };
    let mut source = std::error::Error::source(e);
    while let Some(inner) = source {
        text = format!("{text}: {inner}");
        source = inner.source();
    }
    text
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_is_in_the_catalog() {
        assert_eq!(spec(DEFAULT_MODEL).unwrap().id, "base.en");
    }

    #[test]
    fn unknown_model_names_the_known_ones() {
        let err = spec("large").unwrap_err().to_string();
        assert!(err.contains("\"large\""), "{err}");
        assert!(err.contains("tiny.en, base.en, small.en, base"), "{err}");
    }

    #[test]
    fn ids_are_trimmed() {
        assert_eq!(spec(" small.en ").unwrap().id, "small.en");
    }

    #[test]
    fn every_entry_is_pinned() {
        for m in MODELS {
            assert_eq!(m.sha256.len(), 64, "{}", m.id);
            assert!(m.sha256.chars().all(|c| c.is_ascii_hexdigit()), "{}", m.id);
            assert!(m.url().contains(REVISION), "{}", m.id);
            assert!(m.url().ends_with(m.file), "{}", m.id);
        }
    }

    #[test]
    fn installed_wants_the_exact_size() {
        let dir = tempfile::tempdir().unwrap();
        let models = Models::new(dir.path().to_path_buf());
        let tiny = spec("tiny.en").unwrap();
        assert!(!models.installed(tiny));
        assert!(matches!(
            models.ready(tiny),
            Err(VoiceError::ModelMissing(id)) if id == "tiny.en"
        ));

        std::fs::write(models.path(tiny), b"not a model").unwrap();
        assert!(!models.installed(tiny));
        let err = models.ready(tiny).unwrap_err().to_string();
        assert!(err.contains("11 bytes"), "{err}");
        assert!(err.contains("download it again"), "{err}");

        models.delete(tiny).unwrap();
        assert!(!models.path(tiny).exists());
        // Deleting what isn't there is fine.
        models.delete(tiny).unwrap();
    }

    #[tokio::test]
    async fn a_canceled_download_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let models = Models::new(dir.path().to_path_buf());
        let tiny = spec("tiny.en").unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = models.download(tiny, &cancel, |_, _| {}).await;
        assert!(matches!(result, Err(VoiceError::Canceled)), "{result:?}");
        assert!(!models.path(tiny).exists());
        assert!(!models.path(tiny).with_extension("bin.part").exists());
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(to_hex(&[0x0a, 0xff, 0x00]), "0aff00");
    }
}
