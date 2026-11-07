//! Watching a workspace, for consumers that would otherwise walk it to find
//! out what changed.
//!
//! The first consumer is the sweep in `taurus-tools`, which walks the whole
//! workspace before and after every command so a turn can be undone. On a
//! repository of 20,000–30,000 files that walk is most of a command's
//! overhead; with a watcher it shrinks to statting the handful of paths that
//! actually moved.
//!
//! # The contract
//!
//! [`Watch::changes`] answers with every path touched since a [`Mark`], or with
//! [`Since::Everything`] and a reason. It never answers with a list it is not
//! sure is complete — see [`journal`] for the rule and why it is git
//! fsmonitor's. Each of these produces `Everything`:
//!
//! - the operating system reporting dropped events (`Flag::Rescan`, raised for
//!   FSEvents' `MustScanSubDirs` and inotify's queue overflow);
//! - any error from the watcher;
//! - more paths between two questions than the journal keeps;
//! - the watcher not confirming it has caught up (below).
//!
//! And once the watcher cannot see changes at all — Linux's inotify watch
//! limit reached — every answer is `Everything` for as long as it lives.
//!
//! # Knowing the watcher has caught up
//!
//! Events arrive a moment after the change. A command that writes a file and
//! exits has not necessarily been reported yet when its caller asks what it
//! changed, and an answer missing the write would be a turn that cannot be
//! undone. So before answering, [`Watch::changes`] writes a cookie file and
//! waits for its event: events arrive in order, so once the cookie has arrived
//! every change made before it was written has too. That is how Watchman and
//! git's own fsmonitor daemon solve the same problem, and like git's daemon
//! the wait is bounded — a second, after which the answer is `Everything`.
//!
//! The cookie goes inside the repository's git directory, where git's daemon
//! puts its own, so nothing appears in the working tree. A workspace that is
//! not a git repository is not watched; its consumers keep walking.
//!
//! On macOS Watchman's documentation says FSEvents does not strictly promise
//! that order. The consumer is expected to cross-check the watcher against a
//! walk now and then and stop trusting it if they disagree; the sweep does.
//!
//! # Per platform
//!
//! - **macOS** watches the root recursively. FSEvents costs nothing per
//!   directory, so `target/` and `node_modules/` are watched and their events
//!   filtered by the consumer.
//! - **Linux** watches nothing below the root on its own. inotify needs one
//!   watch per directory and they are a per-user kernel resource, so a
//!   recursive watch into `node_modules/` exhausts them for every other program
//!   on the machine — which is what Cursor's and Codex's editor extensions were
//!   reported doing. The consumer names the directories it covers, with
//!   [`Watch::watch_dirs`], and only those are watched.
//! - **Windows** watches recursively, but nothing in Taurus trusts it for undo
//!   yet: notify 8.2 can lose a `ReadDirectoryChangesW` buffer overflow
//!   without reporting it, and only its 9.0 release candidates fix that.
//! - **Network and FUSE mounts** are refused on every platform. See [`mounts`].

mod journal;
mod mounts;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use journal::Journal;
pub use journal::{Mark, Since};

/// How long [`Watch::changes`] waits for its cookie by default. git's
/// fsmonitor daemon waits the same.
pub const SYNC_TIMEOUT: Duration = Duration::from_secs(1);

/// Where cookies go, inside the git directory.
const COOKIES: &str = "taurus-cookies";

/// A workspace being watched.
///
/// Stops watching when dropped.
pub struct Watch {
    shared: Arc<Shared>,
    watcher: Mutex<RecommendedWatcher>,
    cookies: PathBuf,
    next_cookie: AtomicU64,
}

struct Shared {
    journal: Mutex<Journal>,
    arrived: Condvar,
}

impl Shared {
    fn journal(&self) -> MutexGuard<'_, Journal> {
        // A panic while holding this could only come from the handler, and the
        // journal it leaves is still a valid journal. Losing everything is the
        // safe reading of an unknown state.
        match self.journal.lock() {
            Ok(journal) => journal,
            Err(poisoned) => {
                let mut journal = poisoned.into_inner();
                journal.lose("the watcher stopped partway through an update".into());
                journal
            }
        }
    }
}

/// Why a workspace is not being watched. Its consumers walk instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unwatched(pub String);

impl std::fmt::Display for Unwatched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Watch {
    /// Starts watching `root`, which should be absolute.
    ///
    /// Changes are reported under `root` as given, even where the operating
    /// system reports them under its canonical form — FSEvents reports
    /// `/private/var/…` for a watch on `/var/…`.
    pub fn start(root: &Path) -> Result<Watch, Unwatched> {
        let git = git_dir(root).ok_or_else(|| {
            Unwatched(format!(
                "{} is not a git repository, so there is nowhere to put the file that proves the \
                 watcher has caught up",
                root.display()
            ))
        })?;
        if let Some(kind) = mounts::unreliable(root) {
            return Err(Unwatched(format!(
                "{} is on {kind}, which does not report every change to a watcher",
                root.display()
            )));
        }

        let cookies = git.join(COOKIES);
        std::fs::create_dir_all(&cookies).map_err(|e| {
            Unwatched(format!(
                "could not create {} for the watcher ({e})",
                cookies.display()
            ))
        })?;

        let shared = Arc::new(Shared {
            journal: Mutex::new(Journal::default()),
            arrived: Condvar::new(),
        });

        let filter = Filter::new(root, &git, &cookies);
        let handler = {
            let shared = Arc::clone(&shared);
            move |event: notify::Result<notify::Event>| handle(&shared, &filter, event)
        };
        let mut watcher = notify::recommended_watcher(handler)
            .map_err(|e| Unwatched(format!("the file watcher could not start ({e})")))?;

        let mode = if cfg!(target_os = "linux") {
            RecursiveMode::NonRecursive
        } else {
            RecursiveMode::Recursive
        };
        watcher.watch(root, mode).map_err(|e| {
            Unwatched(format!(
                "the file watcher could not watch {} ({e})",
                root.display()
            ))
        })?;
        // Inside the root, a recursive watch already covers it. Outside — a
        // worktree's git directory lives in the main repository — or under a
        // per-directory watch, it needs its own.
        if cfg!(target_os = "linux") || !cookies.starts_with(root) {
            watcher
                .watch(&cookies, RecursiveMode::NonRecursive)
                .map_err(|e| {
                    Unwatched(format!(
                        "the file watcher could not watch {} ({e})",
                        cookies.display()
                    ))
                })?;
        }

        Ok(Watch {
            shared,
            watcher: Mutex::new(watcher),
            cookies,
            next_cookie: AtomicU64::new(0),
        })
    }

    /// Whether [`Watch::watch_dirs`] does anything here — only on Linux, where
    /// every covered directory needs its own watch.
    pub fn per_directory() -> bool {
        cfg!(target_os = "linux")
    }

    /// The journal as it stands, to ask what changed after.
    pub fn mark(&self) -> Mark {
        self.shared.journal().mark()
    }

    /// Why this watcher can no longer see changes, if it can't.
    pub fn broken(&self) -> Option<String> {
        self.shared.journal().broken().map(str::to_string)
    }

    /// Every path touched since `mark`, once the watcher has confirmed it has
    /// caught up, and a mark for next time.
    ///
    /// Waits up to `timeout` for that confirmation. The new mark is taken
    /// after it, so a change made while the caller is still acting on this
    /// answer is in the next one.
    pub fn changes(&self, mark: Mark, timeout: Duration) -> (Since, Mark) {
        let synced = self.sync(timeout);
        let journal = self.shared.journal();
        let now = journal.mark();
        if !synced {
            if let Some(reason) = journal.broken() {
                return (Since::Everything(reason.to_string()), now);
            }
            return (
                Since::Everything(format!(
                    "the file watcher did not confirm it had caught up within {}ms",
                    timeout.as_millis()
                )),
                now,
            );
        }
        (journal.since(mark), now)
    }

    /// Watches each of `dirs` on its own, non-recursively. A no-op except on
    /// Linux; see the crate header.
    ///
    /// Hitting the operating system's watch limit breaks the watcher for good,
    /// with a reason that names the setting to raise. A directory that has
    /// gone by the time it is watched is skipped: its removal is its parent's
    /// event.
    pub fn watch_dirs<'a>(&self, dirs: impl IntoIterator<Item = &'a Path>) {
        if !Self::per_directory() {
            return;
        }
        let mut watcher = match self.watcher.lock() {
            Ok(watcher) => watcher,
            Err(poisoned) => poisoned.into_inner(),
        };
        for dir in dirs {
            let Err(error) = watcher.watch(dir, RecursiveMode::NonRecursive) else {
                continue;
            };
            match &error.kind {
                notify::ErrorKind::MaxFilesWatch => {
                    self.shared.journal().break_with(limit_reached());
                    return;
                }
                notify::ErrorKind::PathNotFound => {}
                notify::ErrorKind::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {}
                _ => {
                    self.shared.journal().lose(format!(
                        "the file watcher could not watch {} ({error})",
                        dir.display()
                    ));
                }
            }
        }
    }

    /// Stops watching each of `dirs`. A no-op except on Linux.
    ///
    /// For a directory that was removed or renamed. inotify drops the watch on
    /// a removed directory itself, but a renamed one keeps its watch under the
    /// old name, and its events would go on arriving under a path that no
    /// longer exists.
    pub fn unwatch_dirs<'a>(&self, dirs: impl IntoIterator<Item = &'a Path>) {
        if !Self::per_directory() {
            return;
        }
        let mut watcher = match self.watcher.lock() {
            Ok(watcher) => watcher,
            Err(poisoned) => poisoned.into_inner(),
        };
        for dir in dirs {
            // Fails for a watch the kernel already dropped, which is the
            // ordinary case for a removed directory.
            let _ = watcher.unwatch(dir);
        }
    }

    /// Writes a cookie and waits until its event arrives, or `timeout` passes.
    fn sync(&self, timeout: Duration) -> bool {
        let name = format!(
            "{}-{}",
            std::process::id(),
            self.next_cookie.fetch_add(1, Ordering::Relaxed)
        );
        let path = self.cookies.join(&name);

        self.shared.journal().expect_cookie(name.clone());
        if std::fs::write(&path, b"").is_err() {
            self.shared.journal().abandon_cookie(&name);
            return false;
        }

        let deadline = Instant::now() + timeout;
        let mut journal = self.shared.journal();
        let seen = loop {
            if journal.take_cookie(&name) {
                break true;
            }
            if journal.broken().is_some() {
                break false;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break false;
            }
            journal = match self.shared.arrived.wait_timeout(journal, left) {
                Ok((journal, _)) => journal,
                Err(poisoned) => poisoned.into_inner().0,
            };
        };
        if !seen {
            journal.abandon_cookie(&name);
        }
        drop(journal);

        let _ = std::fs::remove_file(&path);
        seen
    }
}

fn limit_reached() -> String {
    "the operating system's limit on watched directories was reached, so the file watcher \
     stopped (raise fs.inotify.max_user_watches to watch a workspace this size)"
        .into()
}

/// Which events are recorded, and under what path.
struct Filter {
    root: PathBuf,
    /// The root as the operating system spells it, when that differs.
    canonical: Option<PathBuf>,
    git: PathBuf,
    cookies: PathBuf,
    canonical_cookies: Option<PathBuf>,
}

impl Filter {
    fn new(root: &Path, git: &Path, cookies: &Path) -> Self {
        let canonical = |path: &Path| path.canonicalize().ok().filter(|c| c != path);
        Filter {
            root: root.to_path_buf(),
            canonical: canonical(root),
            git: git.to_path_buf(),
            cookies: cookies.to_path_buf(),
            canonical_cookies: canonical(cookies),
        }
    }

    /// The cookie this event is for, if it is one.
    fn cookie<'a>(&self, path: &'a Path) -> Option<&'a str> {
        let rest = path
            .strip_prefix(&self.cookies)
            .ok()
            .or_else(|| path.strip_prefix(self.canonical_cookies.as_ref()?).ok())?;
        rest.to_str().filter(|name| !name.is_empty())
    }

    /// The path to record, or `None` for one nobody asked about: the git
    /// directory, which nothing covers and every git command writes to.
    fn recorded(&self, path: PathBuf) -> Option<PathBuf> {
        let path = match &self.canonical {
            Some(canonical) => match path.strip_prefix(canonical) {
                Ok(rest) => self.root.join(rest),
                Err(_) => path,
            },
            None => path,
        };
        (!path.starts_with(&self.git)).then_some(path)
    }
}

fn handle(shared: &Shared, filter: &Filter, event: notify::Result<notify::Event>) {
    let mut journal = shared.journal();
    match event {
        Err(error) => match error.kind {
            notify::ErrorKind::MaxFilesWatch => journal.break_with(limit_reached()),
            _ => journal.lose(format!("the file watcher reported an error ({error})")),
        },
        Ok(event) if event.need_rescan() => {
            let why = event.info().unwrap_or("events were dropped");
            journal.lose(format!("the operating system reported that {why}"));
        }
        // Reading a file changes nothing a consumer compares.
        Ok(event) if matches!(event.kind, EventKind::Access(_)) => {}
        Ok(event) => {
            for path in event.paths {
                if let Some(name) = filter.cookie(&path) {
                    journal.cookie_seen(name);
                } else if let Some(path) = filter.recorded(path) {
                    journal.record(path);
                }
            }
        }
    }
    drop(journal);
    shared.arrived.notify_all();
}

/// The directory git keeps its state in, which in a worktree is not `.git`
/// itself: there `.git` is a file naming the real directory.
fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let meta = std::fs::metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }
    let pointer = std::fs::read_to_string(&dot_git).ok()?;
    let target = Path::new(pointer.strip_prefix("gitdir:")?.trim());
    let dir = match target.is_absolute() {
        true => target.to_path_buf(),
        false => root.join(target),
    };
    dir.is_dir().then_some(dir)
}

#[cfg(test)]
mod tests;
