//! Walking the workspace and embedding what changed.
//!
//! The expensive half. Embedding a repository is thousands of HTTP requests
//! worth of work the first time and almost none of it every time after, so
//! everything here is about making the second case the common one and the first
//! case interruptible.
//!
//! # What is walked
//!
//! The files the search tools walk — the same [`ignore`] rules, so `target/`
//! and `node_modules/` are not embedded and neither is anything a `.gitignore`
//! excludes. That is a narrower rule than [`taurus_tools::sweep`] uses, and
//! deliberately: the sweep looks past an ignored *file* because `.env` is worth
//! being able to undo, while an index is a thing the model searches, and
//! putting secrets in front of it is the opposite of what anyone wants.
//!
//! # What is skipped
//!
//! Anything that is not UTF-8 text, anything past [`MAX_FILE_BYTES`], and
//! anything whose length and modification time match what the index already
//! holds. The last one is what makes a refresh cheap.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use taurus_provider::Provider;
use tokio_util::sync::CancellationToken;

use crate::chunk;
use crate::store::{encode, stamp, Entry, Index, MAX_FILES, MAX_FILE_BYTES};

/// Chunks sent to the backend in one request.
///
/// Ollama takes a batch and answers in order, so this trades round trips
/// against how much is lost when one fails. Sixteen keeps a large repository to
/// a few hundred requests while making a failure cost a fraction of a second's
/// work rather than a minute's.
const BATCH: usize = 16;

/// What a refresh did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Refreshed {
    /// Files re-embedded because they were new or had changed.
    pub embedded: usize,
    /// Files left alone because the index was already current for them.
    pub unchanged: usize,
    /// Entries dropped because their file is gone.
    pub removed: usize,
    pub chunks: usize,
    /// Why this covered less than the whole workspace, when that happened.
    pub caveat: Option<String>,
}

impl Refreshed {
    /// One line for a human, or for the tool result.
    pub fn summary(&self) -> String {
        let mut line = if self.embedded == 0 && self.removed == 0 {
            format!(
                "Index is current: {} files, nothing to re-read.",
                self.unchanged
            )
        } else {
            format!(
                "Indexed {} file{} ({} chunks); {} already current{}.",
                self.embedded,
                if self.embedded == 1 { "" } else { "s" },
                self.chunks,
                self.unchanged,
                if self.removed > 0 {
                    format!(", {} gone", self.removed)
                } else {
                    String::new()
                }
            )
        };
        if let Some(caveat) = &self.caveat {
            line.push(' ');
            line.push_str(caveat);
        }
        line
    }
}

/// Brings the index up to date with the workspace.
///
/// Returns the entries as they now stand alongside what it did, so a caller
/// that is about to search does not re-read what it just wrote.
pub async fn refresh(
    index: &Index,
    workspace: &Path,
    provider: &Arc<dyn Provider>,
    model: &str,
    cancel: &CancellationToken,
) -> Result<(Vec<Entry>, Refreshed), String> {
    let held = index.load(model);

    // Grouped by file, because staleness is a property of a file and every
    // chunk of a stale one has to go.
    let mut by_file: HashMap<String, Vec<Entry>> = HashMap::new();
    for entry in held {
        by_file.entry(entry.path.clone()).or_default().push(entry);
    }

    let (files, caveat) = walk(workspace);
    let mut report = Refreshed {
        caveat,
        ..Default::default()
    };

    let mut keep: Vec<Entry> = Vec::new();
    let mut pending: Vec<(String, u64, u64, chunk::Chunk)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (relative, absolute) in &files {
        if cancel.is_cancelled() {
            return Err("indexing was canceled".into());
        }
        seen.insert(relative.clone());

        let Some((len, modified)) = stamp(absolute) else {
            continue;
        };
        if len > MAX_FILE_BYTES {
            continue;
        }

        // Length and mtime, the comparison `make` and `rsync` have always
        // used. Every chunk of a file carries it, so one matching entry is
        // enough to vouch for the file.
        if let Some(existing) = by_file.get(relative) {
            if existing
                .first()
                .is_some_and(|e| e.len == len && e.modified == modified)
            {
                keep.extend(existing.iter().cloned());
                report.unchanged += 1;
                continue;
            }
        }

        let Ok(contents) = std::fs::read_to_string(absolute) else {
            // Not text. Nothing to embed and nothing to report — a binary in a
            // source tree is ordinary.
            continue;
        };
        let chunks = chunk::split(&contents);
        if chunks.is_empty() {
            continue;
        }
        report.embedded += 1;
        for piece in chunks {
            pending.push((relative.clone(), len, modified, piece));
        }
    }

    // Files the index knew about that are no longer there.
    report.removed = by_file.keys().filter(|path| !seen.contains(*path)).count();

    report.chunks = pending.len();
    for batch in pending.chunks(BATCH) {
        if cancel.is_cancelled() {
            return Err("indexing was canceled".into());
        }
        let texts: Vec<String> = batch.iter().map(|(_, _, _, c)| c.text.clone()).collect();
        let vectors = provider
            .embed(model, &texts)
            .await
            .map_err(|e| e.to_string())?;

        for ((path, len, modified, piece), vector) in batch.iter().zip(vectors) {
            keep.push(Entry {
                path: path.clone(),
                start_line: piece.start_line,
                end_line: piece.end_line,
                len: *len,
                modified: *modified,
                vector: encode(&vector),
            });
        }
    }

    // Written only when something moved. A search on an unchanged workspace
    // should not rewrite a 20 MB file to say so.
    if report.embedded > 0 || report.removed > 0 {
        index.save(model, &keep)?;
    }

    Ok((keep, report))
}

/// Every indexable file, as `(workspace-relative, absolute)`.
fn walk(workspace: &Path) -> (Vec<(String, std::path::PathBuf)>, Option<String>) {
    let mut files = Vec::new();
    let mut truncated = false;

    // The same rules the search tools walk under, plus `.taurus` — a session
    // transcript in the index would let a search over the project answer with
    // the conversation about the project.
    let walker = ignore::WalkBuilder::new(workspace)
        .hidden(false)
        // The same setting the search tools use. Without it, ignore rules only
        // apply inside a git repository — so a workspace that is not one would
        // have its `target/` embedded, which is both the slowest and the least
        // useful thing in it.
        .require_git(false)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            name != ".git" && name != ".taurus"
        })
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if files.len() >= MAX_FILES {
            truncated = true;
            break;
        }
        let path = entry.path().to_path_buf();
        let relative = taurus_tools::path_guard::display(workspace, &path);
        files.push((relative, path));
    }

    let caveat = truncated.then(|| {
        format!(
            "This workspace holds more than {MAX_FILES} files, so only the first {MAX_FILES} are \
             indexed; grep and glob still cover the rest."
        )
    });
    (files, caveat)
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use taurus_provider::{Capabilities, ChatRequest, ModelInfo, StopReason, StreamEvent};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    /// Embeds deterministically from the text, so a chunk's vector is stable
    /// across runs and two identical chunks agree — enough to test the walk,
    /// the staleness rule, and the batching without a model.
    struct Counting {
        calls: AtomicUsize,
        texts: AtomicUsize,
    }

    #[async_trait]
    impl Provider for Counting {
        fn id(&self) -> &str {
            "counting"
        }
        async fn models(&self) -> taurus_provider::Result<Vec<ModelInfo>> {
            Ok(Vec::new())
        }
        async fn capabilities(&self, _: &str) -> taurus_provider::Result<Capabilities> {
            Ok(Capabilities::default())
        }
        async fn stream(
            &self,
            _: ChatRequest,
            _: mpsc::Sender<StreamEvent>,
            _: CancellationToken,
        ) -> taurus_provider::Result<StopReason> {
            Ok(StopReason::EndTurn)
        }
        async fn embed(
            &self,
            _: &str,
            inputs: &[String],
        ) -> taurus_provider::Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.texts.fetch_add(inputs.len(), Ordering::SeqCst);
            Ok(inputs
                .iter()
                .map(|text| {
                    let n = text.len() as f32;
                    vec![n.sin(), n.cos(), 1.0]
                })
                .collect())
        }
    }

    fn counting() -> Arc<dyn Provider> {
        Arc::new(Counting {
            calls: AtomicUsize::new(0),
            texts: AtomicUsize::new(0),
        })
    }

    struct Fixture {
        root: std::path::PathBuf,
        index: Index,
        _dir: TempDir,
        _logs: TempDir,
    }

    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        let logs = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        Fixture {
            index: Index::new(logs.path(), &root),
            root,
            _dir: dir,
            _logs: logs,
        }
    }

    fn write(root: &Path, name: &str, lines: usize) {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let body: String = (0..lines)
            .map(|n| format!("fn thing_{n}() {{ compute({n}); }}\n"))
            .collect();
        std::fs::write(path, body).unwrap();
    }

    #[tokio::test]
    async fn a_first_pass_embeds_everything_and_a_second_embeds_nothing() {
        // The property the whole design turns on: indexing a repository is
        // expensive once and nearly free every time after.
        let f = fixture();
        write(&f.root, "src/a.rs", 50);
        write(&f.root, "src/b.rs", 50);
        let provider = counting();

        let (_, first) = refresh(&f.index, &f.root, &provider, "m", &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(first.embedded, 2);
        assert!(first.chunks > 0);

        let (_, second) = refresh(&f.index, &f.root, &provider, "m", &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(second.embedded, 0);
        assert_eq!(second.unchanged, 2);
        assert_eq!(second.chunks, 0);
    }

    #[tokio::test]
    async fn only_the_file_that_changed_is_re_embedded() {
        let f = fixture();
        write(&f.root, "src/a.rs", 50);
        write(&f.root, "src/b.rs", 50);
        let provider = counting();
        refresh(&f.index, &f.root, &provider, "m", &CancellationToken::new())
            .await
            .unwrap();

        // A different length, so the stamp moves whatever the clock did.
        write(&f.root, "src/a.rs", 80);
        let (_, report) = refresh(&f.index, &f.root, &provider, "m", &CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(report.embedded, 1);
        assert_eq!(report.unchanged, 1);
    }

    #[tokio::test]
    async fn a_deleted_file_leaves_the_index() {
        let f = fixture();
        write(&f.root, "src/a.rs", 50);
        write(&f.root, "src/gone.rs", 50);
        let provider = counting();
        refresh(&f.index, &f.root, &provider, "m", &CancellationToken::new())
            .await
            .unwrap();

        std::fs::remove_file(f.root.join("src/gone.rs")).unwrap();
        let (entries, report) =
            refresh(&f.index, &f.root, &provider, "m", &CancellationToken::new())
                .await
                .unwrap();

        assert_eq!(report.removed, 1);
        assert!(!entries.iter().any(|e| e.path.contains("gone.rs")));
    }

    #[tokio::test]
    async fn ignored_directories_are_not_indexed() {
        // Narrower than the sweep's rule on purpose. An index is a thing the
        // model searches, so `target/` is noise and `.env` is worse than noise.
        let f = fixture();
        std::fs::write(f.root.join(".gitignore"), "target/\nsecrets.txt\n").unwrap();
        write(&f.root, "src/a.rs", 50);
        write(&f.root, "target/generated.rs", 50);
        std::fs::write(f.root.join("secrets.txt"), "A".repeat(500)).unwrap();

        let (entries, _) = refresh(
            &f.index,
            &f.root,
            &counting(),
            "m",
            &CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(entries.iter().any(|e| e.path.contains("a.rs")));
        assert!(!entries.iter().any(|e| e.path.contains("generated.rs")));
        assert!(!entries.iter().any(|e| e.path.contains("secrets.txt")));
    }

    #[tokio::test]
    async fn taurus_own_directory_is_never_indexed() {
        // A transcript in the index would let a search over the project answer
        // with the conversation about the project.
        let f = fixture();
        write(&f.root, "src/a.rs", 50);
        write(&f.root, ".taurus/notes.md", 50);

        let (entries, _) = refresh(
            &f.index,
            &f.root,
            &counting(),
            "m",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(!entries.iter().any(|e| e.path.contains(".taurus")));
    }

    #[tokio::test]
    async fn a_binary_file_is_passed_over_without_complaint() {
        // A binary in a source tree is ordinary, not a problem to report.
        let f = fixture();
        write(&f.root, "src/a.rs", 50);
        std::fs::write(f.root.join("logo.png"), [0xff, 0xd8, 0x00, 0x01]).unwrap();

        let (entries, report) = refresh(
            &f.index,
            &f.root,
            &counting(),
            "m",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(report.embedded, 1);
        assert!(!entries.iter().any(|e| e.path.contains("logo.png")));
    }

    #[tokio::test]
    async fn nothing_is_written_when_nothing_moved() {
        // A search on an unchanged workspace must not rewrite the whole index
        // to say so.
        let f = fixture();
        write(&f.root, "src/a.rs", 50);
        let provider = counting();
        refresh(&f.index, &f.root, &provider, "m", &CancellationToken::new())
            .await
            .unwrap();

        let before = std::fs::metadata(f.index.path())
            .unwrap()
            .modified()
            .unwrap();
        refresh(&f.index, &f.root, &provider, "m", &CancellationToken::new())
            .await
            .unwrap();
        let after = std::fs::metadata(f.index.path())
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after, "the index was rewritten for no reason");
    }

    #[tokio::test]
    async fn a_cancel_stops_indexing_rather_than_finishing_the_repository() {
        let f = fixture();
        for n in 0..20 {
            write(&f.root, &format!("src/f{n}.rs"), 60);
        }
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = refresh(&f.index, &f.root, &counting(), "m", &cancel).await;
        assert!(result.is_err());
    }

    #[test]
    fn an_unchanged_index_says_so_rather_than_reporting_zero_of_everything() {
        // "Indexed 0 files (0 chunks); 412 already current" is arithmetic the
        // reader should not have to do.
        let report = Refreshed {
            unchanged: 412,
            ..Default::default()
        };
        assert!(
            report.summary().contains("Index is current"),
            "{}",
            report.summary()
        );
    }
}
