//! A git worktree for each delegate that writes, so several can run at once.
//!
//! See [`taurus_tools::isolation`] for the rules any provider keeps. This is
//! the provider, and the way it keeps them comes from what went wrong in tools
//! that did this before.
//!
//! # Where a delegate starts
//!
//! From the workspace as the parent sees it, not from a branch. A snapshot
//! commit is made of the working tree, with uncommitted and untracked files in
//! it, through an index of its own, so the person's staged work stays exactly
//! as staged. Starting from `HEAD` would hide the parent's own edits from
//! earlier in the turn, and starting from the remote's default branch hides
//! unpushed work, which is the complaint that moved one tool's default three
//! times. The commit is held under `refs/taurus/delegates/<run>/base`, so git
//! keeps it until Taurus lets it go.
//!
//! # No branches
//!
//! The worktree is checked out detached. A branch per delegate would be a name
//! to clash with another session's, and the same branch can't be checked out
//! twice, which is two failures for nothing: the work is a commit either way.
//!
//! # Nothing the repository would run
//!
//! `core.hooksPath` points at an empty folder for every git command here, so a
//! `post-checkout` hook doesn't run because a delegate started. A repository
//! with filters (Git LFS among them) or submodules isn't copied at all: a
//! filter is a program the repository names, and git calls worktree support
//! for submodules incomplete. The delegate runs alone in the main folder
//! instead, and its card says why.
//!
//! # Closing
//!
//! What the delegate changed is committed under `refs/taurus/delegates/<run>/result`
//! before anything is removed, and the folder is then removed with plain
//! `git worktree remove`: no `--force`, nothing deleted by hand, no links
//! followed. The work is kept by the commit, so the folder can go as soon as the
//! delegate stops, and a folder that won't go costs disk but no work. Nothing
//! here writes to the person's folder. Applying is somebody's choice, made
//! elsewhere.
//!
//! # Where they live
//!
//! In `.taurus-worktrees` beside the config home, never inside it and never
//! inside the workspace. Inside the workspace, the parent's searches and
//! command sweeps would find a second copy of every file. Inside the config
//! home, the sandbox reads the folder as a plugin being written, which loosens
//! what a command may write there.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use taurus_tools::isolation::{Closed, Isolation, Kept, Opened, Requester};

/// How long one git command here may take. A snapshot of a big working tree
/// writes every untracked file into the object store, so this is the size of
/// the repository's problem, not a read's.
const DEADLINE: Duration = Duration::from_secs(300);

/// What a commit made here is signed as. Never the person: these commits
/// aren't theirs, and nobody pushes them.
const IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Taurus"),
    ("GIT_AUTHOR_EMAIL", "taurus@localhost"),
    ("GIT_COMMITTER_NAME", "Taurus"),
    ("GIT_COMMITTER_EMAIL", "taurus@localhost"),
];

/// The file listing ignored files to copy into each worktree. The name and
/// meaning are Claude Code's, so a project set up for one works in the other.
pub const INCLUDE_FILE: &str = ".worktreeinclude";

/// How many copied files a note names before it counts the rest.
const NAMED: usize = 5;

/// Where one run stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Its folder exists, and a delegate may be working in it.
    Running,
    /// It stopped and changed something, kept as a commit.
    Kept,
    /// It stopped and changed nothing. Nothing is held.
    Empty,
    /// What it changed was applied to the workspace.
    Applied,
    /// Somebody discarded what it changed.
    Discarded,
}

/// What is known about one run, kept beside its folder.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub run: String,
    /// The workspace that asked.
    pub workspace: PathBuf,
    /// The repository's top, which is where git is asked from.
    pub top: PathBuf,
    /// Where the workspace is under `top`, as git writes it: empty, or ending
    /// in `/`.
    pub prefix: String,
    /// The worktree.
    pub path: PathBuf,
    /// The full id of the commit it started from.
    pub base: String,
    /// The full id of the commit holding what it changed.
    pub result: Option<String>,
    pub agent: String,
    pub session: Option<String>,
    pub call: Option<String>,
    /// Unix seconds.
    pub created: u64,
    pub state: State,
    /// Why the folder is still there after closing, when it is.
    #[serde(default)]
    pub left_behind: Option<String>,
}

impl Record {
    fn short_base(&self) -> String {
        short(&self.base)
    }
}

/// The worktrees on this machine, and what Taurus knows of each.
pub struct Worktrees {
    home: PathBuf,
    /// A lock held on each run this process has open. The operating system
    /// lets go when the process ends, however it ends, so a lock nobody holds
    /// is a run whose owner is gone. See [`Self::tidy`].
    held: std::sync::Mutex<std::collections::HashMap<String, std::fs::File>>,
}

impl Worktrees {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            held: std::sync::Mutex::default(),
        }
    }

    fn lock_path(&self, run: &str) -> PathBuf {
        self.home.join("runs").join(format!("{run}.lock"))
    }

    /// Takes the run's lock for this process, for as long as it's open.
    fn hold(&self, run: &str) -> Result<(), String> {
        let path = self.lock_path(run);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        file.lock()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(run.to_string(), file);
        Ok(())
    }

    fn release(&self, run: &str) {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run);
        let _ = std::fs::remove_file(self.lock_path(run));
    }

    /// Whether some process, this one or another, still has `run` open.
    fn owned(&self, run: &str) -> bool {
        if self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(run)
        {
            return true;
        }
        let Ok(file) = std::fs::OpenOptions::new()
            .write(true)
            .open(self.lock_path(run))
        else {
            return false;
        };
        match file.try_lock() {
            Ok(()) => false,
            Err(std::fs::TryLockError::WouldBlock) => true,
            Err(std::fs::TryLockError::Error(_)) => false,
        }
    }

    /// What a crash left behind, put right. Run when the host starts.
    ///
    /// A run still marked running whose owner is gone is closed the usual way,
    /// so what its delegate had changed is kept rather than lost with the
    /// process. A folder that couldn't be removed before is tried again. Only
    /// runs in this folder's records are touched: a worktree Taurus didn't
    /// make is never Taurus's to remove. Returns what it couldn't do.
    pub async fn tidy(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let Ok(listed) = std::fs::read_dir(self.home.join("runs")) else {
            return problems;
        };
        let runs: Vec<String> = listed
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".json").map(str::to_owned)
            })
            .filter(|run| valid_run(run))
            .collect();
        for run in runs {
            let Ok(mut record) = self.record(&run) else {
                continue;
            };
            if record.state == State::Running {
                if self.owned(&run) {
                    continue;
                }
                if let Err(e) = self.close_run(&run).await {
                    problems.push(format!("Run {run}: {e}"));
                }
                continue;
            }
            if record.left_behind.is_some() {
                record.left_behind = self.remove_folder(&record).await;
                if let Some(why) = &record.left_behind {
                    problems.push(why.clone());
                }
                let _ = self.save(&record);
            }
        }
        problems
    }

    /// `.taurus-worktrees` beside the config home. See the module note for why
    /// not inside it.
    pub fn default_home() -> PathBuf {
        crate::config::home_dir().with_file_name(".taurus-worktrees")
    }

    fn base_ref(run: &str) -> String {
        format!("refs/taurus/delegates/{run}/base")
    }

    fn result_ref(run: &str) -> String {
        format!("refs/taurus/delegates/{run}/result")
    }

    /// Where a run's record is. Runs are found by id alone, so the record isn't
    /// filed under the workspace: closing one needs nothing but its name.
    fn record_path(&self, run: &str) -> PathBuf {
        self.home.join("runs").join(format!("{run}.json"))
    }

    fn hooks_off(&self) -> PathBuf {
        self.home.join("no-hooks")
    }

    pub fn record(&self, run: &str) -> Result<Record, String> {
        if !valid_run(run) {
            return Err(format!("'{run}' isn't the name of a delegate's run."));
        }
        let path = self.record_path(run);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("No record of run {run} at {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| {
            format!(
                "The record of run {run} at {} can't be read: {e}",
                path.display()
            )
        })
    }

    fn save(&self, record: &Record) -> Result<(), String> {
        let path = self.record_path(&record.run);
        let dir = path.parent().expect("a record has a folder");
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let text = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut file, text.as_bytes()).map_err(|e| e.to_string())?;
        file.persist(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(())
    }

    /// Runs git in `dir` with hooks pointed at nothing, and returns stdout.
    async fn git(
        &self,
        dir: &Path,
        args: &[&str],
        env: &[(&str, &OsStr)],
    ) -> Result<String, String> {
        self.git_with(dir, args, env, None).await
    }

    async fn git_with(
        &self,
        dir: &Path,
        args: &[&str],
        env: &[(&str, &OsStr)],
        input: Option<Vec<u8>>,
    ) -> Result<String, String> {
        let hooks = format!("core.hooksPath={}", self.hooks_off().display());
        let mut full: Vec<&str> = vec!["-c", &hooks];
        full.extend_from_slice(args);
        let output = crate::git::launch_full(dir, &full, input, env, DEADLINE).await?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("git {} failed", args.first().unwrap_or(&"command"))
        } else {
            stderr
        })
    }

    /// Commits `tree` on `parent`, as Taurus and never signed: signing would
    /// ask a key agent for a passphrase nobody is there to type.
    async fn commit(
        &self,
        dir: &Path,
        tree: &str,
        parent: Option<&str>,
        message: &str,
    ) -> Result<String, String> {
        let mut args = vec!["commit-tree", "--no-gpg-sign", tree];
        if let Some(parent) = parent {
            args.extend(["-p", parent]);
        }
        args.extend(["-m", message]);
        let env: Vec<(&str, &OsStr)> = IDENTITY.iter().map(|(k, v)| (*k, OsStr::new(*v))).collect();
        Ok(self.git(dir, &args, &env).await?.trim().to_string())
    }

    /// Why `top` can't be copied, if it can't. See the module note.
    async fn refusal(&self, top: &Path) -> Option<String> {
        if top.join(".gitmodules").exists() {
            return Some(
                "This repository has submodules, and git only partly supports them in a \
                 worktree, so the delegate works in your folder instead, by itself."
                    .into(),
            );
        }
        let listed = self
            .git(
                top,
                &[
                    "ls-files",
                    "-z",
                    "--",
                    ".gitattributes",
                    ":(glob)**/.gitattributes",
                ],
                &[],
            )
            .await
            .ok()?;
        for name in listed.split('\0').filter(|n| !n.is_empty()) {
            let Ok(text) = std::fs::read_to_string(top.join(name)) else {
                continue;
            };
            let filter = text
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .flat_map(str::split_whitespace)
                .find_map(|word| word.strip_prefix("filter="));
            match filter {
                Some("lfs") => {
                    return Some(
                        "This repository uses Git LFS, and a worktree made without running \
                         its filter would hold pointer files where the real ones should be, so \
                         the delegate works in your folder instead, by itself."
                            .into(),
                    )
                }
                Some(other) => {
                    return Some(format!(
                        "This repository's {name} runs files through a git filter ({other}), \
                         which is a program the repository names, so the delegate works in your \
                         folder instead, by itself."
                    ))
                }
                None => {}
            }
        }
        None
    }

    /// A commit of `top`'s working tree as it is, made through an index of its
    /// own so the person's staging is untouched.
    async fn snapshot(&self, top: &Path, scratch: &Path, agent: &str) -> Result<String, String> {
        let index = scratch.with_extension("index");
        let env = [("GIT_INDEX_FILE", index.as_os_str())];
        let head = self
            .git(top, &["rev-parse", "--verify", "-q", "HEAD"], &[])
            .await
            .ok()
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty());
        let made = async {
            match &head {
                Some(_) => self.git(top, &["read-tree", "HEAD"], &env).await?,
                None => self.git(top, &["read-tree", "--empty"], &env).await?,
            };
            self.git(top, &["add", "-A", "--", ":/"], &env).await?;
            let tree = self.git(top, &["write-tree"], &env).await?;
            self.commit(
                top,
                tree.trim(),
                head.as_deref(),
                &format!("Taurus: the workspace as {agent} found it"),
            )
            .await
        }
        .await;
        let _ = std::fs::remove_file(&index);
        made
    }

    /// Copies what [`INCLUDE_FILE`] lists from `top` into `into`: only files git
    /// ignores (anything else is in the snapshot already), never over a file
    /// that's there, never through a link. Returns what it copied.
    async fn copy_included(&self, top: &Path, into: &Path) -> Result<Vec<String>, String> {
        let list = top.join(INCLUDE_FILE);
        if !list.is_file() {
            return Ok(Vec::new());
        }
        let exclude_from = format!("--exclude-from={}", list.display());
        let named = self
            .git(
                top,
                &["ls-files", "-z", "--others", "--ignored", &exclude_from],
                &[],
            )
            .await?;
        let candidates: Vec<&str> = named.split('\0').filter(|n| !n.is_empty()).collect();
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let input = candidates.join("\0").into_bytes();
        // Exits 1 when none of them is ignored, which is an answer.
        let ignored = self
            .git_with(top, &["check-ignore", "-z", "--stdin"], &[], Some(input))
            .await
            .unwrap_or_default();
        let ignored: Vec<String> = ignored
            .split('\0')
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect();
        let (top, into) = (top.to_path_buf(), into.to_path_buf());
        tokio::task::spawn_blocking(move || {
            let mut copied = Vec::new();
            for name in ignored {
                let relative = Path::new(&name);
                if relative
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
                {
                    continue;
                }
                let from = top.join(relative);
                let to = into.join(relative);
                let is_file =
                    std::fs::symlink_metadata(&from).is_ok_and(|m| m.file_type().is_file());
                if !is_file || std::fs::symlink_metadata(&to).is_ok() {
                    continue;
                }
                if let Some(parent) = to.parent() {
                    if std::fs::create_dir_all(parent).is_err() {
                        continue;
                    }
                }
                if std::fs::copy(&from, &to).is_ok() {
                    copied.push(name);
                }
            }
            copied
        })
        .await
        .map_err(|e| e.to_string())
    }

    async fn open_run(&self, workspace: &Path, who: &Requester) -> Result<Opened, String> {
        let top = self
            .git(workspace, &["rev-parse", "--show-toplevel"], &[])
            .await
            .map_err(|_| {
                "This folder isn't in a git repository, so there's no worktree to give the \
                 delegate, and it works in your folder instead, by itself."
                    .to_string()
            })?;
        let top = PathBuf::from(top.trim());
        let prefix = self
            .git(workspace, &["rev-parse", "--show-prefix"], &[])
            .await?
            .trim()
            .to_string();
        if let Some(why) = self.refusal(&top).await {
            return Err(why);
        }

        std::fs::create_dir_all(self.hooks_off())
            .map_err(|e| format!("{}: {e}", self.hooks_off().display()))?;
        let run = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let key = crate::sessions::workspace_key(&top);
        let parent = self.home.join(&key);
        std::fs::create_dir_all(&parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let path = parent.join(&run);

        let base = self.snapshot(&top, &path, &who.agent).await?;
        self.git(&top, &["update-ref", &Self::base_ref(&run), &base], &[])
            .await?;
        let mut record = Record {
            run: run.clone(),
            workspace: workspace.to_path_buf(),
            top: top.clone(),
            prefix: prefix.clone(),
            path: path.clone(),
            base: base.clone(),
            result: None,
            agent: who.agent.clone(),
            session: who.session.clone(),
            call: who.call.clone(),
            created: taurus_tools::now_ms() / 1000,
            state: State::Running,
            left_behind: None,
        };
        // Before the folder exists, so a crash while it's made leaves something
        // that says whose it is, and a lock that says its owner has gone.
        self.save(&record)?;
        self.hold(&run)?;

        let reason = format!("Taurus delegate {run}");
        let path_text = path.to_string_lossy().into_owned();
        if let Err(e) = self
            .git(
                &top,
                &[
                    "worktree", "add", "--detach", "--lock", "--reason", &reason, &path_text, &base,
                ],
                &[],
            )
            .await
        {
            let _ = self
                .git(&top, &["update-ref", "-d", &Self::base_ref(&run)], &[])
                .await;
            record.state = State::Empty;
            let _ = self.save(&record);
            self.release(&run);
            return Err(format!(
                "git couldn't make a worktree for the delegate: {e}"
            ));
        }

        // Checked rather than trusted: a delegate told it's in a copy while it
        // is really somewhere else is the one outcome worse than no copy.
        let root = path.join(&prefix);
        let verified = async {
            let canonical = path.canonicalize().map_err(|e| e.to_string())?;
            let seen = self
                .git(&root, &["rev-parse", "--show-toplevel"], &[])
                .await?;
            let seen = PathBuf::from(seen.trim())
                .canonicalize()
                .map_err(|e| e.to_string())?;
            if seen != canonical {
                return Err(format!(
                    "git places the worktree at {} rather than {}",
                    seen.display(),
                    canonical.display()
                ));
            }
            Ok::<_, String>(root.canonicalize().map_err(|e| e.to_string())?)
        }
        .await;
        let root = match verified {
            Ok(root) => root,
            Err(e) => {
                let _ = self.close(&run).await;
                return Err(format!("The delegate's worktree couldn't be checked: {e}"));
            }
        };

        let copied = self.copy_included(&top, &path).await.unwrap_or_default();
        let note = (!copied.is_empty()).then(|| {
            let mut named: Vec<&str> = copied.iter().take(NAMED).map(String::as_str).collect();
            let more = copied.len().saturating_sub(NAMED);
            let more = if more > 0 {
                format!(", and {more} more")
            } else {
                String::new()
            };
            named.sort_unstable();
            format!(
                "Copied in because {INCLUDE_FILE} lists them: {}{more}.",
                named.join(", ")
            )
        });

        Ok(Opened {
            run,
            root,
            base: short(&base),
            note,
        })
    }

    async fn close_run(&self, run: &str) -> Result<Closed, String> {
        let mut record = self.record(run)?;
        if record.state != State::Running {
            return Ok(Closed {
                kept: self.kept_of(&record).await,
                left_behind: record.left_behind.clone(),
            });
        }

        if record.path.is_dir() {
            let dir = record.path.clone();
            self.git(&dir, &["add", "-A", "--", ":/"], &[]).await?;
            let tree = self.git(&dir, &["write-tree"], &[]).await?;
            let tree = tree.trim();
            let base_tree = format!("{}^{{tree}}", record.base);
            let was = self.git(&dir, &["rev-parse", &base_tree], &[]).await?;
            if tree == was.trim() {
                record.state = State::Empty;
            } else {
                let message = format!("Taurus: what {} changed", record.agent);
                let result = self
                    .commit(&dir, tree, Some(&record.base), &message)
                    .await?;
                self.git(&dir, &["update-ref", &Self::result_ref(run), &result], &[])
                    .await?;
                // So the folder is clean, and plain `worktree remove` takes it.
                self.git(&dir, &["update-ref", "--no-deref", "HEAD", &result], &[])
                    .await?;
                record.result = Some(result);
                record.state = State::Kept;
            }
        } else {
            // Nothing to read the work from. It never got as far as a folder,
            // or something outside Taurus removed it.
            record.state = State::Empty;
        }
        if record.state == State::Empty {
            let _ = self
                .git(
                    &record.top,
                    &["update-ref", "-d", &Self::base_ref(run)],
                    &[],
                )
                .await;
        }
        record.left_behind = self.remove_folder(&record).await;
        self.save(&record)?;
        self.release(run);
        Ok(Closed {
            kept: self.kept_of(&record).await,
            left_behind: record.left_behind.clone(),
        })
    }

    /// Removes a run's folder the only way git calls safe. `Some` says why it
    /// couldn't.
    async fn remove_folder(&self, record: &Record) -> Option<String> {
        if !record.path.exists() {
            let _ = self.git(&record.top, &["worktree", "prune"], &[]).await;
            return None;
        }
        let path = record.path.to_string_lossy().into_owned();
        let _ = self
            .git(&record.top, &["worktree", "unlock", &path], &[])
            .await;
        self.git(&record.top, &["worktree", "remove", &path], &[])
            .await
            .err()
            .map(|e| {
                format!(
                    "The worktree at {path} couldn't be removed ({e}). What the delegate \
                     changed is kept either way, and Taurus tries again when it next starts."
                )
            })
    }

    /// What a stopped run kept, read from its two commits.
    async fn kept_of(&self, record: &Record) -> Option<Kept> {
        let result = record.result.as_deref()?;
        if !matches!(record.state, State::Kept | State::Applied) {
            return None;
        }
        let listed = self
            .git(
                &record.top,
                &[
                    "diff",
                    "--name-only",
                    "-z",
                    "--no-renames",
                    &record.base,
                    result,
                ],
                &[],
            )
            .await
            .ok()?;
        let mut files = Vec::new();
        let mut outside = Vec::new();
        for name in listed.split('\0').filter(|n| !n.is_empty()) {
            match name.strip_prefix(record.prefix.as_str()) {
                Some(inside) => files.push(inside.to_string()),
                None => outside.push(name.to_string()),
            }
        }
        files.sort();
        outside.sort();
        Some(Kept {
            run: record.run.clone(),
            base: record.short_base(),
            files,
            outside,
        })
    }
}

#[async_trait::async_trait]
impl Isolation for Worktrees {
    async fn open(&self, workspace: &Path, who: &Requester) -> Result<Opened, String> {
        self.open_run(workspace, who).await
    }

    async fn close(&self, run: &str) -> Result<Closed, String> {
        self.close_run(run).await
    }
}

/// A run's id is twelve hex characters, so it can be a file name and a ref
/// part and nothing else.
fn valid_run(run: &str) -> bool {
    run.len() == 12 && run.bytes().all(|b| b.is_ascii_hexdigit())
}

fn short(id: &str) -> String {
    id.chars().take(10).collect()
}

#[cfg(test)]
mod tests;
