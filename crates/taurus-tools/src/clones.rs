//! Pre-images held as copy-on-write clones, for files a checkpoint could not
//! otherwise keep.
//!
//! A pre-image is normally the file's text, inline in the checkpoint log. That
//! leaves out two kinds of file: one past the size a sweep holds in memory,
//! and one that isn't text at all — an image, a database, a build artifact
//! someone checked in. Both used to be recorded as unrestorable, so `rm` of a
//! PNG could not be undone.
//!
//! APFS, Btrfs and XFS can *clone* a file: a new file that shares every block
//! with the original until one of them is written. Making one is a single
//! system call whatever the size, and it costs no space until the original
//! changes. So those files are cloned instead of read:
//!
//! - **Before a command**, the sweep clones each one into a *stage* beside the
//!   checkpoint logs. A clone of a file the command leaves alone costs only its
//!   metadata, and [`crate::SweepCache`] keeps it for the next command, so each
//!   version of a file is cloned once.
//! - **When the command changed it**, the staged clone is cloned again into the
//!   conversation's blobs, `<session>.blobs/` beside its log, and the log names
//!   that copy. Only now does it hold real space — the blocks the command
//!   replaced.
//! - **A rewind** clones the blob back beside the file and renames it over, so
//!   the file is never seen half-written.
//!
//! # Where it does not
//!
//! A clone only works within one filesystem. The checkpoint logs live under
//! `~/.taurus/`, so a workspace on another volume — an external drive, a
//! separate partition — keeps the old behavior: those files are reported, not
//! restored. So does a filesystem that cannot clone (ext4, NTFS), which is
//! found by trying once per store and remembering.
//!
//! Windows does not clone yet. ReFS can, but the library Taurus clones through
//! calls its Windows path untested, and it has an open bug that leaves a clone
//! zero-filled after its first megabyte — the same failure that made pnpm turn
//! cloning off on Windows.
//!
//! Only a real clone is ever made. `std::fs::copy` and `copy_file_range` fall
//! back to copying every byte without saying so, which would turn a 4 GB
//! video into 4 GB written before every command.
//!
//! # What it may hold
//!
//! A blob holds real space once the original changes, and holds it until the
//! conversation is deleted. So the blobs in one workspace's checkpoint folder
//! are capped — 20 GB by default, `checkpoint_clone_limit_gb` in the global
//! settings — and a file that would pass the cap is recorded as unrestorable,
//! with a reason naming the setting. Each blob is counted at its full length,
//! though it may still share some blocks with the file it came from: the cap
//! is a ceiling on what the folder could hold, not a measurement of it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::checkpoint::State;

/// What a workspace's blobs may hold, by default.
pub const DEFAULT_LIMIT_BYTES: u64 = 20 * GB;

const GB: u64 = 1024 * 1024 * 1024;

/// The stage's folder inside a checkpoint store.
const STAGE: &str = "stage";

/// The suffix on a conversation's blob folder. A session id never contains a
/// `.`, so this can't meet a log's own name.
const BLOBS: &str = "blobs";

/// Clones files from one workspace into one checkpoint store.
pub(crate) struct Cloner {
    store: PathBuf,
    stage: PathBuf,
    limit: u64,
}

impl Cloner {
    /// A cloner for `workspace` into `store`, or `None` where cloning can't
    /// work: another volume, a filesystem that can't clone, or Windows.
    ///
    /// Asked of the filesystem once per store and volume, and remembered.
    pub(crate) fn new(store: &Path, workspace: &Path, limit: u64) -> Option<Arc<Cloner>> {
        if cfg!(windows) {
            return None;
        }
        let stage = store.join(STAGE);
        std::fs::create_dir_all(&stage).ok()?;
        let volume = same_volume(&stage, workspace)?;
        if !supported(&stage, volume) {
            return None;
        }
        Some(Arc::new(Cloner {
            store: store.to_path_buf(),
            stage,
            limit,
        }))
    }

    /// Clones `file` into the stage, as it stands now.
    pub(crate) fn stage(&self, file: &Path, len: u64) -> Option<Staged> {
        let path = self.stage.join(stage_name());
        reflink_copy::reflink(file, &path).ok()?;
        Some(Staged { path, len })
    }

    /// Clones `from` into `session`'s blobs and says how the log should record
    /// it: as the blob, or as unrestorable when the workspace's blobs would
    /// pass the limit or the clone failed.
    ///
    /// `from` is a staged clone, or the workspace file itself for a tool that
    /// names a file just before writing it.
    pub(crate) fn keep(&self, from: &Path, len: u64, session: &str) -> State {
        let held = self.held();
        if held.saturating_add(len) > self.limit {
            return State::Opaque {
                reason: format!(
                    "was {} when it was recorded, and keeping it would take this workspace's \
                     kept copies past {} (checkpoint_clone_limit_gb in settings.json)",
                    size(len),
                    size(self.limit)
                ),
            };
        }

        let folder = format!("{session}.{BLOBS}");
        let dir = self.store.join(&folder);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return State::Opaque {
                reason: format!("could not be kept when it was recorded ({e})"),
            };
        }
        let name = uuid::Uuid::new_v4().simple().to_string();
        match reflink_copy::reflink(from, dir.join(&name)) {
            Ok(()) => State::Clone {
                blob: format!("{folder}/{name}"),
                len,
            },
            Err(e) => State::Opaque {
                reason: format!("could not be kept when it was recorded ({e})"),
            },
        }
    }

    /// Everything this store's blobs hold, at full length.
    fn held(&self) -> u64 {
        let Ok(entries) = std::fs::read_dir(&self.store) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|entry| is_blob_folder(&entry.path()))
            .filter_map(|entry| std::fs::read_dir(entry.path()).ok())
            .flat_map(|blobs| blobs.flatten())
            .filter_map(|blob| blob.metadata().ok())
            .map(|meta| meta.len())
            .sum()
    }
}

fn is_blob_folder(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == BLOBS) && path.is_dir()
}

/// A clone in the stage, removed when the last holder lets go of it.
pub(crate) struct Staged {
    path: PathBuf,
    len: u64,
}

impl Staged {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }
}

impl Drop for Staged {
    /// Handed to a thread of its own rather than removed here. A cache that
    /// lets go of a large workspace's clones lets go of tens of thousands at
    /// once — measured at about 40µs each on APFS, so most of a second — and
    /// the last holder is often a task on the async runtime.
    fn drop(&mut self) {
        remove_later(std::mem::take(&mut self.path));
    }
}

enum Removal {
    File(PathBuf),
    /// Answers once everything sent before it is gone.
    #[cfg_attr(not(test), allow(dead_code))]
    Flush(std::sync::mpsc::SyncSender<()>),
}

/// The removal thread's queue, started the first time a clone is let go of.
/// `None` if the thread couldn't start, and removal happens in place.
fn queue() -> Option<&'static Mutex<std::sync::mpsc::Sender<Removal>>> {
    static QUEUE: OnceLock<Option<Mutex<std::sync::mpsc::Sender<Removal>>>> = OnceLock::new();
    QUEUE
        .get_or_init(|| {
            let (send, receive) = std::sync::mpsc::channel::<Removal>();
            std::thread::Builder::new()
                .name("taurus-clone-removal".into())
                .spawn(move || {
                    for removal in receive {
                        match removal {
                            Removal::File(path) => {
                                let _ = std::fs::remove_file(path);
                            }
                            Removal::Flush(done) => {
                                let _ = done.send(());
                            }
                        }
                    }
                })
                .ok()
                .map(|_| Mutex::new(send))
        })
        .as_ref()
}

fn remove_later(path: PathBuf) {
    let sent = queue().is_some_and(|queue| {
        queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .send(Removal::File(path.clone()))
            .is_ok()
    });
    if !sent {
        let _ = std::fs::remove_file(path);
    }
}

/// Waits until every staged clone already let go of has been removed. For
/// tests, which look at the stage right after letting go.
#[cfg(test)]
pub(crate) fn settle() {
    let Some(queue) = queue() else { return };
    let (done, wait) = std::sync::mpsc::sync_channel(1);
    let sent = queue
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .send(Removal::Flush(done))
        .is_ok();
    if sent {
        let _ = wait.recv_timeout(std::time::Duration::from_secs(10));
    }
}

impl std::fmt::Debug for Staged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Staged")
            .field("path", &self.path)
            .field("len", &self.len)
            .finish()
    }
}

/// Where a blob named in a log lives, or `None` for a name that tries to leave
/// the store. The log is a file on disk like any other.
pub(crate) fn blob_path(store: &Path, blob: &str) -> Option<PathBuf> {
    let (folder, name) = blob.split_once('/')?;
    let session = folder.strip_suffix(&format!(".{BLOBS}"))?;
    let plain = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    (plain(session) && plain(name)).then(|| store.join(folder).join(name))
}

/// Puts a blob back at `target`: cloned beside it, then renamed over it, so
/// nothing ever sees the file half-written. Copies where a clone can't be
/// made — the workspace has since moved to another volume — because a rewind
/// is worth a copy.
pub(crate) fn restore(blob: &Path, target: &Path) -> std::io::Result<()> {
    if !blob.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "its kept copy is gone from {}",
                blob.parent().unwrap_or(blob).display()
            ),
        ));
    }
    let parent = target.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = parent.join(format!(".{name}.taurus-{}", stage_name()));
    if reflink_copy::reflink(blob, &temp).is_err() {
        std::fs::copy(blob, &temp)?;
    }
    std::fs::rename(&temp, target).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// Clones a blob from one conversation's folder into another's, for a fork
/// that copies the turns naming it. `None` when it can't, and the fork records
/// the file as unrestorable rather than pointing at a blob it doesn't own.
pub(crate) fn copy_blob(store: &Path, blob: &str, to_session: &str) -> Option<String> {
    let from = blob_path(store, blob)?;
    let folder = format!("{to_session}.{BLOBS}");
    let dir = store.join(&folder);
    std::fs::create_dir_all(&dir).ok()?;
    let name = uuid::Uuid::new_v4().simple().to_string();
    let to = dir.join(&name);
    if reflink_copy::reflink(&from, &to).is_err() {
        std::fs::copy(&from, &to).ok()?;
    }
    Some(format!("{folder}/{name}"))
}

/// Removes a conversation's blobs, for a conversation being deleted.
pub(crate) fn forget(store: &Path, session: &str) -> std::io::Result<()> {
    match std::fs::remove_dir_all(store.join(format!("{session}.{BLOBS}"))) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// A unique name in the stage. The process id comes first so a stage left by
/// a process that died can be told apart from one still in use; see
/// [`prune_stage`].
fn stage_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        uuid::Uuid::new_v4().simple()
    )
}

/// The volume `stage` is on, when `workspace` is on it too.
#[cfg(unix)]
fn same_volume(stage: &Path, workspace: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let here = std::fs::metadata(stage).ok()?.dev();
    (std::fs::metadata(workspace).ok()?.dev() == here).then_some(here)
}

#[cfg(not(unix))]
fn same_volume(_stage: &Path, _workspace: &Path) -> Option<u64> {
    None
}

/// Whether the filesystem `stage` is on can clone, asked once per stage and
/// volume. The first time a stage is used in this process it is also cleared
/// of clones left by processes that have since exited.
fn supported(stage: &Path, volume: u64) -> bool {
    static KNOWN: OnceLock<Mutex<HashMap<(PathBuf, u64), bool>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    let key = (stage.to_path_buf(), volume);
    if let Some(&answer) = known.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return answer;
    }

    prune_stage(stage);
    let probe = stage.join(format!("probe-{}", stage_name()));
    let clone = stage.join(format!("probe-{}", stage_name()));
    let answer = std::fs::write(&probe, b"taurus").is_ok()
        && reflink_copy::reflink(&probe, &clone).is_ok()
        && std::fs::read(&clone).is_ok_and(|bytes| bytes == b"taurus");
    let _ = std::fs::remove_file(&probe);
    let _ = std::fs::remove_file(&clone);

    known
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, answer);
    answer
}

/// Removes staged clones whose process is gone. One that exited without
/// dropping them — a crash, a kill — would otherwise leave them for good, and
/// a clone of a file that has since changed holds real space.
fn prune_stage(stage: &Path) {
    let Ok(entries) = std::fs::read_dir(stage) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("probe-").or(Some(n)))
            .and_then(|n| n.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != std::process::id() && !alive(pid) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    // Permission denied means it exists and belongs to someone else.
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(e) => e == rustix::io::Errno::PERM,
    }
}

#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    true
}

/// A size as a person reads it.
pub(crate) fn size(n: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else {
        format!("{n} bytes")
    }
}

#[cfg(test)]
mod tests;
