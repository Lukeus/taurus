//! What git says about the workspace, and one way to write to it.
//!
//! Taurus has had a checkpoint log since before it had a UI, and it is a better
//! undo than git for the thing it does: it records what a file held before a
//! turn touched it, whether or not that file was tracked, staged, or even
//! inside a repository. What it cannot do is *keep* anything. A checkpoint lives
//! in the config home, is keyed by session id, and disappears with the
//! conversation. The turn that got it right is worth more than that.
//!
//! So this is the other half, and only the other half. It asks git two
//! questions the UI needs answered — is there a repository, and what branch is
//! it on — and offers one write: commit the files a turn changed. Nothing here
//! is a tool the model can call. The model reaches git the way it always has,
//! through `run_command`, where the permission engine sees it and
//! [`taurus_tools::sweep`] records what it did; adding a second path would mean
//! two things to keep in step and two places for the sweep's caveat about `.git`
//! to be reasoned about.
//!
//! # Shelling out
//!
//! There is no git library in the tree, and this does not add one. The
//! questions asked here are the ones `git` answers in a single word, the write
//! is a command a user could have typed, and a linked library would mean
//! matching the installed git's behaviour on worktrees, submodules,
//! `core.hooksPath`, signing, and `commit.gpgsign` — all of which the binary
//! already gets right, because it is the one the user's own aliases and hooks
//! are written against.
//!
//! What that costs is a process per question. The two reads are cheap enough to
//! run on a drawer opening; nothing here polls.
//!
//! # Committing only what the turn touched
//!
//! `git commit -- <paths>` is `--only`: it commits the working-tree state of
//! exactly those paths and leaves the index alone. That is the property that
//! makes this safe to offer beside a running conversation — someone who has
//! staged unrelated work still has it staged afterwards, and a turn that touched
//! four files commits four files even if forty others are dirty.
//!
//! The paths are asked about before they are used, because three different
//! things make a path uncommittable and they need three different sentences.
//! See [`Repo::commit`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use ts_rs::TS;

/// Longest commit subject built from a turn's prompt.
///
/// Git itself imposes nothing, but a subject past this stops being readable in
/// `git log --oneline`, which is the view a commit made from a drawer is most
/// likely to be read in. The user can type a longer one; this only bounds the
/// suggestion.
const SUBJECT_MAX_CHARS: usize = 72;

/// Where the workspace stands with git, as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepoStatus {
    /// False for a workspace that is not in a repository, or a machine with no
    /// git. Everything else here is `None` when this is false.
    pub repository: bool,
    /// The checked-out branch. `None` on a detached HEAD, where there is no
    /// branch to name and pretending otherwise would mislabel a session.
    #[ts(optional)]
    pub branch: Option<String>,
    /// Short hash of `HEAD`. `None` in a repository with no commits yet, which
    /// is a real state a fresh `git init` is in.
    #[ts(optional)]
    pub head: Option<String>,
    /// Why git could not be asked, when that is the reason `repository` is
    /// false. `None` means the question was asked and answered: this is simply
    /// not a repository.
    #[ts(optional)]
    pub unavailable: Option<String>,
}

impl RepoStatus {
    /// The answer for a workspace git does not cover.
    fn none() -> Self {
        Self {
            repository: false,
            branch: None,
            head: None,
            unavailable: None,
        }
    }
}

/// A file the commit left out, and why.
///
/// Reported rather than silently dropped: a commit that quietly covered three
/// of a turn's four files is the failure this whole surface exists to prevent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Skipped {
    pub path: String,
    /// A complete phrase following the file's name.
    pub reason: String,
}

/// A commit that happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Commit {
    /// Short hash, as `git log --oneline` would show it.
    pub sha: String,
    pub subject: String,
    /// Workspace-relative paths that went in, sorted.
    pub files: Vec<String>,
    /// Paths that did not, each with its reason.
    pub skipped: Vec<Skipped>,
}

/// A git repository containing the workspace.
///
/// Holds the workspace directory rather than the repository root: every command
/// runs with the workspace as its working directory, and every path handed in
/// or out is relative to the workspace, so a Taurus opened on a subdirectory of
/// a repository behaves like a terminal opened in the same place.
pub struct Repo {
    workspace: PathBuf,
}

impl Repo {
    /// The repository covering `workspace`, if there is one.
    ///
    /// `Ok(None)` is "this is not a repository", which is an ordinary state and
    /// not a problem to report. `Err` is "git could not be asked", which is —
    /// the two look identical from the outside and mean opposite things about
    /// whether committing could ever work here.
    pub async fn discover(workspace: &Path) -> Result<Option<Self>, String> {
        // `--is-inside-work-tree` rather than `--show-toplevel`: it answers
        // false inside a bare repository and inside `.git` itself, where there
        // is a toplevel but nothing a commit of working-tree paths could mean.
        //
        // Read through `launch` rather than `run` because the ordinary answer
        // arrives as a failure: outside a repository this exits 128 with
        // `fatal: not a git repository`, which `run` would report as git being
        // unavailable. Any non-zero exit means the same thing here — there is
        // no work tree at this path — so the status is what is inspected, and
        // no error string is matched against.
        let output = launch(workspace, &["rev-parse", "--is-inside-work-tree"]).await?;
        let inside =
            output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true";
        Ok(inside.then(|| Self {
            workspace: workspace.to_path_buf(),
        }))
    }

    /// Where the workspace stands, in one round trip per question.
    ///
    /// Never fails: this is drawn beside a conversation, and a status line that
    /// could throw would have to be handled at every call site to say the one
    /// thing it already knows how to say.
    pub async fn status(workspace: &Path) -> RepoStatus {
        match Self::discover(workspace).await {
            Ok(Some(repo)) => RepoStatus {
                repository: true,
                branch: repo.branch().await,
                head: repo.head().await,
                unavailable: None,
            },
            Ok(None) => RepoStatus::none(),
            Err(reason) => RepoStatus {
                unavailable: Some(reason),
                ..RepoStatus::none()
            },
        }
    }

    /// The checked-out branch, or `None` on a detached HEAD.
    ///
    /// `symbolic-ref` rather than `rev-parse --abbrev-ref HEAD`, which answers
    /// the literal string `HEAD` when detached — a value that would be recorded
    /// as a branch name and then shown to someone as though they were on a
    /// branch called HEAD.
    pub async fn branch(&self) -> Option<String> {
        let branch = run(&self.workspace, &["symbolic-ref", "--short", "HEAD"])
            .await
            .ok()?;
        let branch = branch.trim();
        (!branch.is_empty()).then(|| branch.to_string())
    }

    /// Short hash of `HEAD`, or `None` before the first commit.
    pub async fn head(&self) -> Option<String> {
        let head = run(&self.workspace, &["rev-parse", "--short", "HEAD"])
            .await
            .ok()?;
        let head = head.trim();
        (!head.is_empty()).then(|| head.to_string())
    }

    /// Commits exactly `paths`, leaving the index and every other path alone.
    ///
    /// The paths are workspace-relative, which is what a checkpoint records.
    ///
    /// A path can fail to be committable three ways, and each gets its own
    /// sentence rather than one shrug covering all of them:
    ///
    /// - git is ignoring it, so the turn's work is real but deliberately
    ///   untracked — `.env` is the case, and it is the one where a silent skip
    ///   would be worst.
    /// - it matches `HEAD` already, because a later turn put it back or the
    ///   user did. Nothing to commit is not a failure.
    /// - it was created and removed inside the same turn, so git never saw it
    ///   and there is no deletion to record.
    ///
    /// When nothing survives that filter the commit is refused rather than
    /// producing an empty one, and the refusal carries every reason it
    /// collected — "nothing to commit" alone would send someone to look for a
    /// bug that is not there.
    pub async fn commit(&self, paths: &[String], message: &str) -> Result<Commit, String> {
        let message = message.trim();
        if message.is_empty() {
            return Err("A commit needs a message.".into());
        }
        if paths.is_empty() {
            return Err("That turn changed no files, so there is nothing to commit.".into());
        }

        let dirty = self.dirty(paths).await?;
        let mut skipped = Vec::new();
        let mut staging = Vec::new();

        for path in paths {
            if dirty.contains(path) {
                staging.push(path.clone());
            } else if self.is_ignored(path).await {
                skipped.push(Skipped {
                    path: path.clone(),
                    reason: "is ignored by git, so it is not in the repository to commit".into(),
                });
            } else if !self.workspace.join(path).exists() {
                // Not dirty, not ignored, and not there. A *tracked* file that
                // was deleted shows up in `status` as a deletion, so the only
                // way to land here is a file that was created and removed
                // inside the same turn — git never saw it, and there is no
                // deletion to record. Saying "already matches the last commit"
                // about a path that was never in a commit sends someone to
                // `git show` to find something that is not there.
                skipped.push(Skipped {
                    path: path.clone(),
                    reason: "was created and removed within the turn, so git never saw it".into(),
                });
            } else {
                skipped.push(Skipped {
                    path: path.clone(),
                    reason: "already matches the last commit".into(),
                });
            }
        }

        if staging.is_empty() {
            let reasons: Vec<String> = skipped
                .iter()
                .map(|s| format!("{} {}", s.path, s.reason))
                .collect();
            return Err(format!(
                "Nothing to commit from that turn: {}.",
                reasons.join("; ")
            ));
        }
        staging.sort();

        // `add` covers a deletion as well as a change, so a turn that removed a
        // tracked file commits the removal rather than skipping it.
        let mut add = vec!["add", "--"];
        add.extend(staging.iter().map(String::as_str));
        run(&self.workspace, &add).await?;

        // `-- <paths>` implies `--only`. Someone's unrelated staged work is
        // still staged when this returns.
        let mut commit = vec!["commit", "--message", message, "--"];
        commit.extend(staging.iter().map(String::as_str));
        run(&self.workspace, &commit).await?;

        Ok(Commit {
            sha: self.head().await.unwrap_or_default(),
            subject: subject_of(message),
            files: staging,
            skipped,
        })
    }

    /// Which of `paths` git sees as changed against `HEAD` or untracked.
    ///
    /// One call rather than one per file, and it answers the question that
    /// actually matters — an ignored path is simply absent from the output,
    /// which is why the caller has to ask about those separately.
    async fn dirty(&self, paths: &[String]) -> Result<BTreeSet<String>, String> {
        let mut args = vec!["status", "--porcelain", "-z", "--"];
        args.extend(paths.iter().map(String::as_str));
        let output = run(&self.workspace, &args).await?;

        Ok(output
            .split('\0')
            .filter(|entry| entry.len() > 3)
            // `XY <path>`: two status columns, a space, then the path. Rename
            // entries carry a second NUL-separated path, which cannot appear
            // here — a rename needs both halves in the pathspec, and even then
            // the entry names the destination, which is the path we asked about.
            .map(|entry| entry[3..].to_string())
            .collect())
    }

    /// Whether git is deliberately ignoring a path.
    ///
    /// Only ever asked about a path already known not to be dirty, so this
    /// costs a process for the files a commit is about to leave out and nothing
    /// for the ones it takes.
    async fn is_ignored(&self, path: &str) -> bool {
        // `check-ignore` exits 1 for "not ignored", which `run` reports as an
        // error. That is the answer, not a failure to get one.
        run(&self.workspace, &["check-ignore", "--quiet", "--", path])
            .await
            .is_ok()
    }
}

/// The first line of a message, bounded, for a listing to show back.
fn subject_of(message: &str) -> String {
    let first = message.lines().next().unwrap_or_default().trim();
    if first.chars().count() <= SUBJECT_MAX_CHARS {
        return first.to_string();
    }
    let kept: String = first.chars().take(SUBJECT_MAX_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

/// Runs one git command in the workspace and returns its stdout.
///
/// Every non-zero exit becomes an `Err` carrying git's own stderr, because git
/// explains itself better than a rewritten summary would — `Committer identity
/// unknown` arrives with the two `git config` lines that fix it, and no message
/// invented here would be as useful.
async fn run(workspace: &Path, args: &[&str]) -> Result<String, String> {
    let output = launch(workspace, args).await?;
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

/// Starts git and waits for it, reporting only the failures that mean git
/// could not be *asked*.
///
/// A non-zero exit comes back as `Ok`, because for some of these commands it is
/// the answer rather than a fault: `check-ignore` exits 1 for "no", and
/// `rev-parse --is-inside-work-tree` exits 128 for "not here". Callers that want
/// the usual reading use [`run`].
async fn launch(workspace: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(workspace)
        // A commit must never open an editor or a credential prompt: there is
        // no terminal behind this, so one would hang the call until it timed
        // out with nothing to show for it.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .stdin(std::process::Stdio::null());
    taurus_tools::no_console(&mut command);

    command.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            // The one failure with a fix the user can act on, and the one most
            // likely on a fresh machine.
            "git is not installed, or is not on this application's PATH.".to_string()
        } else {
            format!("could not run git: {e}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    /// A real repository in a temp directory, with identity configured locally
    /// so the test does not depend on — or disturb — the machine's git config.
    struct Fixture {
        root: PathBuf,
        _dir: TempDir,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let root = dir.path().canonicalize().unwrap();
            for args in [
                vec!["init", "--initial-branch", "main"],
                vec!["config", "user.email", "test@example.invalid"],
                vec!["config", "user.name", "Taurus Test"],
                // A machine with commit signing turned on globally would
                // otherwise fail every commit here for a reason unrelated to
                // what is being tested.
                vec!["config", "commit.gpgsign", "false"],
            ] {
                run(&root, &args).await.expect("git init must work");
            }
            Self { root, _dir: dir }
        }

        fn write(&self, name: &str, contents: &str) {
            let path = self.root.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, contents).unwrap();
        }

        async fn repo(&self) -> Repo {
            Repo::discover(&self.root)
                .await
                .expect("git must be available")
                .expect("the fixture is a repository")
        }

        async fn log(&self) -> String {
            run(&self.root, &["log", "--oneline"])
                .await
                .unwrap_or_default()
        }
    }

    /// Skips the test rather than failing it on a machine with no git.
    macro_rules! needs_git {
        () => {
            if Command::new("git").arg("--version").output().await.is_err() {
                eprintln!("skipping: no git on PATH");
                return;
            }
        };
    }

    #[tokio::test]
    async fn a_directory_that_is_not_a_repository_says_so_without_erroring() {
        needs_git!();
        // Not a problem to report. The drawer draws a different thing, and an
        // error here would put a red box on a perfectly ordinary workspace.
        let dir = TempDir::new().unwrap();
        let status = Repo::status(&dir.path().canonicalize().unwrap()).await;
        assert!(!status.repository);
        assert_eq!(status.unavailable, None);
    }

    #[tokio::test]
    async fn a_fresh_repository_reports_its_branch_and_no_head() {
        needs_git!();
        // A `git init` with no commits is a real state, and `rev-parse HEAD`
        // fails in it. That must read as "no commits yet", not as broken.
        let f = Fixture::new().await;
        let status = Repo::status(&f.root).await;
        assert!(status.repository);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.head, None);
    }

    #[tokio::test]
    async fn a_detached_head_has_no_branch_rather_than_one_called_head() {
        needs_git!();
        // `rev-parse --abbrev-ref HEAD` answers the literal string "HEAD" here,
        // which would be recorded as a branch name and shown to someone as
        // though they were on a branch called HEAD.
        let f = Fixture::new().await;
        f.write("a.txt", "one\n");
        let repo = f.repo().await;
        repo.commit(&["a.txt".into()], "first").await.unwrap();

        let head = repo.head().await.unwrap();
        run(&f.root, &["checkout", "--detach", &head])
            .await
            .unwrap();

        let status = Repo::status(&f.root).await;
        assert!(status.repository);
        assert_eq!(status.branch, None);
        assert!(status.head.is_some());
    }

    #[tokio::test]
    async fn a_commit_takes_the_turns_files_and_leaves_everything_else() {
        needs_git!();
        // The property that makes this safe to offer beside a running
        // conversation: a turn that touched one file commits one file, however
        // dirty the rest of the tree is.
        let f = Fixture::new().await;
        f.write("tracked.txt", "one\n");
        let repo = f.repo().await;
        repo.commit(&["tracked.txt".into()], "seed").await.unwrap();

        f.write("tracked.txt", "two\n");
        f.write("unrelated.txt", "not the turn's work\n");

        let commit = repo
            .commit(&["tracked.txt".into()], "the turn's change")
            .await
            .unwrap();
        assert_eq!(commit.files, vec!["tracked.txt".to_string()]);
        assert!(commit.skipped.is_empty(), "{:?}", commit.skipped);

        let left = run(&f.root, &["status", "--porcelain"]).await.unwrap();
        assert!(
            left.contains("unrelated.txt"),
            "the unrelated file must still be uncommitted: {left}"
        );
    }

    #[tokio::test]
    async fn staged_work_outside_the_turn_survives_the_commit() {
        needs_git!();
        // `git commit -- <paths>` is `--only`. Someone who has staged
        // unrelated work has to still have it staged afterwards, or this
        // feature quietly eats work nobody asked it to touch.
        let f = Fixture::new().await;
        f.write("a.txt", "one\n");
        f.write("staged.txt", "mine\n");
        let repo = f.repo().await;
        repo.commit(&["a.txt".into()], "seed").await.unwrap();

        run(&f.root, &["add", "--", "staged.txt"]).await.unwrap();
        f.write("a.txt", "two\n");
        repo.commit(&["a.txt".into()], "turn").await.unwrap();

        let staged = run(&f.root, &["diff", "--cached", "--name-only"])
            .await
            .unwrap();
        assert!(
            staged.contains("staged.txt"),
            "staged work was swept into the commit: {staged:?}"
        );
    }

    #[tokio::test]
    async fn a_file_the_turn_deleted_is_committed_as_a_deletion() {
        needs_git!();
        let f = Fixture::new().await;
        f.write("doomed.txt", "here\n");
        let repo = f.repo().await;
        repo.commit(&["doomed.txt".into()], "seed").await.unwrap();

        std::fs::remove_file(f.root.join("doomed.txt")).unwrap();
        let commit = repo
            .commit(&["doomed.txt".into()], "remove it")
            .await
            .unwrap();
        assert_eq!(commit.files, vec!["doomed.txt".to_string()]);

        let tracked = run(&f.root, &["ls-files"]).await.unwrap();
        assert!(!tracked.contains("doomed.txt"), "still tracked: {tracked}");
    }

    #[tokio::test]
    async fn an_ignored_file_is_named_as_ignored_rather_than_silently_dropped() {
        needs_git!();
        // `.env` is the case. A turn that rewrote it did real work, and a
        // commit that said nothing about leaving it out would be the worst
        // version of this.
        let f = Fixture::new().await;
        f.write(".gitignore", ".env\n");
        f.write("a.txt", "one\n");
        let repo = f.repo().await;
        repo.commit(&[".gitignore".into(), "a.txt".into()], "seed")
            .await
            .unwrap();

        f.write(".env", "SECRET=1\n");
        f.write("a.txt", "two\n");

        let commit = repo
            .commit(&["a.txt".into(), ".env".into()], "turn")
            .await
            .unwrap();
        assert_eq!(commit.files, vec!["a.txt".to_string()]);
        assert_eq!(commit.skipped.len(), 1);
        assert_eq!(commit.skipped[0].path, ".env");
        assert!(
            commit.skipped[0].reason.contains("ignored"),
            "{:?}",
            commit.skipped[0]
        );
    }

    #[tokio::test]
    async fn a_scratch_file_the_turn_made_and_removed_is_not_called_committed() {
        // A model that writes a helper script, runs it, and deletes it leaves
        // the path in the checkpoint log. Git never saw it, so there is nothing
        // to commit — but "already matches the last commit" would send someone
        // to `git show` looking for a file that was never in one.
        let f = Fixture::new().await;
        f.write("keep.txt", "one\n");
        let repo = f.repo().await;
        repo.commit(&["keep.txt".into()], "seed").await.unwrap();

        f.write("keep.txt", "two\n");
        f.write("scratch.sh", "#!/bin/sh\n");
        std::fs::remove_file(f.root.join("scratch.sh")).unwrap();

        let commit = repo
            .commit(&["keep.txt".into(), "scratch.sh".into()], "turn")
            .await
            .unwrap();
        assert_eq!(commit.files, vec!["keep.txt".to_string()]);
        assert_eq!(commit.skipped.len(), 1);
        assert_eq!(commit.skipped[0].path, "scratch.sh");
        assert!(
            commit.skipped[0].reason.contains("never saw it"),
            "{:?}",
            commit.skipped[0]
        );
    }

    #[tokio::test]
    async fn a_turn_whose_files_were_all_put_back_refuses_with_the_reasons() {
        needs_git!();
        // A later turn reverted it, or the user did. "Nothing to commit" alone
        // would send someone looking for a bug that is not there.
        let f = Fixture::new().await;
        f.write("a.txt", "one\n");
        let repo = f.repo().await;
        repo.commit(&["a.txt".into()], "seed").await.unwrap();

        let before = f.log().await;
        let error = repo.commit(&["a.txt".into()], "nothing").await.unwrap_err();
        assert!(error.contains("a.txt"), "{error}");
        assert!(error.contains("already matches"), "{error}");
        assert_eq!(f.log().await, before, "an empty commit was made anyway");
    }

    #[tokio::test]
    async fn an_empty_message_is_refused_before_git_is_asked() {
        needs_git!();
        let f = Fixture::new().await;
        f.write("a.txt", "one\n");
        let error = f
            .repo()
            .await
            .commit(&["a.txt".into()], "   ")
            .await
            .unwrap_err();
        assert!(error.contains("needs a message"), "{error}");
    }

    #[tokio::test]
    async fn a_path_with_a_leading_dash_is_a_path_and_not_a_flag() {
        needs_git!();
        // Every invocation puts `--` before the paths. Without it a file the
        // model happened to name `-f` would be parsed as an option.
        let f = Fixture::new().await;
        f.write("-weird.txt", "one\n");
        let commit = f
            .repo()
            .await
            .commit(&["-weird.txt".into()], "odd name")
            .await
            .unwrap();
        assert_eq!(commit.files, vec!["-weird.txt".to_string()]);
    }

    #[tokio::test]
    async fn a_file_in_a_subdirectory_keeps_its_relative_path() {
        needs_git!();
        // Checkpoints record workspace-relative paths, and git is run with the
        // workspace as its working directory, so the two have to agree without
        // anything in between rewriting them.
        let f = Fixture::new().await;
        f.write("src/deep/mod.rs", "// one\n");
        let commit = f
            .repo()
            .await
            .commit(&["src/deep/mod.rs".into()], "nested")
            .await
            .unwrap();
        assert_eq!(commit.files, vec!["src/deep/mod.rs".to_string()]);
    }

    #[tokio::test]
    async fn a_long_message_is_reported_back_by_its_subject_alone() {
        needs_git!();
        // `git log --oneline` is where a commit made from a drawer is most
        // likely to be read back, and the body is not part of that line.
        let f = Fixture::new().await;
        f.write("a.txt", "one\n");
        let body = "x".repeat(200);
        let commit = f
            .repo()
            .await
            .commit(&["a.txt".into()], &format!("short subject\n\n{body}"))
            .await
            .unwrap();
        assert_eq!(commit.subject, "short subject");
    }

    #[test]
    fn a_subject_too_long_to_read_in_a_log_is_bounded() {
        let subject = subject_of(&"x".repeat(200));
        assert!(subject.chars().count() <= SUBJECT_MAX_CHARS, "{subject}");
        assert!(subject.ends_with('…'));
    }
}
