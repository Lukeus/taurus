//! The folder downloaded models live in, and the download.
//!
//! The same shape as the speech models' (`taurus_voice::models`): fetched to a
//! `.part` file, checked against the pinned size as it arrives and the pinned
//! SHA-256 at the end, and renamed into place only then. A model is
//! installed when every one of its files is there at its pinned size.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::catalog::{File, Spec};
use crate::EmbedError;

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

    /// The folder one model's files are in.
    pub fn path(&self, spec: &Spec) -> PathBuf {
        self.dir.join(spec.id)
    }

    /// Whether every file is on disk at the size it should be.
    ///
    /// Size, not checksum: hashing half a gigabyte each time Settings opens
    /// would be noticed, and the download already checked it.
    pub fn installed(&self, spec: &Spec) -> bool {
        self.ready(spec).is_ok()
    }

    /// The model's folder, or why it can't be used.
    pub fn ready(&self, spec: &Spec) -> Result<PathBuf, EmbedError> {
        let folder = self.path(spec);
        for file in &spec.files {
            let path = folder.join(file.name);
            match std::fs::metadata(&path) {
                Ok(m) if m.len() == file.bytes => {}
                Ok(m) => {
                    return Err(EmbedError::Load {
                        path: path.display().to_string(),
                        reason: format!("it's {} bytes, and should be {}", m.len(), file.bytes),
                    })
                }
                Err(_) => return Err(EmbedError::ModelMissing(spec.id.to_string())),
            }
        }
        Ok(folder)
    }

    /// Deletes the model's folder. Not an error if it isn't there.
    pub fn delete(&self, spec: &Spec) -> Result<(), EmbedError> {
        match std::fs::remove_dir_all(self.path(spec)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(EmbedError::Io(e.to_string())),
        }
    }

    /// Fetches every file the model needs, checks each, and puts it in place.
    ///
    /// A file already there at its pinned size is kept, so a download stopped
    /// after the 700 KB tokenizer doesn't fetch it again — and, more to the
    /// point, nothing is half-written: each file is renamed into place only
    /// after its checksum matches.
    ///
    /// `progress` gets `(done, total)` in bytes across all the files, at most
    /// a couple of hundred times.
    pub async fn download(
        &self,
        spec: &Spec,
        cancel: &CancellationToken,
        progress: impl Fn(u64, u64),
    ) -> Result<(), EmbedError> {
        let folder = self.path(spec);
        tokio::fs::create_dir_all(&folder)
            .await
            .map_err(|e| EmbedError::Io(format!("couldn't create {}: {e}", folder.display())))?;

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            // Between reads, not for the whole body: a slow connection that
            // keeps delivering is fine, one that stops for this long isn't.
            .read_timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| EmbedError::Io(e.to_string()))?;

        let total = spec.bytes();
        let step = (total / 200).max(1);
        // Atomic only because the future must be `Send` and a `Cell` borrowed
        // across an await isn't.
        let reported = std::sync::atomic::AtomicU64::new(0);
        let mut before = 0u64;
        progress(0, total);
        for file in &spec.files {
            let dest = folder.join(file.name);
            let have = tokio::fs::metadata(&dest).await.ok().map(|m| m.len());
            if have != Some(file.bytes) {
                fetch(&client, spec, file, &dest, cancel, |done| {
                    let at = before + done;
                    let last = reported.load(std::sync::atomic::Ordering::Relaxed);
                    if at.saturating_sub(last) >= step {
                        reported.store(at, std::sync::atomic::Ordering::Relaxed);
                        progress(at, total);
                    }
                })
                .await?;
            }
            before += file.bytes;
            progress(before, total);
        }
        Ok(())
    }
}

async fn fetch(
    client: &reqwest::Client,
    spec: &Spec,
    file: &File,
    dest: &Path,
    cancel: &CancellationToken,
    progress: impl Fn(u64),
) -> Result<(), EmbedError> {
    let url = spec.url(file);
    let part = dest.with_file_name(format!("{}.part", file.name));
    let fail = |reason: String| EmbedError::Download {
        file: file.name.to_string(),
        url: url.clone(),
        reason,
    };

    let response = tokio::select! {
        r = client.get(&url).send() => r.map_err(|e| fail(describe(&e)))?,
        () = cancel.cancelled() => return Err(EmbedError::Canceled),
    };
    if !response.status().is_success() {
        return Err(fail(format!("the server answered {}", response.status())));
    }

    let result = async {
        let write = |e: std::io::Error| fail(format!("couldn't write {}: {e}", part.display()));
        let mut out = tokio::fs::File::create(&part).await.map_err(write)?;
        let mut hasher = Sha256::new();
        let mut done = 0u64;
        let mut body = response.bytes_stream();
        loop {
            let chunk = tokio::select! {
                c = body.next() => c,
                () = cancel.cancelled() => return Err(EmbedError::Canceled),
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(|e| fail(describe(&e)))?;
            done += chunk.len() as u64;
            // Checked as it arrives, so a server sending far more than the
            // pinned size is stopped rather than allowed to fill the disk.
            if done > file.bytes {
                return Err(fail(format!(
                    "the server sent more than the expected {} bytes",
                    file.bytes
                )));
            }
            hasher.update(&chunk);
            out.write_all(&chunk).await.map_err(write)?;
            progress(done);
        }
        out.flush().await.map_err(write)?;
        drop(out);

        if done != file.bytes {
            return Err(fail(format!(
                "it stopped after {done} of {} bytes",
                file.bytes
            )));
        }
        let digest: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if digest != file.sha256 {
            return Err(fail(format!(
                "the file's checksum is {digest}, not the expected {}",
                file.sha256
            )));
        }
        tokio::fs::rename(&part, dest)
            .await
            .map_err(|e| fail(format!("couldn't move it into place: {e}")))
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
}

/// A reqwest error as one readable clause, cause included. Its own `Display`
/// names the URL, which the message already does, and hides the cause.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{spec, DEFAULT_MODEL};

    #[test]
    fn installed_wants_every_file_at_its_exact_size() {
        let dir = tempfile::tempdir().unwrap();
        let models = Models::new(dir.path().to_path_buf());
        let spec = spec(DEFAULT_MODEL).unwrap();
        assert!(!models.installed(spec));
        assert!(matches!(
            models.ready(spec),
            Err(EmbedError::ModelMissing(id)) if id == DEFAULT_MODEL
        ));

        std::fs::create_dir_all(models.path(spec)).unwrap();
        std::fs::write(models.path(spec).join("config.json"), b"{}").unwrap();
        let err = models.ready(spec).unwrap_err().to_string();
        assert!(err.contains("2 bytes"), "{err}");
        assert!(err.contains("download it again"), "{err}");

        models.delete(spec).unwrap();
        assert!(!models.path(spec).exists());
        models.delete(spec).unwrap();
    }

    #[tokio::test]
    async fn a_canceled_download_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let models = Models::new(dir.path().to_path_buf());
        let spec = spec(DEFAULT_MODEL).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = models.download(spec, &cancel, |_, _| {}).await;
        assert!(matches!(result, Err(EmbedError::Canceled)), "{result:?}");
        assert!(!models.installed(spec));
        for file in &spec.files {
            let part = models.path(spec).join(format!("{}.part", file.name));
            assert!(!part.exists(), "{}", part.display());
        }
    }
}
