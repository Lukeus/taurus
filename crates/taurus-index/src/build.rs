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
use std::time::{Duration, Instant};

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

/// How many times a refresh says where it has got to.
///
/// Not once per batch. A first index of this repository is around seventy
/// batches, and seventy near-identical lines is a wall of text rather than a
/// progress report — while one line at the start and nothing for forty-four
/// seconds is what made this feel hung. Twenty is a bar you can watch move.
const PROGRESS_STEPS: usize = 20;

/// How often a refresh in progress writes down what it has embedded.
///
/// A first index is a minute of requests, and until this existed a stop threw
/// all of it away — so the second attempt cost what the first one had, and the
/// only way to get an index was to sit through the whole of it uninterrupted.
/// Written every so often instead, a stop keeps everything up to the last
/// write and the next refresh carries on from there: the stale chunks of a
/// file that has not been re-embedded yet are already out of what is written,
/// so a scan finds it missing and embeds it, which is exactly the resume.
///
/// By the clock rather than by a chunk count, because the file holds every
/// vector in the workspace and grows as it goes: a fixed number of chunks
/// between writes costs a large repository quadratically, and ten seconds
/// costs it a fraction of the embedding it is interleaved with.
const SAVE_EVERY: Duration = Duration::from_secs(10);

/// Where a refresh reports what it is doing while it does it.
///
/// A trait rather than a callback because the two callers report to completely
/// different places: a tool call streams into the transcript through
/// [`taurus_tools::ToolProgress`], and the desktop app's **Build index** button
/// streams into a Tauri channel with no turn behind it at all.
#[async_trait::async_trait]
pub trait IndexProgress: Send + Sync {
    /// `done` of `total` passages embedded. Called at most [`PROGRESS_STEPS`]
    /// times plus once at the end, never on a refresh that embeds nothing.
    async fn embedding(&self, done: usize, total: usize);
}

/// Which `done` values are worth reporting, given how many there are in total.
///
/// Pure so the cadence can be tested without a backend, because getting it
/// wrong is invisible in the obvious direction: too few reports and the thing
/// looks hung again, which is the bug being fixed.
fn reports_at(done: usize, previous: usize, total: usize) -> bool {
    if done >= total {
        return true;
    }
    let bucket = |n: usize| n * PROGRESS_STEPS / total.max(1);
    bucket(done) > bucket(previous)
}

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
    progress: Option<&dyn IndexProgress>,
) -> Result<(Vec<Entry>, Refreshed), String> {
    // Off the runtime, all of it. What follows reads the whole index off disk,
    // walks the tree, and stats and reads every file in it — tens of thousands
    // of syscalls on a large repository, none of them yielding. Left inline it
    // occupied a runtime worker for the duration, and the thing sharing that
    // pool is the forwarder pumping a live turn's tokens into the window.
    let scanned = {
        let index = index.clone();
        let workspace = workspace.to_path_buf();
        let model = model.to_string();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || scan(&index, &workspace, &model, &cancel))
            .await
            .map_err(|e| format!("indexing failed to run: {e}"))??
    };
    let Scan {
        mut keep,
        pending,
        mut report,
    } = scanned;
    report.chunks = pending.len();
    let total = pending.len();
    let mut done = 0;
    let mut written = Instant::now();
    // How much of `keep` belongs to files that are entirely embedded.
    //
    // A part-embedded file must never be written down: a file is judged
    // current by its first chunk's length and modification time, so an index
    // holding half of one would report it as up to date and the other half
    // would never arrive. Everything carried over from the last index is whole
    // by definition, which is where this starts.
    let mut whole = keep.len();
    let mut current: Option<&str> = None;
    for batch in pending.chunks(BATCH) {
        if cancel.is_cancelled() {
            // What is embedded is worth keeping even though the answer is an
            // error: the caller asked for a refresh and is not getting one,
            // but the next refresh should not start over.
            keep.truncate(whole);
            let _ = write_down(index, model, keep).await;
            return Err("indexing was canceled".into());
        }
        let texts: Vec<String> = batch.iter().map(|(_, _, _, c)| c.text.clone()).collect();
        let vectors = provider
            .embed(model, &texts)
            .await
            .map_err(|e| e.to_string())?;

        for ((path, len, modified, piece), vector) in batch.iter().zip(vectors) {
            // A new file starting means the one before it is complete, and
            // everything already in `keep` is safe to write.
            if current != Some(path.as_str()) {
                whole = keep.len();
                current = Some(path.as_str());
            }
            keep.push(Entry {
                path: path.clone(),
                start_line: piece.start_line,
                end_line: piece.end_line,
                len: *len,
                modified: *modified,
                vector: encode(&vector),
            });
        }

        // Reported after the batch lands rather than before it is sent, so the
        // number is work finished rather than work started. On a first index
        // this is the only thing between the caller and forty-four seconds of
        // nothing.
        let before = done;
        done += batch.len();
        if let Some(progress) = progress {
            if reports_at(done, before, total) {
                progress.embedding(done, total).await;
            }
        }

        if written.elapsed() >= SAVE_EVERY && whole > 0 {
            // Split rather than cloned: these entries carry every vector in the
            // workspace, and the part-embedded file at the end of them is about
            // to be finished rather than thrown away.
            let rest = keep.split_off(whole);
            keep = write_down(index, model, keep).await?;
            keep.extend(rest);
            written = Instant::now();
        }
    }

    // Written only when something moved. A search on an unchanged workspace
    // should not rewrite a 20 MB file to say so — and when it does move, the
    // write goes to a blocking thread for the reason the scan does: 20 MB of
    // JSON serialized in-line is 20 MB a streaming turn is not being pumped
    // through.
    let keep = if report.embedded > 0 || report.removed > 0 {
        write_down(index, model, keep).await?
    } else {
        keep
    };

    Ok((keep, report))
}

/// Writes the index and hands the entries back.
///
/// Handed over and handed back rather than copied: these entries carry every
/// vector in the workspace, and the caller is about to go on embedding into
/// them. The write goes to a blocking thread because 20 MB of JSON serialized
/// in line is 20 MB a streaming turn is not being pumped through.
async fn write_down(index: &Index, model: &str, keep: Vec<Entry>) -> Result<Vec<Entry>, String> {
    let index = index.clone();
    let model = model.to_string();
    tokio::task::spawn_blocking(move || index.save(&model, &keep).map(|()| keep))
        .await
        .map_err(|e| format!("writing the index failed to run: {e}"))?
}

/// What one pass over the workspace found, before anything is embedded.
struct Scan {
    /// Entries still good, carried over from the index on disk.
    keep: Vec<Entry>,
    /// Chunks of files that moved, waiting for a vector.
    pending: Vec<(String, u64, u64, chunk::Chunk)>,
    report: Refreshed,
}

/// The whole synchronous half of a refresh: read the index, walk the tree, and
/// sort every file into "unchanged" or "needs embedding".
///
/// Split out so it can be handed to a blocking thread — see [`refresh`]. It is
/// also the only part that can be reasoned about without a runtime, which makes
/// it the part worth testing directly.
fn scan(
    index: &Index,
    workspace: &Path,
    model: &str,
    cancel: &CancellationToken,
) -> Result<Scan, String> {
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

    Ok(Scan {
        keep,
        pending,
        report,
    })
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

        let (_, first) = refresh(
            &f.index,
            &f.root,
            &provider,
            "m",
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(first.embedded, 2);
        assert!(first.chunks > 0);

        let (_, second) = refresh(
            &f.index,
            &f.root,
            &provider,
            "m",
            &CancellationToken::new(),
            None,
        )
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
        refresh(
            &f.index,
            &f.root,
            &provider,
            "m",
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();

        // A different length, so the stamp moves whatever the clock did.
        write(&f.root, "src/a.rs", 80);
        let (_, report) = refresh(
            &f.index,
            &f.root,
            &provider,
            "m",
            &CancellationToken::new(),
            None,
        )
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
        refresh(
            &f.index,
            &f.root,
            &provider,
            "m",
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();

        std::fs::remove_file(f.root.join("src/gone.rs")).unwrap();
        let (entries, report) = refresh(
            &f.index,
            &f.root,
            &provider,
            "m",
            &CancellationToken::new(),
            None,
        )
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
            None,
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
            None,
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
            None,
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
        refresh(
            &f.index,
            &f.root,
            &provider,
            "m",
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();

        let before = std::fs::metadata(f.index.path())
            .unwrap()
            .modified()
            .unwrap();
        refresh(
            &f.index,
            &f.root,
            &provider,
            "m",
            &CancellationToken::new(),
            None,
        )
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

        let result = refresh(&f.index, &f.root, &counting(), "m", &cancel, None).await;
        assert!(result.is_err());
    }

    /// Cancels the refresh partway through, from inside the embedding call, so
    /// a stop lands where a real one would — between batches, with work done.
    struct Stopping {
        cancel: CancellationToken,
        after: usize,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Provider for Stopping {
        fn id(&self) -> &str {
            "stopping"
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
            if self.calls.fetch_add(1, Ordering::SeqCst) + 1 >= self.after {
                self.cancel.cancel();
            }
            Ok(inputs
                .iter()
                .map(|text| {
                    let n = text.len() as f32;
                    vec![n.sin(), n.cos(), 1.0]
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn a_stopped_first_index_keeps_what_it_embedded() {
        // A first index is a minute of requests, and a stop used to throw all
        // of it away — so the way to get one was to sit through the whole of
        // it uninterrupted.
        let f = fixture();
        for n in 0..6 {
            write(&f.root, &format!("src/f{n}.rs"), 120);
        }

        let cancel = CancellationToken::new();
        let stopping: Arc<dyn Provider> = Arc::new(Stopping {
            cancel: cancel.clone(),
            after: 1,
            calls: AtomicUsize::new(0),
        });
        assert!(refresh(&f.index, &f.root, &stopping, "m", &cancel, None)
            .await
            .is_err());

        let kept = f.index.load("m");
        assert!(!kept.is_empty(), "a stop kept nothing at all");

        // And it resumes rather than starting over.
        let (entries, resumed) = refresh(
            &f.index,
            &f.root,
            &counting(),
            "m",
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert!(
            resumed.embedded < 6,
            "everything was embedded again: {} files",
            resumed.embedded
        );
        assert_eq!(resumed.embedded + resumed.unchanged, 6);

        // What it resumed to is what a run that was never stopped produces:
        // no file is left holding half its chunks.
        let clean = fixture();
        for n in 0..6 {
            write(&clean.root, &format!("src/f{n}.rs"), 120);
        }
        let (whole, _) = refresh(
            &clean.index,
            &clean.root,
            &counting(),
            "m",
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(entries.len(), whole.len());
    }

    #[tokio::test]
    async fn a_stop_never_writes_down_half_a_file() {
        // The failure this guards: a file is judged current by its first
        // chunk's stamp, so an index holding half of one reports it as up to
        // date and the rest never arrives.
        // Big enough to span several batches, so the stop lands inside it.
        let f = fixture();
        write(&f.root, "src/one.rs", 3000);

        let cancel = CancellationToken::new();
        let stopping: Arc<dyn Provider> = Arc::new(Stopping {
            cancel: cancel.clone(),
            after: 1,
            calls: AtomicUsize::new(0),
        });
        let _ = refresh(&f.index, &f.root, &stopping, "m", &cancel, None).await;

        assert!(
            f.index.load("m").is_empty(),
            "part of a file was written down as though it were all of it"
        );

        let (_, resumed) = refresh(
            &f.index,
            &f.root,
            &counting(),
            "m",
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(resumed.embedded, 1, "the half-indexed file was skipped");
    }

    /// Collects what a refresh reported, so the cadence can be asserted on.
    #[derive(Default)]
    struct Recording(std::sync::Mutex<Vec<(usize, usize)>>);

    #[async_trait]
    impl IndexProgress for Recording {
        async fn embedding(&self, done: usize, total: usize) {
            self.0.lock().unwrap().push((done, total));
        }
    }

    #[tokio::test]
    async fn a_first_index_reports_its_way_through_rather_than_going_quiet() {
        // The bug this closes: one line at the start and nothing for the next
        // forty-four seconds, which reads as a hung tool rather than a slow one.
        let f = fixture();
        for n in 0..12 {
            write(&f.root, &format!("src/f{n}.rs"), 120);
        }
        let progress = Recording::default();

        let (_, report) = refresh(
            &f.index,
            &f.root,
            &counting(),
            "m",
            &CancellationToken::new(),
            Some(&progress),
        )
        .await
        .unwrap();

        let seen = progress.0.lock().unwrap().clone();
        assert!(seen.len() > 1, "one report is the silence being fixed");
        assert!(
            seen.len() <= PROGRESS_STEPS + 1,
            "{} reports is a wall of text, not a progress bar",
            seen.len()
        );
        // Monotonic, and it finishes on the total rather than near it: a bar
        // that stops at 94% is worse than one that never moved.
        assert!(seen.windows(2).all(|w| w[0].0 < w[1].0), "{seen:?}");
        assert_eq!(seen.last(), Some(&(report.chunks, report.chunks)));
    }

    #[tokio::test]
    async fn a_refresh_with_nothing_to_do_reports_nothing() {
        // Every search refreshes first, and almost every one of those has no
        // work in it. A progress line there is noise on every single call.
        let f = fixture();
        write(&f.root, "src/a.rs", 50);
        let provider = counting();
        refresh(
            &f.index,
            &f.root,
            &provider,
            "m",
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();

        let progress = Recording::default();
        refresh(
            &f.index,
            &f.root,
            &provider,
            "m",
            &CancellationToken::new(),
            Some(&progress),
        )
        .await
        .unwrap();
        assert!(progress.0.lock().unwrap().is_empty());
    }

    #[test]
    fn the_last_passage_always_reports_however_the_buckets_fall() {
        // 7 chunks into 20 buckets: every step crosses a boundary, and the
        // final one has to report even when it does not.
        assert!(reports_at(7, 6, 7));
        assert!(reports_at(1, 0, 7));
        // 1000 chunks into 20 buckets is one report per 50, not per batch.
        assert!(!reports_at(17, 16, 1000));
        assert!(reports_at(50, 49, 1000));
        // A total of zero never divides by zero and never reports a step.
        assert!(reports_at(0, 0, 0));
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
