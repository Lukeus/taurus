//! Looking again after a command that ran under a standing grant.
//!
//! The permission gate reads the line, and a line doesn't say what a script
//! does. A grant for `python3` covers `python3 build.py`, and `build.py` can
//! write `.git/hooks/pre-commit`, which then runs the next time anybody commits,
//! with nobody asked. So the files that decide what runs are read before such a
//! command and looked at again when it ends, and any change is put back, with
//! what the command wrote kept somewhere and the change reported.
//!
//! This is repair after the fact, not a sandbox. The command has already run,
//! and anything it wrote could have run before it ended. Only this set of files
//! is covered: see [`listing`] for it.
//!
//! A call somebody approved isn't watched, because they said yes.
//!
//! # Overlapping commands
//!
//! Two commands can't each keep their own picture. If B starts after A wrote a
//! hook, B's picture has the hook in it, and when A's end removes it, B's end
//! would put it back. So a workspace has one [`Watch`] with one baseline (a
//! repository and its linked worktrees count as one, since they share
//! `.git/hooks` and `config`): the
//! first watched command takes it, later ones share it, and it goes when the
//! last one ends. Whichever ends first repairs everything found so far, so the
//! report can name a command that isn't the one that did it, and says so.
//!
//! A call somebody approved can overlap too, and its changes have to stand. While
//! one runs nothing is repaired, and when the last one ends the baseline is read
//! again. So a grant-covered write made during an approved call stands as well.
//! What Taurus itself writes moves the baseline as it happens: see
//! [`written_by_this_process`].
//!
//! A background command is looked at when it exits and not before. Reading the
//! set again while it runs, when nothing else is, would be simpler, and it would
//! absorb whatever that command had written so far, so a dev server's write would
//! never be caught. So the window stays long, and a hook it wrote stays live until
//! it exits.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::permission::PermissionEngine;
use crate::tool::ToolContext;

/// The files read in `.taurus`, as `path_guard::decides_what_runs` lists them.
pub(crate) const TAURUS_FILES: &[&str] = &[
    "permissions.json",
    "hooks.json",
    "mcp.json",
    "providers.json",
    "search.json",
];

/// The most of one file held to put back. A larger one is noticed and reported,
/// and can't be restored.
const MAX_FILE: u64 = 2 * 1024 * 1024;
/// The most held altogether, for a `.taurus/plugins` with a lot in it.
const MAX_TOTAL: u64 = 32 * 1024 * 1024;
/// How many entries a walk looks at. Past this, what's further in isn't covered.
const MAX_ENTRIES: usize = 4096;

/// What says a file might have changed. A ctime and an inode can't be set back
/// the way a modification time can.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stat {
    len: u64,
    mode: u32,
    modified: Option<std::time::SystemTime>,
    changed: i64,
    changed_ns: i64,
    inode: u64,
}

impl Stat {
    fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                len: meta.len(),
                mode: meta.mode(),
                modified: meta.modified().ok(),
                changed: meta.ctime(),
                changed_ns: meta.ctime_nsec(),
                inode: meta.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                len: meta.len(),
                mode: u32::from(meta.permissions().readonly()),
                modified: meta.modified().ok(),
                changed: 0,
                changed_ns: 0,
                inode: 0,
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Node {
    /// `bytes` is `None` for a look that doesn't need them, or one too big to
    /// hold.
    File {
        stat: Stat,
        bytes: Option<Vec<u8>>,
    },
    Link {
        target: PathBuf,
    },
    Dir,
}

#[derive(Clone, Debug)]
struct Entry {
    node: Node,
    /// A git settings file, where only some changes count.
    settings: bool,
    /// A directory the set hangs off. One appearing isn't a change, because
    /// `.taurus` holds plenty that isn't in the set. A symlink in its place is.
    frame: bool,
}

#[derive(Clone, Debug)]
struct Snapshot {
    entries: BTreeMap<PathBuf, Entry>,
    /// The git directories in use when this was taken, every cover's together.
    /// A `.git` that arrives later isn't watched: `git init` and `git clone`
    /// make hooks of their own.
    git_dirs: Vec<PathBuf>,
    /// The folders this reading covers, each with the git directories it was
    /// read with. One for a plain workspace. A repository and its worktrees
    /// share one reading, because they share `.git/hooks` and `config`, and
    /// each one that joins adds its own folder. See [`watch_key`].
    covers: Vec<Cover>,
}

#[derive(Clone, Debug)]
struct Cover {
    places: Places,
    git_dirs: Vec<PathBuf>,
}

impl Cover {
    fn of(places: &Places) -> Self {
        Self {
            places: places.clone(),
            git_dirs: git_dirs(&places.root),
        }
    }

    /// Whether `path` is one this cover's listing would read.
    ///
    /// Exact rather than "under the repository": a worktree's own git
    /// directory sits inside the main one's, under `worktrees/`, and the main
    /// folder's look must not count a worktree's `config.worktree` as a file of
    /// its own that went missing.
    fn holds(&self, path: &Path) -> bool {
        if path.starts_with(self.places.root.join(".taurus")) {
            return true;
        }
        let in_git = self
            .git_dirs
            .iter()
            .any(|dir| path.starts_with(dir) && !path.starts_with(dir.join("worktrees")));
        in_git
            || self
                .places
                .home
                .as_deref()
                .is_some_and(|home| home_files(home).iter().any(|file| file == path))
    }
}

/// Which reading a workspace shares: its repository's, when it has one.
///
/// A linked worktree's `.git` names the main repository's directory as the one
/// it shares hooks and settings with, so a worktree and the folder it came from
/// key to the same place. Two readings over one `.git/hooks` would each judge
/// the other's approved write a change to undo.
fn watch_key(root: &Path) -> PathBuf {
    git_dirs(root).pop().unwrap_or_else(|| root.to_path_buf())
}

/// The files read in the config home.
fn home_files(home: &Path) -> Vec<PathBuf> {
    let mut files = vec![home.join("permissions.json"), home.join("hooks.json")];
    if home.file_name().is_some_and(|name| name == ".taurus") {
        if let Some(user) = home.parent() {
            files.push(user.join(".gitconfig"));
            files.push(user.join(".config/git/config"));
        }
    }
    files
}

/// Where a workspace's watched files are, and where what's undone goes.
#[derive(Clone, Debug)]
struct Places {
    root: PathBuf,
    home: Option<PathBuf>,
    keep_to: Option<PathBuf>,
}

/// The git directories behind a workspace: `.git`, or where a `.git` file says
/// it is, and for a linked worktree the directory it shares hooks and settings
/// with.
fn git_dirs(root: &Path) -> Vec<PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return dot_git
            .canonicalize()
            .map_or_else(|_| Vec::new(), |d| vec![d]);
    }
    let Ok(text) = std::fs::read_to_string(&dot_git) else {
        return Vec::new();
    };
    let Some(named) = text.trim().strip_prefix("gitdir:") else {
        return Vec::new();
    };
    let Ok(dir) = root.join(named.trim()).canonicalize() else {
        return Vec::new();
    };
    let mut dirs = vec![dir.clone()];
    if let Ok(common) = std::fs::read_to_string(dir.join("commondir")) {
        if let Ok(common) = dir.join(common.trim()).canonicalize() {
            dirs.push(common);
        }
    }
    dirs
}

/// Reading one look at the set: stat always, contents only when `hold` says.
struct Reader {
    hold: bool,
    held: u64,
    seen: usize,
    entries: BTreeMap<PathBuf, Entry>,
}

impl Reader {
    fn one(&mut self, path: &Path, settings: bool, frame: bool) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        let kind = meta.file_type();
        let node = if kind.is_symlink() {
            match std::fs::read_link(path) {
                Ok(target) => Node::Link { target },
                Err(_) => return,
            }
        } else if kind.is_dir() {
            Node::Dir
        } else {
            let wanted = self.hold && meta.len() <= MAX_FILE && self.held + meta.len() <= MAX_TOTAL;
            let bytes = if wanted {
                std::fs::read(path).ok()
            } else {
                None
            };
            self.held += bytes.as_ref().map_or(0, |b| b.len() as u64);
            Node::File {
                stat: Stat::of(&meta),
                bytes,
            }
        };
        self.entries.insert(
            path.to_path_buf(),
            Entry {
                node,
                settings,
                frame,
            },
        );
    }

    /// A directory the set includes everything under, not following links.
    fn walk(&mut self, dir: &Path, frame: bool, skip_samples: bool) {
        self.one(dir, false, frame);
        if !matches!(
            self.entries.get(dir),
            Some(Entry {
                node: Node::Dir,
                ..
            })
        ) {
            return;
        }
        let Ok(listed) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in listed.flatten() {
            if self.seen >= MAX_ENTRIES {
                return;
            }
            self.seen += 1;
            let path = entry.path();
            // Nothing runs a `.sample`, and `git init` writes them.
            if skip_samples && path.extension().is_some_and(|e| e == "sample") {
                continue;
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                self.walk(&path, false, skip_samples);
            } else {
                self.one(&path, false, false);
            }
        }
    }

    /// `hooks` and `config` of a git directory, and of its submodules.
    fn git(&mut self, dir: &Path) {
        self.walk(&dir.join("hooks"), true, true);
        self.one(&dir.join("config"), true, false);
        self.one(&dir.join("config.worktree"), true, false);
        if let Ok(modules) = std::fs::read_dir(dir.join("modules")) {
            for module in modules.flatten() {
                if self.seen >= MAX_ENTRIES {
                    return;
                }
                self.seen += 1;
                if module.file_type().is_ok_and(|t| t.is_dir()) {
                    self.git(&module.path());
                }
            }
        }
    }
}

/// The set, as it stands.
///
/// In the workspace: the `.taurus` files that hand over capability,
/// `.taurus/plugins/*`, and each git directory's `hooks/*` and settings. Global:
/// `permissions.json` and `hooks.json` in the config home, and `~/.gitconfig` and
/// `~/.config/git/config` when the config home is `~/.taurus`. A handful of stats,
/// so they're in. Everything else under the home directory isn't.
fn listing(places: &Places, git_dirs: &[PathBuf], hold: bool) -> BTreeMap<PathBuf, Entry> {
    let mut reader = Reader {
        hold,
        held: 0,
        seen: 0,
        entries: BTreeMap::new(),
    };
    let taurus = places.root.join(".taurus");
    reader.one(&taurus, false, true);
    for file in TAURUS_FILES {
        reader.one(&taurus.join(file), false, false);
    }
    reader.walk(&taurus.join("plugins"), true, false);
    for dir in git_dirs {
        reader.git(dir);
    }
    if let Some(home) = &places.home {
        for (i, file) in home_files(home).iter().enumerate() {
            // The first two are Taurus's own; the rest are git's settings.
            reader.one(file, i >= 2, false);
        }
    }
    reader.entries
}

fn snapshot(places: &Places) -> Snapshot {
    let cover = Cover::of(places);
    Snapshot {
        entries: listing(places, &cover.git_dirs, true),
        git_dirs: cover.git_dirs.clone(),
        covers: vec![cover],
    }
}

impl Snapshot {
    /// Reads `places` into this baseline, if it isn't covered already.
    ///
    /// What's already held stays as it was read. The shared part, the
    /// repository's hooks and settings, was read when the first command
    /// started, and reading it again now would absorb whatever has been
    /// written to it since, which is the thing being watched for.
    fn join(&mut self, places: &Places) {
        if self.covers.iter().any(|c| c.places.root == places.root) {
            return;
        }
        let cover = Cover::of(places);
        for (path, entry) in listing(places, &cover.git_dirs, true) {
            self.entries.entry(path).or_insert(entry);
        }
        for dir in &cover.git_dirs {
            if !self.git_dirs.contains(dir) {
                self.git_dirs.push(dir.clone());
            }
        }
        self.covers.push(cover);
    }

    /// Every cover read again, for when the last approved call ends.
    fn reread(&self) -> Self {
        let mut covers = self.covers.iter();
        let first = covers.next().expect("a baseline covers something");
        let mut fresh = snapshot(&first.places);
        for cover in covers {
            fresh.join(&cover.places);
        }
        fresh
    }

    fn cover(&self, places: &Places) -> Option<&Cover> {
        self.covers.iter().find(|c| c.places.root == places.root)
    }
}

/// The settings in a git config file that run something, as `key=value`.
///
/// Only those. `git remote add` and `git branch -u` rewrite the same file under
/// an ordinary `git` grant, and that's nothing to undo. Lenient where git is
/// strict, and a section header it can't read counts as a setting, so a file
/// mangled into hiding one still shows as a change.
fn settings_that_run(text: &str) -> BTreeSet<String> {
    let joined = text.replace("\\\r\n", "").replace("\\\n", "");
    let mut found = BTreeSet::new();
    let mut section = String::new();
    for line in joined.lines() {
        let mut line = line.trim();
        if line.starts_with('[') {
            let Some(end) = line.rfind(']').filter(|end| {
                // `[a] b = "]"` has its header end at the first one outside quotes.
                let first = line
                    .find("\"]")
                    .map_or_else(|| line.find(']'), |q| Some(q + 1));
                first.is_some_and(|first| first <= *end)
            }) else {
                found.insert(format!("unreadable:{line}"));
                continue;
            };
            let first = line
                .find("\"]")
                .map_or_else(|| line.find(']').unwrap_or(end), |q| q + 1);
            let header = &line[1..first];
            section = match header.split_once(char::is_whitespace) {
                Some((name, sub)) => format!("{}.{}", name, sub.trim().trim_matches('"')),
                None => header.to_string(),
            };
            line = line[first + 1..].trim();
        }
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        let (name, value) = line.split_once('=').unwrap_or((line, ""));
        let key = format!("{section}.{}", name.trim());
        if taurus_process::command_line::git_key_runs_something(&key) {
            found.insert(format!("{}={}", key.to_lowercase(), value.trim()));
        }
    }
    found
}

/// One file that isn't what the baseline says.
struct Change {
    path: PathBuf,
    /// What the baseline holds. `None` means it wasn't there.
    was: Option<Entry>,
    /// What's there now, with no contents read.
    now: Option<Entry>,
}

/// Whether a file differs from what's held, by stat first and contents only if
/// the stat moved.
fn differs(was: &Entry, now: &Entry, path: &Path) -> bool {
    match (&was.node, &now.node) {
        (Node::Dir, Node::Dir) => false,
        (Node::Link { target: a }, Node::Link { target: b }) => a != b,
        (Node::File { stat: a, bytes }, Node::File { stat: b, .. }) => {
            if a == b {
                return false;
            }
            if a.mode != b.mode {
                return true;
            }
            let Some(bytes) = bytes else {
                return true;
            };
            a.len != b.len || std::fs::read(path).map_or(true, |current| current != *bytes)
        }
        _ => true,
    }
}

fn text_of(entry: Option<&Entry>, path: &Path, held: bool) -> String {
    let bytes = match entry.map(|e| &e.node) {
        Some(Node::File { bytes: Some(b), .. }) if held => b.clone(),
        Some(Node::File { .. }) if !held => std::fs::read(path).unwrap_or_default(),
        _ => Vec::new(),
    };
    String::from_utf8_lossy(&bytes).into_owned()
}

fn changes(baseline: &Snapshot, places: &Places) -> Vec<Change> {
    let Some(cover) = baseline.cover(places) else {
        return Vec::new();
    };
    let now = listing(places, &cover.git_dirs, false);
    // A repository removed altogether has nothing to put hooks back into.
    let gone = |path: &Path| {
        baseline
            .git_dirs
            .iter()
            .any(|dir| path.starts_with(dir) && !dir.is_dir())
    };
    // Only this folder's part of a shared reading. Another worktree's
    // `.taurus` isn't listed now, and would otherwise read as removed.
    let paths: BTreeSet<&PathBuf> = baseline
        .entries
        .keys()
        .filter(|path| cover.holds(path))
        .chain(now.keys())
        .collect();
    let mut found = Vec::new();
    for path in paths {
        let was = baseline.entries.get(path);
        let current = now.get(path);
        let changed = match (was, current) {
            (Some(a), Some(b)) => {
                if a.settings {
                    !matches!((&a.node, &b.node), (Node::File { stat: x, .. }, Node::File { stat: y, .. }) if x == y)
                        && settings_that_run(&text_of(Some(a), path, true))
                            != settings_that_run(&text_of(Some(b), path, false))
                } else {
                    differs(a, b, path)
                }
            }
            (None, Some(b)) => {
                if b.settings {
                    !settings_that_run(&text_of(Some(b), path, false)).is_empty()
                } else {
                    !(b.frame && matches!(b.node, Node::Dir))
                }
            }
            (Some(a), None) => {
                !gone(path)
                    && if a.settings {
                        !settings_that_run(&text_of(Some(a), path, true)).is_empty()
                    } else {
                        // A frame goes with what's in it, and comes back with it.
                        !a.frame
                    }
            }
            (None, None) => false,
        };
        if changed {
            found.push(Change {
                path: path.clone(),
                was: was.cloned(),
                now: current.cloned(),
            });
        }
    }
    found
}

/// A name for a change, in a sentence.
fn shown(places: &Places, path: &Path) -> String {
    crate::path_guard::display_under(&places.root, path)
}

/// Keeps what's at `path` so undoing it loses nothing, and returns where.
fn keep(places: &Places, into: &Path, change: &Change) -> std::io::Result<()> {
    let relative = change
        .path
        .strip_prefix(&places.root)
        .or_else(|_| change.path.strip_prefix("/"))
        .unwrap_or(&change.path);
    let target = into.join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match change.now.as_ref().map(|e| &e.node) {
        Some(Node::File { .. }) => {
            std::fs::copy(&change.path, &target)?;
            // Kept to be read, not to run, and it may hold anything.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))?;
            }
        }
        Some(Node::Link { target: to }) => {
            let mut name = target.into_os_string();
            name.push(".symlink.txt");
            std::fs::write(name, to.display().to_string())?;
        }
        _ => {}
    }
    Ok(())
}

/// Removes whatever is at `path`, short of a directory with something in it.
fn clear(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn put_back(change: &Change) -> Result<(), String> {
    let path = &change.path;
    let was = change.was.as_ref().map(|e| &e.node);
    match was {
        // A directory that arrived goes once what's in it has, so afterwards.
        None if matches!(change.now.as_ref().map(|e| &e.node), Some(Node::Dir)) => Ok(()),
        None => clear(path).map_err(|e| e.to_string()),
        Some(Node::Dir) => {
            if !matches!(change.now.as_ref().map(|e| &e.node), Some(Node::Dir) | None) {
                clear(path).map_err(|e| e.to_string())?;
            }
            std::fs::create_dir_all(path).map_err(|e| e.to_string())
        }
        Some(Node::Link { target }) => {
            clear(path).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, path).map_err(|e| e.to_string())
            }
            #[cfg(not(unix))]
            {
                let _ = target;
                Err("a symbolic link can't be put back on this platform".to_string())
            }
        }
        Some(Node::File { stat, bytes }) => {
            let Some(bytes) = bytes else {
                return Err("it was too large to hold a copy of".to_string());
            };
            write_back(path, bytes, stat).map_err(|e| e.to_string())
        }
    }
}

fn write_back(path: &Path, bytes: &[u8], stat: &Stat) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    // A link or an empty directory in its place would otherwise take the write.
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
        clear(path)?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(stat.mode & 0o7777))?;
    }
    #[cfg(not(unix))]
    let _ = stat;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

static KEPT: AtomicU64 = AtomicU64::new(0);

/// Where this repair's copies go: beside the session's spilled output when there
/// is one, in a folder that the spill's trimming, which only counts files,
/// leaves alone.
fn keep_dir(places: &Places) -> PathBuf {
    let base = places
        .keep_to
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("taurus-restored"));
    let stamp = crate::delegate::now_ms();
    let seq = KEPT.fetch_add(1, Ordering::Relaxed);
    base.join("restored").join(format!("{stamp}-{seq}"))
}

/// Undoes `found`, and says what happened in words for the model and the person.
fn repair(found: Vec<Change>, places: &Places) -> String {
    let into = keep_dir(places);
    let mut restored = Vec::new();
    let mut failed = Vec::new();
    let mut unkept = Vec::new();
    for change in &found {
        if let Err(e) = keep(places, &into, change) {
            unkept.push(format!("{} ({e})", shown(places, &change.path)));
        }
        let name = shown(places, &change.path);
        match put_back(change) {
            Ok(()) => restored.push(match (&change.was, &change.now) {
                (None, _) => format!("{name} (added, removed again)"),
                (_, None) => format!("{name} (removed, put back)"),
                _ => format!("{name} (changed, put back)"),
            }),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    // Deepest first, so a directory that arrived goes after what's in it.
    for change in found.iter().rev() {
        if change.was.is_none() && matches!(change.now.as_ref().map(|e| &e.node), Some(Node::Dir)) {
            let _ = std::fs::remove_dir(&change.path);
        }
    }

    let mut note = String::from(
        "A command running under a standing grant changed files that decide what runs on this \
         machine, without anybody being asked. It may be this command or another one running \
         beside it.",
    );
    if !restored.is_empty() {
        note.push_str(&format!(" Taurus undid this: {}.", restored.join(", ")));
    }
    if unkept.is_empty() {
        note.push_str(&format!(" What it wrote is kept in {}.", into.display()));
    } else {
        tracing::error!(
            ?unkept,
            "could not keep what a command wrote before undoing it"
        );
        note.push_str(&format!(
            " WARNING: a copy of what it wrote could not be kept, so this is lost: {}.",
            unkept.join(", ")
        ));
    }
    if !failed.is_empty() {
        tracing::error!(?failed, "could not put a protected file back");
        note.push_str(&format!(
            " WARNING: THESE COULD NOT BE PUT BACK AND ARE STILL AS THE COMMAND LEFT THEM, so \
             check them by hand before running anything: {}.",
            failed.join("; ")
        ));
    }
    note.push_str(
        " To let the change stand, run the same line again and approve it when asked, or make \
         the change yourself. Anything these files ran before this point has already run.",
    );
    note
}

#[derive(Default)]
struct State {
    baseline: Option<Snapshot>,
    watching: usize,
    approved: usize,
}

/// One workspace's baseline. See the module note.
#[derive(Default)]
struct Watch {
    state: Mutex<State>,
}

fn watches() -> &'static Mutex<HashMap<PathBuf, Arc<Watch>>> {
    static WATCHES: OnceLock<Mutex<HashMap<PathBuf, Arc<Watch>>>> = OnceLock::new();
    WATCHES.get_or_init(Default::default)
}

fn watch_of(root: &Path) -> Arc<Watch> {
    watches()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(root.to_path_buf())
        .or_default()
        .clone()
}

/// This process just wrote `path` on purpose, so what's there now is the new
/// baseline.
///
/// Call it after anything Taurus itself does to a file in the set: a Settings
/// edit to hooks or MCP, a canvas save, a plugin installed or removed. `path` can
/// be a file or a whole folder, and it can be gone by now. Without this, a
/// long-running command under a grant would be blamed for the write when it
/// ended, and the write would be undone.
///
/// It finds every workspace being watched whose set holds `path`, including one
/// that holds it through the config home, and takes that part of the set again.
/// A path in no reading's set, or no reading at all, makes this nothing but a
/// lock and a compare. It trusts the file as it stands, so a command that wrote
/// the same file a moment before has that write absorbed. An edit from another
/// program isn't this, and still looks like the command did it.
pub fn written_by_this_process(path: &Path) {
    let lexical = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |dir| dir.join(path))
    };
    let path = crate::path_guard::canonical_where_it_can(&lexical).unwrap_or(lexical);
    let all: Vec<Arc<Watch>> = watches()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .cloned()
        .collect();
    for watch in all {
        let mut state = locked(&watch);
        if let Some(baseline) = &mut state.baseline {
            // Listed afresh rather than read alone, so a settings file stays one
            // and a folder comes with everything in it.
            baseline.entries.retain(|held, _| !held.starts_with(&path));
            for cover in &baseline.covers {
                let fresh = listing(&cover.places, &cover.git_dirs, true);
                for (held, entry) in fresh {
                    if held.starts_with(&path) {
                        baseline.entries.insert(held, entry);
                    }
                }
            }
        }
    }
}

fn locked(watch: &Watch) -> std::sync::MutexGuard<'_, State> {
    watch
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Watching,
    Approved,
}

/// One call's place in a workspace's [`Watch`]. Dropped unfinished, as when a
/// turn goes away mid-call, it gives its place up and repairs nothing.
pub struct Ticket {
    watch: Arc<Watch>,
    kind: Kind,
    places: Places,
    /// What to ask about next time, once this has repaired something.
    again: Option<(Arc<PermissionEngine>, String)>,
    /// What this call runs under, when anything. See [`crate::sandbox`].
    fence: Option<Arc<crate::sandbox::Fence>>,
    /// Said the first time a conversation would have had a fence and can't.
    no_fence: Option<String>,
    done: bool,
}

fn places_of(ctx: &ToolContext) -> Option<Places> {
    let root = ctx.canonical_workspace().ok()?;
    let home = ctx.permissions.config_home().canonicalize().ok();
    Some(Places {
        root,
        home,
        keep_to: ctx.command_output.clone(),
    })
}

impl Ticket {
    /// For a call running on a standing grant: reads the set if nothing else is
    /// holding a baseline. `None` for a workspace that can't be resolved, and the
    /// tool is about to say so.
    pub(crate) async fn watching(ctx: &ToolContext, line: Option<String>) -> Option<Self> {
        let places = places_of(ctx)?;
        let watch = watch_of(&watch_key(&places.root));
        let again = line.map(|line| (ctx.permissions.clone(), line));
        let fencing = ctx.fencing;
        let session = ctx.session_id.clone();
        tokio::task::spawn_blocking(move || {
            let (fence, no_fence) = if fencing {
                let dirs = git_dirs(&places.root);
                match crate::sandbox::Fence::ask(&places.root, &dirs, places.home.as_deref()) {
                    crate::sandbox::Asked::Fenced(fence) => (Some(Arc::new(fence)), None),
                    crate::sandbox::Asked::Unavailable(why) => {
                        (None, crate::sandbox::say_once(session.as_deref(), &why))
                    }
                    crate::sandbox::Asked::TurnedOff => (None, None),
                }
            } else {
                (None, None)
            };
            {
                let mut state = locked(&watch);
                state.watching += 1;
                match &mut state.baseline {
                    Some(baseline) => baseline.join(&places),
                    None => state.baseline = Some(snapshot(&places)),
                }
            }
            Self {
                watch,
                kind: Kind::Watching,
                places,
                again,
                fence,
                no_fence,
                done: false,
            }
        })
        .await
        .ok()
    }

    /// For a call somebody approved: while it runs nothing is repaired.
    pub(crate) fn approved(ctx: &ToolContext) -> Option<Self> {
        let places = places_of(ctx)?;
        let watch = watch_of(&watch_key(&places.root));
        locked(&watch).approved += 1;
        Some(Self {
            watch,
            kind: Kind::Approved,
            places,
            again: None,
            fence: None,
            no_fence: None,
            done: false,
        })
    }

    /// What this call runs under. See [`ToolContext::fence`].
    pub(crate) fn fence(&self) -> Option<Arc<crate::sandbox::Fence>> {
        self.fence.clone()
    }

    /// The sentence saying there's no sandbox here, if it's due.
    pub(crate) fn take_no_fence_note(&mut self) -> Option<String> {
        self.no_fence.take()
    }

    /// What to say about what the command printed: the sandbox not starting, or a
    /// write it refused. A refusal gets the line asked about next time, because a
    /// model told only "permission denied" tries again, and an answer of yes is
    /// the one route that works.
    pub(crate) fn about_output(&self, text: &str) -> Option<String> {
        self.fence.as_ref()?;
        if crate::sandbox::Fence::failed_to_start(text) {
            return Some(format!(
                "The write protection this command runs under could not start, so the command \
                 did not run at all. That isn't the command failing. Setting {}=1 for Taurus \
                 turns the protection off.",
                crate::sandbox::OFF_ENV
            ));
        }
        let refused = self.fence.as_ref()?.refusal_in(text)?;
        if let Some((engine, line)) = &self.again {
            engine.ask_next_time(line);
        }
        Some(format!(
            "This ran under a standing grant, so the system refused a write to a protected path: \
             \"{}\". Trying the same thing again fails the same way. To do it, run the same line \
             again, and the person will be asked to approve it, or ask them to make the change \
             themselves.",
            refused.trim()
        ))
    }

    /// The call has ended: looks at the set, and undoes what changed. What comes
    /// back is what to tell the model and the person.
    pub(crate) async fn finish(mut self) -> Option<String> {
        tokio::task::spawn_blocking(move || self.settle(true))
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "the look after a command did not finish");
                Some(format!(
                    "WARNING: Taurus could not check whether this command changed the files that \
                     decide what runs ({e}). Look at .git/hooks and .taurus by hand."
                ))
            })
    }

    fn settle(&mut self, repairing: bool) -> Option<String> {
        self.done = true;
        let mut state = locked(&self.watch);
        match self.kind {
            Kind::Approved => {
                state.approved = state.approved.saturating_sub(1);
                if state.approved == 0 {
                    state.baseline = match (&state.baseline, state.watching > 0) {
                        (Some(baseline), true) => Some(baseline.reread()),
                        (None, true) => Some(snapshot(&self.places)),
                        (_, false) => None,
                    };
                }
                None
            }
            Kind::Watching => {
                let found = match (&state.baseline, repairing && state.approved == 0) {
                    (Some(baseline), true) => changes(baseline, &self.places),
                    _ => Vec::new(),
                };
                state.watching = state.watching.saturating_sub(1);
                if state.watching == 0 && state.approved == 0 {
                    state.baseline = None;
                }
                if found.is_empty() {
                    return None;
                }
                let note = repair(found, &self.places);
                tracing::warn!("{note}");
                if let Some((engine, line)) = &self.again {
                    engine.ask_next_time(line);
                }
                Some(note)
            }
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.done {
            self.settle(false);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::permission::{AllowAll, DenyAll, PermissionPrompt};
    use crate::registry::ToolRegistry;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    struct Bench {
        dir: tempfile::TempDir,
        spill: tempfile::TempDir,
        ctx: ToolContext,
        registry: ToolRegistry,
    }

    /// A workspace with a `.git`, and a grant for the script `./x.sh`. The real
    /// tool and a real script, because a script is what word-reading can't see.
    fn bench(prompt: Box<dyn PermissionPrompt>, grant: bool, git: bool) -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".taurus")).unwrap();
        if grant {
            std::fs::write(
                root.join(".taurus/permissions.json"),
                r#"{"allowed": ["run_command:./x.sh"]}"#,
            )
            .unwrap();
        }
        if git {
            std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
            std::fs::write(root.join(".git/hooks/keep"), "#!/bin/sh\nold\n").unwrap();
            std::fs::set_permissions(
                root.join(".git/hooks/keep"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            std::fs::write(root.join(".git/config"), "[core]\n\tbare = false\n").unwrap();
        }
        let spill = tempfile::tempdir().unwrap();
        let home = root.join("no-home");
        std::fs::create_dir_all(&home).unwrap();
        let engine = PermissionEngine::new(&root, &home, prompt);
        let mut ctx = ToolContext::new(&root, Arc::new(engine), CancellationToken::new());
        // This is the look afterwards on its own, so the writes have to land.
        ctx.fencing = false;
        ctx.command_output = Some(spill.path().to_path_buf());
        Bench {
            dir,
            spill,
            ctx,
            registry: ToolRegistry::with_builtins(),
        }
    }

    impl Bench {
        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().canonicalize().unwrap().join(rel)
        }
        fn script(&self, body: &str) {
            let path = self.path("x.sh");
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        async fn run(&self) -> String {
            self.registry
                .execute(
                    "run_command",
                    serde_json::json!({"command": "./x.sh"}),
                    &self.ctx,
                )
                .await
                .unwrap()
                .to_string()
        }
        fn kept_anywhere(&self, wanted: &str) -> bool {
            fn search(dir: &Path, wanted: &str) -> bool {
                std::fs::read_dir(dir).is_ok_and(|entries| {
                    entries.flatten().any(|e| {
                        let p = e.path();
                        if p.is_dir() {
                            search(&p, wanted)
                        } else {
                            std::fs::read_to_string(&p).is_ok_and(|t| t.contains(wanted))
                        }
                    })
                })
            }
            search(self.spill.path(), wanted)
        }
    }

    #[tokio::test]
    async fn a_script_adding_a_hook_is_undone_and_reported() {
        let b = bench(Box::new(DenyAll), true, true);
        // Through a variable, which no reading of the line would see.
        b.script("D=.git/hooks; echo evil > $D/pre-commit; echo done");
        let text = b.run().await;
        assert!(!b.path(".git/hooks/pre-commit").exists(), "{text}");
        assert!(text.contains("done"), "{text}");
        assert!(text.contains("pre-commit (added, removed again)"), "{text}");
        assert!(text.contains("approve it"), "{text}");
        assert!(b.kept_anywhere("evil"), "what it wrote was lost");
    }

    #[tokio::test]
    async fn a_changed_or_removed_hook_comes_back_as_it_was() {
        let b = bench(Box::new(DenyAll), true, true);
        b.script("echo evil > .git/hooks/keep; chmod 644 .git/hooks/keep");
        let text = b.run().await;
        assert!(text.contains("changed, put back"), "{text}");
        let keep = b.path(".git/hooks/keep");
        assert_eq!(std::fs::read_to_string(&keep).unwrap(), "#!/bin/sh\nold\n");
        assert_eq!(
            std::fs::metadata(&keep).unwrap().permissions().mode() & 0o777,
            0o755
        );

        // A different line, because the one just undone gets asked about.
        b.script("rm -rf .git/hooks");
        let text = b
            .registry
            .execute(
                "run_command",
                serde_json::json!({"command": "./x.sh again"}),
                &b.ctx,
            )
            .await
            .unwrap()
            .to_string();
        assert!(text.contains("removed, put back"), "{text}");
        assert!(keep.exists());
    }

    #[tokio::test]
    async fn a_symlink_in_a_hooks_place_is_undone() {
        let b = bench(Box::new(DenyAll), true, true);
        b.script("rm -rf .git/hooks; ln -s /tmp .git/hooks");
        b.run().await;
        assert!(b.path(".git/hooks").symlink_metadata().unwrap().is_dir());
        assert!(b.path(".git/hooks/keep").exists());
    }

    #[tokio::test]
    async fn the_taurus_files_are_undone_and_the_rest_of_taurus_is_left() {
        let b = bench(Box::new(DenyAll), true, true);
        b.script(
            "echo '{}' > .taurus/hooks.json; mkdir -p .taurus/plugins/x .taurus/notes; \
             echo evil > .taurus/plugins/x/plugin.json; echo mine > .taurus/notes/a.md; \
             echo '{\"allowed\": [\"run_command:rm\"]}' > .taurus/permissions.json",
        );
        let text = b.run().await;
        assert!(!b.path(".taurus/hooks.json").exists(), "{text}");
        assert!(!b.path(".taurus/plugins/x").exists(), "{text}");
        assert_eq!(
            std::fs::read_to_string(b.path(".taurus/notes/a.md")).unwrap(),
            "mine\n"
        );
        assert!(std::fs::read_to_string(b.path(".taurus/permissions.json"))
            .unwrap()
            .contains("./x.sh"));
    }

    #[tokio::test]
    async fn an_ordinary_git_config_change_stands() {
        let b = bench(Box::new(DenyAll), true, true);
        b.script(
            "printf '[remote \"origin\"]\\n\\turl = x\\n[branch \"a\"]\\n\\tremote = o\\n' \
             >> .git/config",
        );
        let text = b.run().await;
        assert!(!text.contains("[taurus]"), "{text}");
        assert!(std::fs::read_to_string(b.path(".git/config"))
            .unwrap()
            .contains("origin"));
    }

    #[tokio::test]
    async fn a_git_setting_that_runs_something_is_undone() {
        for added in [
            "[core]\\n\\thooksPath = z\\n",
            "[CORE] sshCommand = z\\n",
            "[alias]\\n\\tx = !z\\n",
            "[include]\\n\\tpath = z\\n",
        ] {
            let b = bench(Box::new(DenyAll), true, true);
            b.script(&format!("printf '{added}' >> .git/config"));
            let text = b.run().await;
            assert!(
                text.contains("config (changed, put back)"),
                "{added}: {text}"
            );
            assert_eq!(
                std::fs::read_to_string(b.path(".git/config")).unwrap(),
                "[core]\n\tbare = false\n"
            );
        }
    }

    #[tokio::test]
    async fn a_call_somebody_approved_stands() {
        let b = bench(Box::new(AllowAll), false, true);
        b.script("echo mine > .git/hooks/pre-commit");
        let text = b.run().await;
        assert!(!text.contains("[taurus]"), "{text}");
        assert!(b.path(".git/hooks/pre-commit").exists());
    }

    #[tokio::test]
    async fn a_project_with_no_git_is_left_alone() {
        let b = bench(Box::new(DenyAll), true, false);
        // What `git init` does, and a repository arriving isn't a change to one.
        b.script("mkdir -p .git/hooks; echo x > .git/hooks/pre-commit; echo x > .git/config");
        let text = b.run().await;
        assert!(!text.contains("[taurus]"), "{text}");
        assert!(b.path(".git/hooks/pre-commit").exists());
    }

    #[tokio::test]
    async fn a_repository_removed_does_not_get_a_stray_hooks_folder() {
        let b = bench(Box::new(DenyAll), true, true);
        b.script("rm -rf .git");
        let text = b.run().await;
        assert!(!b.path(".git").exists(), "{text}");
    }

    #[tokio::test]
    async fn a_sample_hook_is_not_a_hook() {
        let b = bench(Box::new(DenyAll), true, true);
        b.script("echo x > .git/hooks/pre-commit.sample");
        let text = b.run().await;
        assert!(!text.contains("[taurus]"), "{text}");
        assert!(b.path(".git/hooks/pre-commit.sample").exists());
    }

    #[tokio::test]
    async fn a_worktrees_hooks_are_found_through_its_git_file() {
        let b = bench(Box::new(DenyAll), true, false);
        let real = b.spill.path().join("real");
        std::fs::create_dir_all(real.join("hooks")).unwrap();
        std::fs::write(b.path(".git"), format!("gitdir: {}\n", real.display())).unwrap();
        b.script(&format!("echo evil > {}/hooks/post-merge", real.display()));
        let text = b.run().await;
        assert!(!real.join("hooks/post-merge").exists(), "{text}");
    }

    #[tokio::test]
    async fn the_same_line_is_asked_about_next_time() {
        let b = bench(Box::new(DenyAll), true, true);
        b.script("echo evil > .git/hooks/pre-commit");
        b.run().await;
        let refused = b
            .registry
            .execute(
                "run_command",
                serde_json::json!({"command": "./x.sh"}),
                &b.ctx,
            )
            .await;
        assert!(
            matches!(refused, Err(crate::tool::ToolError::Denied)),
            "the promised way to allow it does not exist: {refused:?}"
        );
        // Once. The grant goes back to covering it.
        b.script("echo fine");
        b.run().await;
    }

    #[tokio::test]
    async fn a_background_command_is_looked_at_when_it_exits() {
        let mut b = bench(Box::new(DenyAll), true, true);
        let jobs = Arc::new(crate::jobs::Jobs::new());
        b.ctx.jobs = Some(jobs.clone());
        b.script("sleep 1; echo evil > .git/hooks/pre-push");
        b.registry
            .execute(
                "run_command",
                serde_json::json!({"command": "./x.sh", "background": true}),
                &b.ctx,
            )
            .await
            .unwrap();
        let report = jobs.check(Some(1), Duration::from_secs(20)).await.unwrap();
        assert!(!b.path(".git/hooks/pre-push").exists(), "{report}");
        assert!(report.contains("pre-push"), "{report}");
        assert!(b.kept_anywhere("evil"));
    }

    #[tokio::test]
    async fn a_background_repair_reaches_the_next_call_when_nobody_checks() {
        let mut b = bench(Box::new(DenyAll), true, true);
        let jobs = Arc::new(crate::jobs::Jobs::new());
        b.ctx.jobs = Some(jobs.clone());
        b.script("echo evil > .git/hooks/pre-push");
        b.registry
            .execute(
                "run_command",
                serde_json::json!({"command": "./x.sh", "background": true}),
                &b.ctx,
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            while b.path(".git/hooks/pre-push").exists() || jobs.running() > 0 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let next = b
            .registry
            .execute("list_dir", serde_json::json!({}), &b.ctx)
            .await
            .unwrap()
            .to_string();
        assert!(next.contains("pre-push"), "{next}");
    }

    #[tokio::test]
    async fn overlapping_commands_share_one_baseline() {
        let b = bench(Box::new(DenyAll), true, true);
        let first = Ticket::watching(&b.ctx, None).await.unwrap();
        // Written while the first runs, and before the second starts.
        std::fs::write(b.path(".git/hooks/evil"), "x").unwrap();
        let second = Ticket::watching(&b.ctx, None).await.unwrap();

        assert!(first.finish().await.is_some());
        assert!(!b.path(".git/hooks/evil").exists());
        // Its own picture would have had the hook in it, and put it back.
        assert!(second.finish().await.is_none());
        assert!(!b.path(".git/hooks/evil").exists());
    }

    #[tokio::test]
    async fn what_an_approved_call_changes_is_not_undone_by_a_neighbour() {
        let b = bench(Box::new(DenyAll), true, true);
        let watching = Ticket::watching(&b.ctx, None).await.unwrap();
        let approved = Ticket::approved(&b.ctx).unwrap();
        std::fs::write(b.path(".git/hooks/mine"), "x").unwrap();
        approved.finish().await;
        assert!(watching.finish().await.is_none());
        assert!(b.path(".git/hooks/mine").exists());
    }

    #[tokio::test]
    async fn a_grant_pressed_during_a_command_is_not_undone() {
        let b = bench(Box::new(crate::permission::AllowAll), true, true);
        let watching = Ticket::watching(&b.ctx, None).await.unwrap();
        b.ctx
            .permissions
            .grant(&["run_command:cargo".to_string()])
            .await;
        assert!(watching.finish().await.is_none());
        assert!(std::fs::read_to_string(b.path(".taurus/permissions.json"))
            .unwrap()
            .contains("cargo"));
    }

    /// A grant saved in one workspace is no part of another's reading. Left in,
    /// the other workspace's look reports that file gone and writes it back.
    #[tokio::test]
    async fn a_grant_in_one_workspace_does_not_reach_anothers_reading() {
        let here = bench(Box::new(DenyAll), true, true);
        let elsewhere = bench(Box::new(AllowAll), true, true);
        let watching = Ticket::watching(&here.ctx, None).await.unwrap();
        elsewhere
            .ctx
            .permissions
            .grant(&["run_command:cargo".to_string()])
            .await;
        let removed = elsewhere.path(".taurus/permissions.json");
        std::fs::remove_file(&removed).unwrap();
        assert!(watching.finish().await.is_none());
        assert!(!removed.exists());
    }

    /// A linked worktree of the bench's repository, as `git worktree add`
    /// lays one out, and a context working in it.
    fn worktree_of(b: &Bench) -> (tempfile::TempDir, PathBuf, ToolContext) {
        let main = b.path("");
        let admin = main.join(".git/worktrees/wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        std::fs::write(admin.join("config.worktree"), "[core]\n").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().canonicalize().unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        std::fs::create_dir_all(wt.join(".taurus")).unwrap();
        std::fs::write(wt.join(".taurus/permissions.json"), "{}").unwrap();
        let ctx = b.ctx.clone().rooted_at(&wt, None);
        (dir, wt, ctx)
    }

    #[tokio::test]
    async fn an_approved_write_in_the_main_folder_is_not_undone_by_its_worktree() {
        let b = bench(Box::new(DenyAll), true, true);
        let (_dir, _wt, worktree) = worktree_of(&b);
        let watching = Ticket::watching(&worktree, None).await.unwrap();
        let approved = Ticket::approved(&b.ctx).unwrap();
        std::fs::write(b.path(".git/hooks/mine"), "x").unwrap();
        approved.finish().await;
        // Its own reading would have had no `mine` in it, and taken it out.
        assert!(watching.finish().await.is_none());
        assert!(b.path(".git/hooks/mine").exists());
    }

    #[tokio::test]
    async fn a_worktree_s_look_still_undoes_a_hook_written_into_the_shared_repository() {
        let b = bench(Box::new(DenyAll), true, true);
        let (_dir, _wt, worktree) = worktree_of(&b);
        let watching = Ticket::watching(&worktree, None).await.unwrap();
        std::fs::write(b.path(".git/hooks/evil"), "x").unwrap();
        assert!(watching.finish().await.is_some());
        assert!(!b.path(".git/hooks/evil").exists());
    }

    #[tokio::test]
    async fn a_shared_reading_judges_each_folder_by_its_own_files() {
        let b = bench(Box::new(DenyAll), true, true);
        let (_dir, wt, worktree) = worktree_of(&b);
        let main = Ticket::watching(&b.ctx, None).await.unwrap();
        let other = Ticket::watching(&worktree, None).await.unwrap();
        // The main folder's look doesn't list the worktree's `.taurus` or its
        // `config.worktree`, and mustn't take them for files it lost.
        assert!(main.finish().await.is_none());
        assert!(wt.join(".taurus/permissions.json").exists());
        assert!(other.finish().await.is_none());
        // And the worktree's own files are still watched.
        let again = Ticket::watching(&worktree, None).await.unwrap();
        std::fs::write(wt.join(".taurus/hooks.json"), "{}").unwrap();
        assert!(again.finish().await.is_some());
        assert!(!wt.join(".taurus/hooks.json").exists());
    }

    #[tokio::test]
    async fn a_ticket_dropped_unfinished_gives_up_its_place() {
        let b = bench(Box::new(DenyAll), true, true);
        drop(Ticket::watching(&b.ctx, None).await.unwrap());
        // No stale baseline left to judge the next command by.
        std::fs::write(b.path(".git/hooks/mine"), "x").unwrap();
        let next = Ticket::watching(&b.ctx, None).await.unwrap();
        assert!(next.finish().await.is_none());
        assert!(b.path(".git/hooks/mine").exists());
    }

    #[tokio::test]
    async fn a_restore_that_fails_says_so_loudly() {
        let b = bench(Box::new(DenyAll), true, true);
        b.script("echo evil > .git/hooks/pre-commit; chmod 555 .git/hooks");
        let text = b.run().await;
        let hooks = b.path(".git/hooks");
        std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A mode change on the folder isn't watched, and the file can't be removed.
        assert!(text.contains("COULD NOT BE PUT BACK"), "{text}");
        assert!(text.contains("pre-commit"), "{text}");
    }

    #[tokio::test]
    async fn the_global_files_are_in_the_set_when_the_home_is_taurus() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().canonicalize().unwrap();
        let home = user.join(".taurus");
        let root = user.join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(root.join(".taurus")).unwrap();
        std::fs::write(
            root.join(".taurus/permissions.json"),
            r#"{"allowed": ["run_command:./x.sh"]}"#,
        )
        .unwrap();
        std::fs::write(user.join(".gitconfig"), "[user]\n\tname = me\n").unwrap();
        let engine = PermissionEngine::new(&root, &home, Box::new(DenyAll));
        let mut ctx = ToolContext::new(&root, Arc::new(engine), CancellationToken::new());
        ctx.fencing = false;
        let registry = ToolRegistry::with_builtins();
        let script = root.join("x.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho '{{}}' > {home}/hooks.json\nprintf '[alias]\\n\\tx = !z\\n' >> {user}/.gitconfig\n",
                home = home.display(),
                user = user.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let text = registry
            .execute(
                "run_command",
                serde_json::json!({"command": "./x.sh"}),
                &ctx,
            )
            .await
            .unwrap()
            .to_string();
        assert!(!home.join("hooks.json").exists(), "{text}");
        assert_eq!(
            std::fs::read_to_string(user.join(".gitconfig")).unwrap(),
            "[user]\n\tname = me\n"
        );
    }

    #[test]
    fn settings_are_read_the_way_git_writes_them() {
        let text = "# c\n[core]\n\tbare = false\n\tHooksPath = z\n[remote \"o\"]\n\turl = x\n\
                    [alias] y = !z\n[diff \"a\"]\n\ttextconv = c\n";
        let found = settings_that_run(text);
        assert!(found.contains("core.hookspath=z"), "{found:?}");
        assert!(found.contains("alias.y=!z"), "{found:?}");
        assert!(found.contains("diff.a.textconv=c"), "{found:?}");
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(!settings_that_run("[broken\nx=y").is_empty());
    }

    #[tokio::test]
    async fn a_file_this_process_wrote_is_the_new_baseline() {
        let b = bench(Box::new(DenyAll), true, true);
        let watching = Ticket::watching(&b.ctx, None).await.unwrap();
        std::fs::write(b.path(".taurus/hooks.json"), "{}").unwrap();
        written_by_this_process(&b.path(".taurus/hooks.json"));
        assert!(watching.finish().await.is_none());
        assert!(b.path(".taurus/hooks.json").exists());
    }

    #[tokio::test]
    async fn a_folder_this_process_installed_or_removed_is_the_new_baseline() {
        let b = bench(Box::new(DenyAll), true, true);
        let plugin = b.path(".taurus/plugins/mine");
        std::fs::create_dir_all(plugin.join("sub")).unwrap();
        std::fs::write(plugin.join("sub/plugin.json"), "{}").unwrap();

        let watching = Ticket::watching(&b.ctx, None).await.unwrap();
        std::fs::write(plugin.join("sub/new.json"), "{}").unwrap();
        written_by_this_process(&plugin);
        // Something else in the set is still judged.
        std::fs::write(b.path(".git/hooks/evil"), "x").unwrap();
        let note = watching.finish().await.unwrap();
        assert!(plugin.join("sub/new.json").exists());
        assert!(note.contains("evil") && !note.contains("mine"), "{note}");

        let watching = Ticket::watching(&b.ctx, None).await.unwrap();
        std::fs::remove_dir_all(&plugin).unwrap();
        written_by_this_process(&plugin);
        assert!(watching.finish().await.is_none());
        assert!(!plugin.exists());
    }

    #[tokio::test]
    async fn a_settings_file_stays_one_after_this_process_writes_it() {
        let b = bench(Box::new(DenyAll), true, true);
        let watching = Ticket::watching(&b.ctx, None).await.unwrap();
        let config = b.path(".git/config");
        std::fs::write(&config, "[core]\n\tbare = true\n").unwrap();
        written_by_this_process(&config);
        std::fs::write(
            &config,
            "[core]\n\tbare = false\n[remote \"o\"]\n\turl = x\n",
        )
        .unwrap();
        // Ordinary, so it stands. Read as a plain file, it would be undone.
        assert!(watching.finish().await.is_none());
        assert!(std::fs::read_to_string(&config).unwrap().contains("remote"));
    }

    #[tokio::test]
    async fn a_write_outside_a_reading_changes_nothing_in_it() {
        let b = bench(Box::new(DenyAll), true, true);
        let watching = Ticket::watching(&b.ctx, None).await.unwrap();
        std::fs::write(b.path(".git/hooks/evil"), "x").unwrap();
        written_by_this_process(&b.path("src/main.rs"));
        written_by_this_process(&b.path("gone/entirely"));
        assert!(watching.finish().await.is_some());
        assert!(!b.path(".git/hooks/evil").exists());
    }

    /// What one look costs, on the folder in TAURUS_MEASURE_DIR. Not a check.
    #[test]
    #[ignore]
    fn measure() {
        let root = PathBuf::from(std::env::var("TAURUS_MEASURE_DIR").unwrap());
        let places = Places {
            root: root.canonicalize().unwrap(),
            home: None,
            keep_to: None,
        };
        let runs = 200u32;
        let t = std::time::Instant::now();
        for _ in 0..runs {
            std::hint::black_box(snapshot(&places));
        }
        let before = t.elapsed() / runs;
        let base = snapshot(&places);
        let t = std::time::Instant::now();
        for _ in 0..runs {
            std::hint::black_box(changes(&base, &places));
        }
        println!(
            "{} entries: before {:?}, after {:?}",
            base.entries.len(),
            before,
            t.elapsed() / runs
        );
    }
}
