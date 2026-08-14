//! What a command changed, for tools that cannot say in advance.
//!
//! [`crate::Tool::touches`] is a declaration, and for the file tools it is
//! enough: `write_file` names its path before it writes, so the checkpoint
//! store can read what is there first. A shell command names nothing. It can
//! `sed -i` a hundred files, delete a directory, or run a script the model
//! wrote a moment earlier, and asking it what it will do is not a question the
//! harness can put.
//!
//! So this asks afterwards, by looking. The workspace is indexed before the
//! command runs and walked again when it finishes; anything whose length or
//! modification time moved, appeared, or vanished is a change, and the contents
//! held from the first pass become its pre-image in the checkpoint log. From
//! there a rewind treats it exactly like an `edit_file` — same records, same
//! restore, same first-capture-wins rule when an earlier tool in the same turn
//! already recorded the file.
//!
//! # What it covers
//!
//! The files the search tools walk, plus the ones an ignore rule excludes by
//! name.
//!
//! Those are not the same bound, and the difference is the point. Indexing
//! `target/` and `node_modules/` would cost gigabytes on every command, and a
//! rewind that deleted build output would be a worse surprise than one that
//! leaves it alone — so a directory an ignore rule excludes is not entered.
//! But a rule that excludes a *file* costs nothing to look past, and the file
//! it usually excludes is `.env`. A command that rewrites that one is exactly
//! the command a user reaches for undo on.
//!
//! The split falls out of the walk rather than being imposed on it. Every
//! directory the walk enters is also read flat, which surfaces the entries the
//! walk itself would skip; a directory the walk never enters is never read
//! here either, because it never arrives to be read. So `.env` beside a
//! `Cargo.toml` is covered and `target/` beside it is not, with no list of
//! special names deciding which is which.
//!
//! # What it costs
//!
//! One read of every covered file before each command, bounded by the
//! constants below, and one metadata-only walk after. For a source repository
//! that is a few megabytes; the caps are what keep a workspace full of large
//! assets from turning every command into a copy of itself. A file past a cap
//! is still *detected* — detection only needs length and modification time —
//! it just has no pre-image, so a rewind reports it instead of restoring it.
//!
//! Because `.env` is now held, the checkpoint log holds it too. That is why
//! [`crate::checkpoint`] keeps its logs readable by their owner and nobody
//! else: a file kept out of version control on purpose should not become
//! world-readable by being recoverable.
//!
//! # Where it is blind
//!
//! A change that moves neither length nor modification time is invisible. On a
//! filesystem with nanosecond timestamps this needs a deliberate effort; on one
//! with coarse timestamps a command that rewrites a file to the same length
//! within the same tick would slip through. Length and mtime is the comparison
//! `make` and `rsync` have always used, and reading every file twice to close
//! it would cost more than the gap is worth.
//!
//! Git's own state is not restored either, and here the sweep at least knows
//! it. `.git` is excluded from the walk, so a turn that ran `git checkout` or
//! `git reset --hard` has its files put back while `HEAD` and the index stay
//! where the command moved them — a tree matching neither commit. Snapshotting
//! the object store to fix that properly is a feature rather than a field, but
//! *noticing* costs two small reads: see [`GitState`]. A turn that moved git
//! and recorded files carries a caveat saying so, which is the difference
//! between a wrong tree and a wrong tree you were told about.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::checkpoint::{read_state, State, TurnRecorder};

/// Largest file whose contents a sweep holds.
///
/// A source file is kilobytes. Something past this is a database, a bundled
/// asset, or a checked-in binary — none of which a text pre-image could put
/// back correctly even if it were held.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Ceiling on everything one sweep holds at once, so a workspace of large text
/// files cannot turn a single command into a 2 GB allocation.
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// Most files a workspace may hold and still be swept. Past this the walk
/// itself is the cost, and it would be paid before and after every command.
const MAX_FILES: usize = 50_000;

/// One file as it stood before the command ran.
struct Indexed {
    len: u64,
    /// `None` on a filesystem that will not report one, which degrades this
    /// file to a length comparison rather than failing the sweep.
    modified: Option<SystemTime>,
    before: State,
}

impl Indexed {
    fn moved(&self, len: u64, modified: Option<SystemTime>) -> bool {
        self.len != len || self.modified != modified
    }
}

/// The workspace as it stood before a command, waiting to be compared with how
/// it stands after.
pub struct Sweep {
    files: HashMap<PathBuf, Indexed>,
    /// Length and mtime of every `.gitignore` and `.ignore` found, so the
    /// second pass can tell whether the rules it is walking under are the ones
    /// the first pass walked under. See [`Sweep::after`].
    ignores: HashMap<PathBuf, (u64, Option<SystemTime>)>,
    /// Where git stood, so a command that moved it can say so. See
    /// [`GitState`].
    git: Option<GitState>,
    /// Set when nothing was indexed, carrying the reason to report.
    abandoned: Option<String>,
}

/// What a sweep found, for the caller to report.
pub struct Change {
    /// Workspace-relative paths, sorted, in the order they were recorded.
    /// Empty when nothing changed.
    pub files: Vec<String>,
    /// How many of those have no pre-image, so a rewind can only name them.
    pub unrestorable: usize,
    /// Why this sweep saw less than it should have, when that happened.
    pub caveat: Option<String>,
}

impl Change {
    fn nothing() -> Self {
        Self {
            files: Vec::new(),
            unrestorable: 0,
            caveat: None,
        }
    }

    /// What the user has to be told, or `None` when the sweep covered the call
    /// and there is nothing to say.
    ///
    /// Only ever the bad news. What was recorded successfully needs no
    /// announcement — the changed-file count and the Changes drawer are read
    /// straight off the log, and a note on every build and test run would be
    /// noise drowning the few that matter. What could *not* be recorded has
    /// nowhere else to appear, and a turn that looks undoable and is not is the
    /// failure worth interrupting for.
    pub fn warning(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.unrestorable > 0 {
            parts.push(format!(
                "{} too large to copy, so a rewind will report {} rather than restore {}.",
                count(self.unrestorable, "changed file"),
                if self.unrestorable == 1 { "it" } else { "them" },
                if self.unrestorable == 1 { "it" } else { "them" },
            ));
        }
        if let Some(caveat) = &self.caveat {
            parts.push(caveat.clone());
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    /// The whole picture, for a human reading a report rather than a model
    /// reading a tool result. Used by the `sweep` example.
    pub fn summary(&self) -> Option<String> {
        match (self.files.is_empty(), self.warning()) {
            (true, warning) => warning,
            (false, warning) => {
                let mut line = format!(
                    "{} changed, recorded so this turn can be undone.",
                    count(self.files.len(), "file")
                );
                if let Some(warning) = warning {
                    line.push(' ');
                    line.push_str(&warning);
                }
                Some(line)
            }
        }
    }
}

impl Sweep {
    /// Indexes the workspace as it stands, before the command runs.
    pub async fn before(root: &Path) -> Self {
        let root = root.to_path_buf();
        tokio::task::spawn_blocking(move || index(&root))
            .await
            .unwrap_or_else(|e| {
                Sweep::abandoned(format!(
                    "The workspace could not be read before this command ({e}), so whatever it \
                     changes cannot be undone."
                ))
            })
    }

    fn abandoned(reason: String) -> Self {
        Self {
            files: HashMap::new(),
            ignores: HashMap::new(),
            git: None,
            abandoned: Some(reason),
        }
    }

    /// Walks again, records every difference, and reports what it found.
    ///
    /// Called whether the command succeeded, failed, timed out, or was
    /// canceled. A command killed halfway through has still written whatever it
    /// wrote, and that is exactly the turn a user reaches for undo on.
    pub async fn after(self, root: &Path, recorder: &TurnRecorder) -> Change {
        if let Some(reason) = self.abandoned {
            return Change {
                caveat: Some(reason),
                ..Change::nothing()
            };
        }

        let scan = {
            let root = root.to_path_buf();
            tokio::task::spawn_blocking(move || (current(&root), git_state(&root))).await
        };
        let Ok((Some(now), git_now)) = scan else {
            return Change {
                caveat: Some(
                    "The workspace could not be read after this command, so whatever it changed \
                     was not recorded and cannot be undone."
                        .into(),
                ),
                ..Change::nothing()
            };
        };

        // Whether the ignore rules still say what they said. A command that
        // edits `.gitignore` changes which files the second walk can even see,
        // and an entry that stops being ignored looks exactly like an entry the
        // command created. Recording those as created would let a later rewind
        // delete files that were sitting there all along, so when the rules
        // move, creations are left out and the caller is told why.
        let rules_held = self.ignores.iter().all(|(path, &stamp)| {
            now.get(path).map(|&(len, modified)| (len, modified)) == Some(stamp)
        }) && !now
            .keys()
            .any(|path| is_ignore_file(path) && !self.ignores.contains_key(path));

        let mut changed: Vec<(PathBuf, State)> = Vec::new();

        // Modified and deleted, both identified against the first pass's index.
        for (path, pre) in &self.files {
            let moved = match now.get(path) {
                Some(&(len, modified)) => pre.moved(len, modified),
                // Missing from the second walk is not the same as deleted. A
                // command that *adds* an ignore rule filters matching files out
                // of the walk while leaving them untouched on disk, and reading
                // absence as deletion would report them changed and copy their
                // contents into the log — which, for the file this happens to
                // most, is somebody's secrets written somewhere they never
                // asked for. So the filesystem is asked directly, and it is
                // still a real change if the file moved as well as vanished.
                None => match std::fs::metadata(path) {
                    Ok(meta) => pre.moved(meta.len(), meta.modified().ok()),
                    Err(_) => true,
                },
            };
            if moved {
                changed.push((path.clone(), pre.before.clone()));
            }
        }

        if rules_held {
            for path in now.keys() {
                if !self.files.contains_key(path) {
                    changed.push((path.clone(), State::Absent));
                }
            }
        }

        // Before anything is written down. Both halves above walk hash maps, so
        // without this the log — and the changed-file list the user reads out
        // of it — would come back in a different order every run.
        changed.sort_by(|(a, _), (b, _)| a.cmp(b));

        let unrestorable = changed
            .iter()
            .filter(|(_, state)| matches!(state, State::Opaque { .. }))
            .count();

        let files: Vec<String> = changed
            .iter()
            .map(|(path, _)| crate::path_guard::display(root, path))
            .collect();

        for (path, before) in changed {
            // Dropped silently when an earlier tool in this turn already
            // recorded the path, which is the behavior that wants keeping: the
            // earlier pre-image is the older one.
            recorder.capture_state(&path, before).await;
        }

        let mut caveats = Vec::new();
        // Narrower than the others on purpose: what was recorded is still
        // sound, and saying otherwise would be its own kind of wrong.
        if !rules_held {
            caveats.push(
                "An ignore rule changed while this command ran, so any file it stopped ignoring \
                 was left out and a rewind will not touch it."
                    .to_string(),
            );
        }
        // Only when something was recorded, which is what makes this a warning
        // rather than a running commentary on git. A `git commit` that touched
        // no working-tree file leaves nothing to undo, so nothing here looks
        // undoable and there is nothing to correct. A `git checkout` that
        // rewrote half the tree is the opposite: the files come back, `HEAD`
        // does not, and the result matches neither commit.
        if !files.is_empty() && self.git != git_now {
            caveats.push(
                "This command moved git's own state as well. A rewind puts the files back but \
                 leaves HEAD and the index where the command left them, so the result would match \
                 neither commit; `git reflog` is the way back to where HEAD was."
                    .to_string(),
            );
        }

        Change {
            files,
            unrestorable,
            caveat: (!caveats.is_empty()).then(|| caveats.join(" ")),
        }
    }
}

/// Every file one pass covers.
///
/// The walk, plus a flat read of each directory it enters. The second half is
/// what brings in a file an ignore rule excludes by name; see the module
/// header for why a directory it excludes stays out.
///
/// `None` when the workspace holds more than [`MAX_FILES`], which is a
/// different answer from holding none.
fn sweepable(root: &Path) -> Option<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = Vec::new();
    // The walk yields a directory before descending into it, so nearly every
    // file arrives from the flat read first and again from the walk. Which one
    // found it does not matter; recording it twice would.
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for entry in walker(root) {
        let Ok(entry) = entry else { continue };
        let Some(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            let Ok(listing) = std::fs::read_dir(entry.path()) else {
                continue;
            };
            for entry in listing.flatten() {
                // Files only, and never a descent: an ignored directory shows
                // up here as an entry and is passed over exactly like the walk
                // passes over it.
                if !entry.file_type().is_ok_and(|t| t.is_file()) {
                    continue;
                }
                // The walk excludes these by name and prunes them as
                // directories, which is how they usually appear. In a git
                // worktree `.git` is a regular *file* pointing elsewhere, and a
                // flat read would hand it over as ordinary content.
                if skipped(&entry.file_name()) {
                    continue;
                }
                if files.len() >= MAX_FILES {
                    return None;
                }
                let path = entry.path();
                if seen.insert(path.clone()) {
                    files.push(path);
                }
            }
            continue;
        }

        // Regular files only. A symlink belongs to whatever it points at, and
        // restoring one as text would replace the link with a file.
        if !file_type.is_file() {
            continue;
        }
        if files.len() >= MAX_FILES {
            return None;
        }
        let path = entry.into_path();
        if seen.insert(path.clone()) {
            files.push(path);
        }
    }

    Some(files)
}

/// The first pass: everything, with contents where they fit.
fn index(root: &Path) -> Sweep {
    let Some(paths) = sweepable(root) else {
        return Sweep::abandoned(format!(
            "This workspace holds more than {MAX_FILES} files, too many to record a command's \
             changes against, so this one cannot be undone."
        ));
    };

    let mut files = HashMap::new();
    let mut ignores = HashMap::new();
    let mut held: u64 = 0;

    for path in paths {
        let Ok(meta) = path.metadata() else { continue };
        let len = meta.len();
        let modified = meta.modified().ok();

        if is_ignore_file(&path) {
            ignores.insert(path.clone(), (len, modified));
        }

        let before = if len > MAX_FILE_BYTES {
            State::Opaque {
                reason: format!(
                    "was {} when it was recorded, above the {} a checkpoint holds",
                    bytes(len),
                    bytes(MAX_FILE_BYTES)
                ),
            }
        } else if held + len > MAX_TOTAL_BYTES {
            State::Opaque {
                reason: format!(
                    "was not held: this workspace has more than {} of files to record before a \
                     command runs",
                    bytes(MAX_TOTAL_BYTES)
                ),
            }
        } else {
            held += len;
            read_state(&path)
        };

        files.insert(
            path,
            Indexed {
                len,
                modified,
                before,
            },
        );
    }

    Sweep {
        files,
        ignores,
        git: git_state(root),
        abandoned: None,
    }
}

/// The second pass: lengths and timestamps, no contents.
///
/// `None` when the workspace grew past [`MAX_FILES`] while the command ran,
/// which makes the comparison meaningless rather than merely incomplete.
fn current(root: &Path) -> Option<HashMap<PathBuf, (u64, Option<SystemTime>)>> {
    let mut now = HashMap::new();
    for path in sweepable(root)? {
        let Ok(meta) = path.metadata() else { continue };
        now.insert(path, (meta.len(), meta.modified().ok()));
    }
    Some(now)
}

/// Where git stood, in the three places a command usually moves it.
///
/// Not enough to put anything back — that would mean snapshotting the object
/// store, which is a feature rather than a field. Enough to *notice*, which is
/// what separates a rewind that leaves a tree matching neither commit from one
/// that leaves it that way and says so.
///
/// `.git` is excluded from the sweep and stays excluded: this reads it, and
/// nothing here records or restores it.
#[derive(PartialEq, Eq)]
struct GitState {
    /// `.git/HEAD`: which branch is checked out, or which commit when
    /// detached. Moved by `git checkout` and `git switch`.
    head: String,
    /// The ref `HEAD` names, read through. `git commit` and `git reset` leave
    /// `HEAD` pointing at the same branch and move the branch instead, so
    /// without this the two most common cases look like nothing happened.
    reference: Option<String>,
}

// `.git/index` is deliberately not here, though a rewind does not carry staging
// either. `git status` refreshes the index's stat cache and writes it back, and
// a model runs `git status` constantly — so watching the index would attach a
// warning to most turns that ran one alongside an edit. A caveat that appears
// on ordinary turns is one nobody reads by the time it matters. The cost is
// that a bare `git add` goes unremarked; every case that moves the working tree
// moves `HEAD` or a branch, and those are watched.

/// Reads that state, or `None` where there is no repository to read.
fn git_state(root: &Path) -> Option<GitState> {
    let dir = git_dir(root)?;
    let head = std::fs::read_to_string(dir.join("HEAD")).ok()?;

    let reference = head
        .strip_prefix("ref:")
        .map(str::trim)
        // `HEAD` is a file in the workspace being worked on, so joining
        // whatever it holds would follow `../` wherever it liked. A ref name is
        // the only thing worth following, and it is easy to insist on.
        .filter(|name| name.starts_with("refs/") && !name.contains(".."))
        // Absent when the ref is packed rather than loose. That reads as `None`
        // both times and so reports nothing, which is the right way for this to
        // fail: git writes a loose ref whenever it moves one, so the update
        // itself is still seen.
        .and_then(|name| std::fs::read_to_string(dir.join(name)).ok());

    Some(GitState { head, reference })
}

/// The directory git keeps its state in, which is not always `.git` itself.
///
/// In a worktree `.git` is a regular file naming the real directory, and
/// reading `HEAD` from the file would find nothing at all.
fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if std::fs::metadata(&dot_git).ok()?.is_dir() {
        return Some(dot_git);
    }

    let pointer = std::fs::read_to_string(&dot_git).ok()?;
    let target = Path::new(pointer.strip_prefix("gitdir:")?.trim());
    Some(match target.is_absolute() {
        true => target.to_path_buf(),
        false => root.join(target),
    })
}

/// Never swept, whatever the ignore rules say about them.
///
/// `.git` is the object store: a rewind that restored it would put `HEAD` and
/// the index back without the commits they name. `.taurus/` is the harness's
/// own state — this project's permission grants and settings — and a rewind
/// that put that back would revoke permissions the user granted, which is not
/// a file change they asked to undo. The agent may still read either; they
/// just do not travel with the turn.
const SKIP: &[&str] = &[".git", ".taurus"];

fn skipped(name: &std::ffi::OsStr) -> bool {
    SKIP.iter().any(|skip| name == *skip)
}

/// The traversal, shared with the search tools so that "the workspace" means
/// one set of directories whether the agent is grepping it or changing it.
///
/// Which *files* come back differs, and deliberately: search skips what an
/// ignore rule excludes, and [`sweepable`] looks once more inside every
/// directory this enters. The asymmetry only runs one way — a file the agent
/// cannot grep may still be recorded, never the reverse — so nothing the agent
/// can destroy goes unrecorded.
fn walker(root: &Path) -> ignore::Walk {
    crate::builtin::search::walker_skipping(root, SKIP)
}

fn is_ignore_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == ".gitignore" || name == ".ignore")
}

/// Sizes as a person reads them, for a message a person reads.
fn bytes(n: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const KB: u64 = 1024;
    if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{} KB", n / KB)
    } else {
        format!("{n} bytes")
    }
}

fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::{CheckpointStore, Restored};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// A workspace, and a checkpoint log to record against.
    struct Fixture {
        store: CheckpointStore,
        recorder: Arc<TurnRecorder>,
        root: PathBuf,
        _logs: TempDir,
        _workspace: TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let logs = TempDir::new().unwrap();
            let workspace = TempDir::new().unwrap();
            // Canonicalized for the same reason the checkpoint tests do it: on
            // macOS the temp dir sits behind /var -> /private/var, and a root
            // that disagrees with the walker's paths would record absolutes.
            let root = workspace.path().canonicalize().unwrap();
            let store = CheckpointStore::new(logs.path());
            let recorder = store.begin_turn("s1", &root, "run something");
            Self {
                store,
                recorder,
                root,
                _logs: logs,
                _workspace: workspace,
            }
        }

        fn write(&self, name: &str, content: &str) {
            let path = self.root.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }

        fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.root.join(name)).unwrap()
        }

        fn exists(&self, name: &str) -> bool {
            self.root.join(name).exists()
        }

        /// Indexes, runs `act`, then records what it did.
        async fn around(&self, act: impl FnOnce()) -> Change {
            let sweep = Sweep::before(&self.root).await;
            act();
            sweep.after(&self.root, &self.recorder).await
        }

        fn rewind(&self) -> Vec<Restored> {
            self.store.rewind("s1", &self.root, 1, false).unwrap()
        }

        fn log_path(&self) -> PathBuf {
            self._logs.path().join("s1.jsonl")
        }

        /// Asserts there was nothing to undo, then proves the file survived it.
        fn rewind_expecting_nothing(&self) {
            assert!(
                self.store.turns("s1").unwrap().is_empty(),
                "nothing should have been recorded"
            );
        }
    }

    #[tokio::test]
    async fn a_file_a_command_rewrote_goes_back() {
        let f = Fixture::new();
        f.write("a.txt", "original");

        let change = f
            .around(|| f.write("a.txt", "what the command wrote"))
            .await;

        assert_eq!(change.files, vec!["a.txt"]);
        f.rewind();
        assert_eq!(f.read("a.txt"), "original");
    }

    #[tokio::test]
    async fn a_file_a_command_created_is_deleted_again() {
        let f = Fixture::new();

        let change = f.around(|| f.write("built.txt", "output")).await;

        assert_eq!(change.files, vec!["built.txt"]);
        f.rewind();
        assert!(!f.exists("built.txt"), "a created file must not survive");
    }

    #[tokio::test]
    async fn a_file_a_command_deleted_comes_back() {
        // The case the old gap was worst for: nothing else in the harness
        // remembers the bytes, so without this they are simply gone.
        let f = Fixture::new();
        f.write("doomed.txt", "please keep me");

        let change = f
            .around(|| std::fs::remove_file(f.root.join("doomed.txt")).unwrap())
            .await;

        assert_eq!(change.files, vec!["doomed.txt"]);
        f.rewind();
        assert_eq!(f.read("doomed.txt"), "please keep me");
    }

    #[tokio::test]
    async fn a_command_that_changed_nothing_records_nothing() {
        // Most commands build, test, or list. None of them should put a turn in
        // the log, or the changed-file list stops being worth reading.
        let f = Fixture::new();
        f.write("a.txt", "untouched");

        let change = f.around(|| {}).await;

        assert!(change.files.is_empty());
        assert!(change.summary().is_none(), "silence is the common case");
        assert!(f.store.turns("s1").unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_directory_a_command_wrote_into_is_recorded_file_by_file() {
        let f = Fixture::new();

        let change = f
            .around(|| {
                f.write("out/one.txt", "1");
                f.write("out/two.txt", "2");
            })
            .await;

        assert_eq!(change.files, vec!["out/one.txt", "out/two.txt"]);
        f.rewind();
        assert!(!f.exists("out/one.txt"));
        assert!(!f.exists("out/two.txt"));
    }

    #[tokio::test]
    async fn what_was_recorded_reads_back_in_a_stable_order() {
        // Creations and modifications are found by walking two hash maps, so
        // the order they are written down in is only stable if it is made so.
        // The user reads this list off the log; it must not reshuffle itself
        // between one look and the next.
        let f = Fixture::new();
        f.write("b.txt", "before");
        f.write("d.txt", "before");

        let change = f
            .around(|| {
                f.write("a.txt", "created");
                f.write("b.txt", "modified");
                std::fs::remove_file(f.root.join("d.txt")).unwrap();
                f.write("c.txt", "created");
            })
            .await;

        assert_eq!(change.files, vec!["a.txt", "b.txt", "c.txt", "d.txt"]);
        assert_eq!(
            f.store.turns("s1").unwrap()[0].files,
            vec!["a.txt", "b.txt", "c.txt", "d.txt"],
            "the log has to agree with what was reported"
        );
    }

    #[tokio::test]
    async fn an_earlier_capture_in_the_same_turn_stays_the_pre_image() {
        // A turn that edits a file and then runs a command over it has two
        // pre-images available. The older one is the one the user last saw.
        let f = Fixture::new();
        f.write("a.txt", "what the user had");

        f.recorder.capture(&f.root.join("a.txt")).await;
        f.write("a.txt", "what edit_file left");
        f.around(|| f.write("a.txt", "what the command left")).await;

        f.rewind();
        assert_eq!(f.read("a.txt"), "what the user had");
    }

    #[tokio::test]
    async fn a_file_ignored_by_name_still_goes_back() {
        // The case this exists for. `.env` is ignored precisely because it
        // matters, and a command that clobbers it is the one a user reaches
        // for undo on. Looking past a rule that names a file costs one flat
        // read of a directory already being walked.
        let f = Fixture::new();
        f.write(".gitignore", ".env\n");
        f.write(".env", "SECRET_KEY=hunter2");

        let change = f.around(|| f.write(".env", "clobbered")).await;

        assert_eq!(change.files, vec![".env"]);
        f.rewind();
        assert_eq!(f.read(".env"), "SECRET_KEY=hunter2");
    }

    #[tokio::test]
    async fn a_file_inside_an_ignored_directory_is_left_alone() {
        // The other half of the same rule, and the reason it is drawn at
        // directories. Indexing `target/` and `node_modules/` would cost
        // gigabytes on every command, and a rewind that deleted build output
        // would be a worse surprise than one that leaves it be.
        let f = Fixture::new();
        f.write(".gitignore", "build/\n");
        f.write("build/out.o", "before");

        let change = f.around(|| f.write("build/out.o", "rebuilt")).await;

        assert!(change.files.is_empty(), "{:?}", change.files);
        assert_eq!(f.read("build/out.o"), "rebuilt");
    }

    #[tokio::test]
    async fn a_worktrees_git_file_does_not_travel_with_the_turn() {
        // `.git` is normally a directory and the walk prunes it by name. In a
        // git worktree it is a regular *file* pointing elsewhere, and the flat
        // read that finds `.env` would hand it over as ordinary content —
        // leaving a rewind able to detach the worktree from its repository.
        let f = Fixture::new();
        f.write(".git", "gitdir: /elsewhere/.git/worktrees/w\n");

        let change = f
            .around(|| f.write(".git", "gitdir: /somewhere/else\n"))
            .await;

        assert!(change.files.is_empty(), "{:?}", change.files);
        f.rewind_expecting_nothing();
    }

    #[tokio::test]
    async fn an_ignored_file_a_command_created_is_deleted_again() {
        // Creation has to follow the same rule as modification, or a rewind
        // leaves behind exactly the file it was asked to remove.
        let f = Fixture::new();
        f.write(".gitignore", ".env\n");

        let change = f.around(|| f.write(".env", "written by the command")).await;

        assert_eq!(change.files, vec![".env"]);
        f.rewind();
        assert!(!f.exists(".env"), "a created file must not survive");
    }

    #[tokio::test]
    async fn a_rule_that_moved_cannot_let_a_rewind_delete_what_it_reveals() {
        // The sharp edge. `dist/` was ignored and full of files that predate
        // the turn; the command un-ignores it. Those files look exactly like
        // ones the command created, and treating them that way would have a
        // rewind delete work nobody asked it to touch.
        let f = Fixture::new();
        f.write(".gitignore", "dist/\n");
        f.write("dist/keep.txt", "older than this turn");

        let change = f
            .around(|| f.write(".gitignore", "# nothing ignored\n"))
            .await;

        assert_eq!(
            change.files,
            vec![".gitignore"],
            "only the rule file itself changed"
        );
        assert!(change.caveat.is_some(), "the user has to be told why");

        f.rewind();
        assert!(
            f.exists("dist/keep.txt"),
            "a rewind deleted a file the turn never created"
        );
    }

    #[tokio::test]
    async fn a_rule_that_started_ignoring_a_file_does_not_report_it_deleted() {
        // The other direction of the same sharp edge, and the worse one. A
        // command that *adds* an ignore rule makes matching files vanish from
        // the second walk, which looks exactly like deletion — so the file gets
        // reported as changed and its contents copied into the checkpoint log.
        // For the file this usually happens to, that is somebody's secrets
        // written somewhere they never asked for.
        let f = Fixture::new();
        f.write(".gitignore", "nothing\n");
        f.write(".env", "SECRET_KEY=hunter2");

        let change = f.around(|| f.write(".gitignore", "nothing\n.env\n")).await;

        assert_eq!(
            change.files,
            vec![".gitignore"],
            "only the rule file itself changed"
        );
        let log = std::fs::read_to_string(f.log_path()).unwrap_or_default();
        assert!(
            !log.contains("hunter2"),
            "the contents of a newly ignored file were copied into the log"
        );
    }

    #[tokio::test]
    async fn a_file_that_became_ignored_but_also_changed_is_still_recorded() {
        // The case the existence check must not swallow: the command edited the
        // file *and* started ignoring it. It is a real change, and the rule
        // moving underneath is no reason to lose it.
        let f = Fixture::new();
        f.write(".gitignore", "nothing\n");
        f.write("notes.txt", "before");

        let change = f
            .around(|| {
                f.write("notes.txt", "after");
                f.write(".gitignore", "nothing\nnotes.txt\n");
            })
            .await;

        assert_eq!(change.files, vec![".gitignore", "notes.txt"]);
        f.rewind();
        assert_eq!(f.read("notes.txt"), "before");
    }

    #[tokio::test]
    async fn a_file_too_large_to_hold_is_reported_rather_than_restored() {
        let f = Fixture::new();
        let big = "x".repeat(MAX_FILE_BYTES as usize + 1);
        f.write("big.bin", &big);

        let change = f.around(|| f.write("big.bin", "truncated")).await;

        assert_eq!(change.files, vec!["big.bin"]);
        assert_eq!(change.unrestorable, 1);
        // And it reaches the user on the tool result, not only at rewind time:
        // learning a file was never recoverable when you go to recover it is
        // learning it too late.
        let warning = change.warning().expect("an uncopyable file must be said");
        assert!(warning.contains("too large to copy"), "{warning}");
        assert!(warning.contains("1 changed file"), "{warning}");

        let restored = f.rewind();
        assert!(matches!(restored[0], Restored::Skipped { .. }));
        assert_eq!(
            f.read("big.bin"),
            "truncated",
            "there was nothing to put back, and pretending otherwise would be worse"
        );
    }

    #[tokio::test]
    async fn the_harnesss_own_state_does_not_travel_with_the_turn() {
        // `.taurus/` holds this project's permission grants. Undoing a turn
        // must not quietly revoke a permission the user chose to grant.
        let f = Fixture::new();
        f.write(".taurus/permissions.json", "{\"allowed\":[]}");

        let change = f
            .around(|| {
                f.write(
                    ".taurus/permissions.json",
                    "{\"allowed\":[\"run_command\"]}",
                )
            })
            .await;

        assert!(change.files.is_empty());
        f.rewind_expecting_nothing();
        assert!(f.read(".taurus/permissions.json").contains("run_command"));
    }

    impl Fixture {
        /// A repository, as far as anything here reads one: `HEAD` naming a
        /// branch, that branch naming a commit, and an index.
        fn repo(&self, branch: &str, commit: &str) {
            self.write(".git/HEAD", &format!("ref: refs/heads/{branch}\n"));
            self.write(&format!(".git/refs/heads/{branch}"), &format!("{commit}\n"));
            self.write(".git/index", "an index");
        }
    }

    #[tokio::test]
    async fn a_command_that_checked_out_a_branch_says_a_rewind_will_not_undo_it() {
        // The turn undo looks equal to and is not. The files come back; `HEAD`
        // stays where the checkout left it, and the result is a tree matching
        // neither commit.
        let f = Fixture::new();
        f.repo("main", "aaaaaaa");
        f.write("a.txt", "on main");

        let change = f
            .around(|| {
                f.write("a.txt", "on the other branch");
                f.write(".git/HEAD", "ref: refs/heads/other\n");
            })
            .await;

        assert_eq!(change.files, vec!["a.txt"], "the file is still recorded");
        let caveat = change.caveat.expect("moving HEAD has to be said");
        assert!(caveat.contains("git reflog"), "{caveat}");
    }

    #[tokio::test]
    async fn a_branch_that_moved_under_a_still_head_is_noticed() {
        // `git reset --hard` and `git commit` both leave `HEAD` pointing at the
        // same branch and move the branch instead. Reading only `HEAD` would
        // call the two most common cases uneventful.
        let f = Fixture::new();
        f.repo("main", "aaaaaaa");
        f.write("a.txt", "before");

        let change = f
            .around(|| {
                f.write("a.txt", "after");
                f.write(".git/refs/heads/main", "bbbbbbb\n");
            })
            .await;

        assert_eq!(change.files, vec!["a.txt"]);
        assert!(change.caveat.is_some(), "a moved branch has to be said");
    }

    #[tokio::test]
    async fn a_command_that_left_git_alone_says_nothing_about_it() {
        // Every ordinary edit happens inside a repository. If this spoke up for
        // those, it would be noise on almost every turn and read as nothing.
        let f = Fixture::new();
        f.repo("main", "aaaaaaa");
        f.write("a.txt", "before");

        let change = f.around(|| f.write("a.txt", "after")).await;

        assert_eq!(change.files, vec!["a.txt"]);
        assert!(change.caveat.is_none(), "{:?}", change.caveat);
    }

    #[tokio::test]
    async fn git_moving_with_nothing_to_undo_is_not_worth_saying() {
        // `git commit` moves the branch and touches no working-tree file. There
        // is no turn in the log, so nothing looks undoable, so there is nothing
        // to correct — and a note on every commit would drown the ones that
        // matter.
        let f = Fixture::new();
        f.repo("main", "aaaaaaa");

        let change = f
            .around(|| f.write(".git/refs/heads/main", "bbbbbbb\n"))
            .await;

        assert!(change.files.is_empty());
        assert!(change.caveat.is_none(), "{:?}", change.caveat);
    }

    #[tokio::test]
    async fn a_refreshed_index_is_not_mistaken_for_a_command_that_moved_git() {
        // `git status` rewrites the index to refresh its stat cache, and a
        // model runs `git status` constantly. Watching the index would put a
        // warning on most turns that ran one next to an edit, and a caveat that
        // shows up on ordinary turns is one nobody reads by the time it counts.
        let f = Fixture::new();
        f.repo("main", "aaaaaaa");
        f.write("a.txt", "before");

        let change = f
            .around(|| {
                f.write("a.txt", "after");
                f.write(".git/index", "a refreshed index");
            })
            .await;

        assert_eq!(change.files, vec!["a.txt"]);
        assert!(change.caveat.is_none(), "{:?}", change.caveat);
    }

    #[tokio::test]
    async fn a_workspace_that_is_not_a_repository_has_nothing_to_say() {
        let f = Fixture::new();
        f.write("a.txt", "before");

        let change = f.around(|| f.write("a.txt", "after")).await;

        assert_eq!(change.files, vec!["a.txt"]);
        assert!(change.caveat.is_none(), "{:?}", change.caveat);
    }

    #[tokio::test]
    async fn a_worktree_keeps_its_state_somewhere_else_and_is_still_read() {
        // In a worktree `.git` is a regular file naming the real directory.
        // Reading `HEAD` from beside it finds nothing, and a checkout would go
        // unremarked in exactly the setup where branches move most.
        let f = Fixture::new();
        let elsewhere = TempDir::new().unwrap();
        let git = elsewhere.path().canonicalize().unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        f.write(".git", &format!("gitdir: {}\n", git.display()));
        f.write("a.txt", "before");

        let change = f
            .around(|| {
                f.write("a.txt", "after");
                std::fs::write(git.join("HEAD"), "ref: refs/heads/other\n").unwrap();
            })
            .await;

        assert_eq!(change.files, vec!["a.txt"]);
        assert!(
            change.caveat.is_some(),
            "a worktree's HEAD moved unremarked"
        );
    }

    #[tokio::test]
    async fn a_head_pointing_outside_the_repository_is_not_followed() {
        // `HEAD` is a file in the workspace being worked on. Joining whatever
        // it holds would read wherever it pointed, so only a ref name is
        // followed and anything else reads as no ref at all.
        let f = Fixture::new();
        f.write(".git/HEAD", "ref: ../../../../etc/passwd\n");
        f.write("a.txt", "before");

        let change = f.around(|| f.write("a.txt", "after")).await;

        assert_eq!(change.files, vec!["a.txt"]);
        assert!(change.caveat.is_none(), "{:?}", change.caveat);
    }

    #[tokio::test]
    async fn a_workspace_too_large_to_index_says_so_instead_of_going_quiet() {
        let f = Fixture::new();
        let sweep = Sweep::abandoned("this workspace has too many files".into());
        let change = sweep.after(&f.root, &f.recorder).await;

        assert!(change.files.is_empty());
        assert_eq!(
            change.summary().as_deref(),
            Some("this workspace has too many files"),
            "an unwatched command must not look like an uneventful one"
        );
    }

    #[test]
    fn sizes_are_written_the_way_they_are_read() {
        assert_eq!(bytes(512), "512 bytes");
        assert_eq!(bytes(2048), "2 KB");
        assert_eq!(bytes(3 * 1024 * 1024), "3.0 MB");
    }
}
