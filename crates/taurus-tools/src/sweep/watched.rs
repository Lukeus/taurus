//! A watched workspace's files, kept current between commands so a sweep need
//! not walk.
//!
//! A sweep's walk visits every directory twice per command, and on a
//! repository of 20,000–30,000 files that is most of what a command's undo
//! costs. With a [`Watch`] running, the first walk becomes a *picture* — every
//! file the walk covers, every directory it entered — and each later sweep
//! asks the watcher what moved since the picture was taken, stats only those
//! paths, and hands back the picture as the walk would have.
//!
//! # Placing a change without walking
//!
//! A pass covers the files in every directory the walk enters. So a changed
//! path matters exactly when its parent is one of those directories, and the
//! picture keeps that set to answer it:
//!
//! - an entered directory that is no longer a directory takes everything under
//!   it out of the picture;
//! - an entered directory that changed is re-listed, as a pass lists it;
//! - a file in an entered directory is statted again, or dropped if it is gone;
//! - a new directory in an entered one is entered only if the walk itself would
//!   enter it, which is asked of the walk — one level from its parent — rather
//!   than worked out again here, and then walked;
//! - anything else is in a directory the walk never enters, `target/` or
//!   `node_modules/`, and is not looked at.
//!
//! An edited `.gitignore` or `.ignore` changes which directories the walk
//! enters at all, and that is not worth reasoning about piecemeal: it walks.
//!
//! # When it walks anyway
//!
//! Whenever the watcher says it cannot answer — dropped events, an error, a
//! cookie that did not come back within a second — and whenever a change
//! cannot be placed. Walking is always correct; the picture is only ever a
//! faster way to the same answer.
//!
//! # Checking the watcher
//!
//! Every tool that leans on a file watcher has shipped a release fixing one
//! that missed changes without saying so, and a missed change here is a turn
//! that cannot be undone. So every [`VERIFY_EVERY`]th answer from the picture
//! is checked against a full walk. Anything the walk finds that the picture
//! does not — and that the watcher cannot explain as having moved during the
//! check — switches the watcher off for this workspace, with a warning in the
//! log naming the file it missed. From then on the sweep walks, as it did
//! before watchers existed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use taurus_watch::{Mark, Since, Unwatched, Watch, SYNC_TIMEOUT};

use super::{is_ignore_file, skipped, walk, Stamped, Walked, MAX_FILES, SKIP};

/// How often an answer from the picture is checked against a walk. One
/// command in sixteen pays for a walk it did not need, which on the largest
/// workspaces measured is about 150ms.
const VERIFY_EVERY: usize = 16;

/// Whether a cache is watching its workspace, for a person or a test asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Watching {
    /// Not watching this workspace: no sweep has run in it yet, or the cache
    /// was cleared.
    No,
    /// The watcher is starting. Sweeps walk until it has.
    Starting,
    /// Sweeps are answered from the watcher.
    Live,
    /// The watcher was refused or switched off, and why. Sweeps walk.
    Off(String),
}

/// Everything a cache holds about watching.
#[derive(Default)]
pub(super) struct Slot {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    phase: Phase,
    /// Moved on every start and every clear, so a watcher that finishes
    /// starting after its workspace was left is dropped rather than installed.
    ticket: u64,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Starting {
        root: PathBuf,
    },
    Live {
        root: PathBuf,
        live: Box<Live>,
    },
    Off {
        root: PathBuf,
        reason: String,
    },
}

impl Phase {
    fn root(&self) -> Option<&Path> {
        match self {
            Phase::Idle => None,
            Phase::Starting { root } | Phase::Live { root, .. } | Phase::Off { root, .. } => {
                Some(root)
            }
        }
    }
}

struct Live {
    watch: Watch,
    picture: Option<Picture>,
    /// Directories with a watch of their own. Only ever filled on Linux; see
    /// [`Watch::per_directory`].
    watched: HashSet<PathBuf>,
    /// Answers given from the picture, for [`VERIFY_EVERY`].
    answered: usize,
}

/// The workspace as the watcher last confirmed it.
struct Picture {
    /// Changes after this have not been applied yet.
    mark: Mark,
    dirs: HashSet<PathBuf>,
    files: HashMap<PathBuf, (u64, Option<SystemTime>)>,
}

/// What one look produced: the files, and a reason to stop trusting the
/// watcher if this look found one.
struct Look {
    files: Option<Vec<Stamped>>,
    distrust: Option<String>,
}

impl Look {
    fn trusted(files: Option<Vec<Stamped>>) -> Self {
        Look {
            files,
            distrust: None,
        }
    }
}

impl Slot {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }

    /// Every file a pass covers in `root`, as [`super::sweepable`] would
    /// return them — from the watcher when it can answer, from a walk when it
    /// cannot.
    ///
    /// Blocking, like the walk it stands in for.
    pub(super) fn files(&self, root: &Path) -> Option<Vec<Stamped>> {
        let mut inner = self.inner();

        if inner.phase.root() != Some(root) {
            inner.ticket += 1;
            inner.phase = match refusal() {
                Some(reason) => Phase::Off {
                    root: root.to_path_buf(),
                    reason,
                },
                None => {
                    start(Arc::clone(&self.inner), root.to_path_buf(), inner.ticket);
                    Phase::Starting {
                        root: root.to_path_buf(),
                    }
                }
            };
            drop(inner);
            return super::sweepable(root);
        }

        let Phase::Live { live, .. } = &mut inner.phase else {
            drop(inner);
            return super::sweepable(root);
        };

        let look = look(live, root);
        let distrust = look
            .distrust
            .or_else(|| live.watch.broken().map(|reason| reason.to_string()));
        if let Some(reason) = distrust {
            tracing::warn!(workspace = %root.display(), "{reason}");
            inner.phase = Phase::Off {
                root: root.to_path_buf(),
                reason,
            };
        }
        look.files
    }

    /// Stops watching, for a workspace being left.
    pub(super) fn clear(&self) {
        let mut inner = self.inner();
        inner.ticket += 1;
        inner.phase = Phase::Idle;
    }

    pub(super) fn watching(&self, root: &Path) -> Watching {
        let inner = self.inner();
        if inner.phase.root() != Some(root) {
            return Watching::No;
        }
        match &inner.phase {
            Phase::Idle => Watching::No,
            Phase::Starting { .. } => Watching::Starting,
            Phase::Live { .. } => Watching::Live,
            Phase::Off { reason, .. } => Watching::Off(reason.clone()),
        }
    }
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    // Everything held is either rebuilt by the next walk or a watcher that is
    // still watching, so a panic partway through leaves nothing unsafe to read.
    match inner.lock() {
        Ok(inner) => inner,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Why this platform does not watch for the sweep at all.
fn refusal() -> Option<String> {
    cfg!(windows).then(|| {
        "the sweep does not use a file watcher on Windows yet: the watcher Taurus uses can lose \
         changes there without saying so"
            .to_string()
    })
}

/// Starts a watcher on a thread of its own and installs it if its workspace is
/// still the one being swept when it is ready.
///
/// Off the command's path because starting is not always quick: inotify adds
/// watches one directory at a time, and on macOS starting an FSEvents stream
/// was measured taking hundreds of milliseconds to seconds for a binary run
/// out of a large `target/` directory. A command never waits for it.
fn start(inner: Arc<Mutex<Inner>>, root: PathBuf, ticket: u64) {
    let spawned = std::thread::Builder::new()
        .name("taurus-watch-start".into())
        .spawn({
            let inner = Arc::clone(&inner);
            let root = root.clone();
            move || {
                let started = Watch::start(&root);
                let mut inner = lock(&inner);
                if inner.ticket != ticket {
                    return;
                }
                inner.phase = match started {
                    Ok(watch) => Phase::Live {
                        root,
                        live: Box::new(Live {
                            watch,
                            picture: None,
                            watched: HashSet::new(),
                            answered: 0,
                        }),
                    },
                    Err(Unwatched(reason)) => {
                        tracing::debug!(workspace = %root.display(), "not watched: {reason}");
                        Phase::Off { root, reason }
                    }
                };
            }
        });
    if let Err(error) = spawned {
        let mut inner = lock(&inner);
        if inner.ticket == ticket {
            inner.phase = Phase::Off {
                root,
                reason: format!("the file watcher's thread could not start ({error})"),
            };
        }
    }
}

/// One look at the workspace through the watcher.
fn look(live: &mut Live, root: &Path) -> Look {
    let Some(mut picture) = live.picture.take() else {
        return Look::trusted(take_picture(live, root));
    };

    let (since, mark) = live.watch.changes(picture.mark, SYNC_TIMEOUT);
    let paths = match since {
        Since::Paths(paths) => paths,
        Since::Everything(reason) => {
            tracing::debug!(workspace = %root.display(), "walking: {reason}");
            return Look::trusted(take_picture(live, root));
        }
    };
    picture.mark = mark;
    if let Err(reason) = apply(&mut picture, &paths, root, live) {
        tracing::debug!(workspace = %root.display(), "walking: {reason}");
        return Look::trusted(take_picture(live, root));
    }
    if picture.files.len() > MAX_FILES {
        return Look::trusted(None);
    }

    live.answered += 1;
    if live.answered.is_multiple_of(VERIFY_EVERY) {
        if let Some(look) = verify(&picture, live, root) {
            return look;
        }
    }

    let files = stamped(&picture.files);
    live.picture = Some(picture);
    Look::trusted(Some(files))
}

fn stamped(files: &HashMap<PathBuf, (u64, Option<SystemTime>)>) -> Vec<Stamped> {
    files
        .iter()
        .map(|(path, &(len, modified))| (path.clone(), len, modified))
        .collect()
}

/// Walks, and keeps what it found as the picture.
///
/// On Linux every directory needs its own watch before its changes can be
/// trusted, so a walk that finds directories without one watches them and
/// walks again: a file created in one between the walk listing it and its
/// watch starting would otherwise be missing from the picture for good.
fn take_picture(live: &mut Live, root: &Path) -> Option<Vec<Stamped>> {
    for _ in 0..3 {
        let mark = live.watch.mark();
        let walked = walk(root)?;
        let unwatched = unwatched(live, &walked.dirs);
        if unwatched.is_empty() {
            forget_watches(live, &walked.dirs);
            let files = walked.files.clone();
            live.picture = Some(picture(mark, walked));
            return Some(files);
        }
        watch(live, unwatched);
    }
    // Directories kept appearing faster than they could be watched. The last
    // walk is still a correct answer; it is just not one to build on.
    super::sweepable(root)
}

fn picture(mark: Mark, walked: Walked) -> Picture {
    Picture {
        mark,
        dirs: walked.dirs.into_iter().collect(),
        files: walked
            .files
            .into_iter()
            .map(|(path, len, modified)| (path, (len, modified)))
            .collect(),
    }
}

/// The directories among `dirs` that need a watch and do not have one.
fn unwatched(live: &Live, dirs: &[PathBuf]) -> Vec<PathBuf> {
    if !Watch::per_directory() {
        return Vec::new();
    }
    dirs.iter()
        .filter(|dir| !live.watched.contains(*dir))
        .cloned()
        .collect()
}

fn watch(live: &mut Live, dirs: Vec<PathBuf>) {
    live.watch.watch_dirs(dirs.iter().map(PathBuf::as_path));
    live.watched.extend(dirs);
}

/// Drops the watches on directories a walk no longer enters.
fn forget_watches(live: &mut Live, dirs: &[PathBuf]) {
    if live.watched.is_empty() {
        return;
    }
    let entered: HashSet<&PathBuf> = dirs.iter().collect();
    let stale: Vec<PathBuf> = live
        .watched
        .iter()
        .filter(|dir| !entered.contains(dir))
        .cloned()
        .collect();
    live.watch.unwatch_dirs(stale.iter().map(PathBuf::as_path));
    for dir in stale {
        live.watched.remove(&dir);
    }
}

/// Brings the picture up to date with the paths the watcher named, or says why
/// it cannot and the caller should walk.
fn apply(
    picture: &mut Picture,
    paths: &[PathBuf],
    root: &Path,
    live: &mut Live,
) -> Result<(), String> {
    // Directories that went first, so that a directory renamed over another
    // does not have its new contents removed along with the old.
    let gone: Vec<&PathBuf> = paths
        .iter()
        .filter(|path| picture.dirs.contains(*path) && !is_dir(path))
        .collect();
    for dir in gone {
        if dir == root {
            return Err("the workspace folder itself moved".into());
        }
        remove_under(picture, dir, live);
    }

    for path in paths {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if relative.components().any(|part| skipped(part.as_os_str())) {
            continue;
        }
        if picture.dirs.contains(path) {
            relist(picture, path);
            continue;
        }
        let Some(parent) = path.parent() else {
            continue;
        };
        if !picture.dirs.contains(parent) {
            continue;
        }
        if is_ignore_file(path) {
            return Err(format!(
                "{} changed, which can change which directories a walk enters",
                relative.display()
            ));
        }

        picture.files.remove(path);
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() => {
                picture
                    .files
                    .insert(path.clone(), (meta.len(), meta.modified().ok()));
            }
            Ok(meta) if meta.is_dir() => enter(picture, path, parent, live)?,
            // Gone, or a symlink, which a pass does not cover either.
            _ => {}
        }
    }
    Ok(())
}

fn is_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// Takes a directory and everything under it out of the picture.
fn remove_under(picture: &mut Picture, dir: &Path, live: &mut Live) {
    let removed: Vec<PathBuf> = picture
        .dirs
        .iter()
        .filter(|entered| entered.starts_with(dir))
        .cloned()
        .collect();
    for entered in &removed {
        picture.dirs.remove(entered);
    }
    picture.files.retain(|path, _| !path.starts_with(dir));

    // A renamed directory keeps its inotify watch under its old name; see
    // `Watch::unwatch_dirs`.
    if Watch::per_directory() {
        live.watch
            .unwatch_dirs(removed.iter().map(PathBuf::as_path));
        for entered in removed {
            live.watched.remove(&entered);
        }
    }
}

/// Lists an entered directory again, as a pass lists it.
fn relist(picture: &mut Picture, dir: &Path) {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return;
    };
    let mut present: HashSet<PathBuf> = HashSet::new();
    for entry in listing.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) || skipped(&entry.file_name()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let path = entry.path();
        picture
            .files
            .insert(path.clone(), (meta.len(), meta.modified().ok()));
        present.insert(path);
    }
    picture
        .files
        .retain(|path, _| path.parent() != Some(dir) || present.contains(path));
}

/// A new directory inside an entered one: entered too if the walk would enter
/// it, and then walked.
fn enter(picture: &mut Picture, dir: &Path, parent: &Path, live: &mut Live) -> Result<(), String> {
    // Asked of the walk rather than worked out here, so the answer comes from
    // the same ignore rules applied the same way.
    let entered = crate::builtin::search::walk_builder(parent, SKIP)
        .max_depth(Some(1))
        .build()
        .flatten()
        .any(|entry| entry.path() == dir);
    if !entered {
        return Ok(());
    }

    for _ in 0..3 {
        let Some(walked) = walk(dir) else {
            return Err(format!(
                "{} holds more than {MAX_FILES} files",
                dir.display()
            ));
        };
        let unwatched = unwatched(live, &walked.dirs);
        if unwatched.is_empty() {
            picture.dirs.extend(walked.dirs);
            picture.files.extend(
                walked
                    .files
                    .into_iter()
                    .map(|(path, len, modified)| (path, (len, modified))),
            );
            return Ok(());
        }
        watch(live, unwatched);
    }
    Err(format!(
        "directories kept appearing under {} faster than they could be watched",
        dir.display()
    ))
}

/// Checks the picture against a walk. `None` when they agree or the check could
/// not be made; otherwise the walk's answer and the reason to stop trusting
/// the watcher.
fn verify(picture: &Picture, live: &Live, root: &Path) -> Option<Look> {
    let walked = walk(root)?;
    // Whatever moved after the picture's mark may differ between the two
    // without either being wrong — a background build writing while this
    // looks. Only a difference the watcher has no event for is a miss.
    let (since, _) = live.watch.changes(picture.mark, SYNC_TIMEOUT);
    let Since::Paths(moving) = since else {
        return None;
    };
    let moving: HashSet<PathBuf> = moving.into_iter().collect();
    let settled = |path: &Path| !path.ancestors().any(|a| moving.contains(a));

    let actual: HashMap<&PathBuf, (u64, Option<SystemTime>)> = walked
        .files
        .iter()
        .map(|(path, len, modified)| (path, (*len, *modified)))
        .collect();
    let mut missed: Vec<&Path> = actual
        .iter()
        .filter(|(path, stamp)| picture.files.get(**path) != Some(stamp))
        .map(|(path, _)| path.as_path())
        .chain(
            picture
                .files
                .keys()
                .filter(|path| !actual.contains_key(path))
                .map(PathBuf::as_path),
        )
        .filter(|path| settled(path))
        .collect();
    if missed.is_empty() {
        return None;
    }
    missed.sort();

    let first = missed[0]
        .strip_prefix(root)
        .unwrap_or(missed[0])
        .display()
        .to_string();
    let reason = format!(
        "the file watcher missed a change to {first}{}, so this workspace is walked before and \
         after every command from now on",
        match missed.len() {
            1 => String::new(),
            n => format!(" and {} other files", n - 1),
        }
    );
    Some(Look {
        files: Some(walked.files),
        distrust: Some(reason),
    })
}

#[cfg(test)]
mod tests;
