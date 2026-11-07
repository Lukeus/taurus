use super::*;
use std::sync::Arc;
use tempfile::TempDir;

/// A repository with one commit, an ignored `.env`, and somewhere to keep
/// worktrees that is outside it.
struct Fixture {
    _dirs: (TempDir, TempDir),
    top: PathBuf,
    trees: Worktrees,
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn fixture() -> Fixture {
    let repo = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let top = repo.path().canonicalize().unwrap();
    sh(&top, &["init", "-q", "-b", "main"]);
    sh(&top, &["config", "user.email", "me@example.com"]);
    sh(&top, &["config", "user.name", "Me"]);
    std::fs::write(top.join("a.txt"), "one\n").unwrap();
    std::fs::write(top.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
    std::fs::create_dir_all(top.join("app/src")).unwrap();
    std::fs::write(top.join("app/src/lib.rs"), "fn a() {}\n").unwrap();
    sh(&top, &["add", "."]);
    sh(&top, &["commit", "-qm", "init"]);
    let trees = Worktrees::new(home.path().join(".taurus-worktrees"));
    Fixture {
        _dirs: (repo, home),
        top,
        trees,
    }
}

fn who() -> Requester {
    Requester {
        agent: "coder".into(),
        session: Some("s1".into()),
        call: Some("c1".into()),
    }
}

#[tokio::test]
async fn a_delegate_starts_from_the_folder_as_it_is_and_the_index_is_untouched() {
    let f = fixture();
    std::fs::write(f.top.join("a.txt"), "one\nedited, not committed\n").unwrap();
    std::fs::write(f.top.join("new.txt"), "untracked\n").unwrap();
    sh(&f.top, &["add", "a.txt"]);
    std::fs::write(f.top.join("a.txt"), "one\nedited again, after staging\n").unwrap();
    let staged_before = sh(&f.top, &["diff", "--cached"]);

    let opened = f.trees.open(&f.top, &who()).await.unwrap();

    // What the parent sees on disk, not what is committed or staged.
    assert_eq!(
        std::fs::read_to_string(opened.root.join("a.txt")).unwrap(),
        "one\nedited again, after staging\n"
    );
    assert!(opened.root.join("new.txt").is_file());
    // Ignored files aren't in a snapshot.
    assert_eq!(sh(&f.top, &["diff", "--cached"]), staged_before);
    assert_eq!(sh(&f.top, &["branch", "--show-current"]), "main");
    // No branch was made for it.
    assert_eq!(sh(&f.top, &["branch", "--list"]), "* main");
    f.trees.close(&opened.run).await.unwrap();
}

#[tokio::test]
async fn the_copy_is_outside_the_workspace_and_the_config_home() {
    let f = fixture();
    let opened = f.trees.open(&f.top, &who()).await.unwrap();
    assert!(!opened.root.starts_with(&f.top));
    assert!(opened
        .root
        .ancestors()
        .all(|dir| dir.file_name() != Some(OsStr::new(".taurus"))));
    f.trees.close(&opened.run).await.unwrap();
}

#[tokio::test]
async fn a_workspace_in_a_subfolder_gets_the_same_subfolder_of_the_copy() {
    let f = fixture();
    let opened = f.trees.open(&f.top.join("app"), &who()).await.unwrap();
    assert!(opened.root.ends_with("app"), "{}", opened.root.display());
    assert!(opened.root.join("src/lib.rs").is_file());

    std::fs::write(opened.root.join("src/lib.rs"), "fn b() {}\n").unwrap();
    std::fs::write(
        opened.root.join("../a.txt"),
        "changed above the workspace\n",
    )
    .unwrap();
    let closed = f.trees.close(&opened.run).await.unwrap();
    let kept = closed.kept.unwrap();
    // Relative to the workspace, the way the parent names them.
    assert_eq!(kept.files, ["src/lib.rs"]);
    assert_eq!(kept.outside, ["a.txt"]);
}

#[tokio::test]
async fn the_repository_s_hooks_do_not_run() {
    let f = fixture();
    let marker = f.top.join("hook-ran");
    let hook = f.top.join(".git/hooks/post-checkout");
    std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let opened = f.trees.open(&f.top, &who()).await.unwrap();
    std::fs::write(opened.root.join("a.txt"), "two\n").unwrap();
    f.trees.close(&opened.run).await.unwrap();
    assert!(!marker.exists(), "post-checkout ran");
}

#[tokio::test]
async fn closing_keeps_the_work_as_a_commit_and_removes_the_folder() {
    let f = fixture();
    let opened = f.trees.open(&f.top, &who()).await.unwrap();
    std::fs::write(opened.root.join("a.txt"), "two\n").unwrap();
    std::fs::write(opened.root.join("b.txt"), "new\n").unwrap();
    std::fs::remove_file(opened.root.join("app/src/lib.rs")).unwrap();
    // Ignored files are thrown away with the folder, and don't stop it going.
    std::fs::create_dir_all(opened.root.join("node_modules/x")).unwrap();
    std::fs::write(opened.root.join("node_modules/x/i.js"), "x").unwrap();

    let closed = f.trees.close(&opened.run).await.unwrap();
    assert_eq!(closed.left_behind, None);
    assert!(!opened.root.exists());
    let kept = closed.kept.unwrap();
    assert_eq!(kept.files, ["a.txt", "app/src/lib.rs", "b.txt"]);
    assert_eq!(kept.base, opened.base);

    // The person's folder is exactly as it was.
    assert_eq!(
        std::fs::read_to_string(f.top.join("a.txt")).unwrap(),
        "one\n"
    );
    assert!(!f.top.join("b.txt").exists());
    // And git still has the work.
    let result = Worktrees::result_ref(&opened.run);
    assert_eq!(sh(&f.top, &["show", &format!("{result}:a.txt")]), "two");
    assert!(!sh(&f.top, &["worktree", "list"]).contains(&opened.run));
}

#[tokio::test]
async fn a_delegate_that_changed_nothing_leaves_nothing() {
    let f = fixture();
    let opened = f.trees.open(&f.top, &who()).await.unwrap();
    let closed = f.trees.close(&opened.run).await.unwrap();
    assert!(closed.kept.is_none());
    assert!(!opened.root.exists());
    assert_eq!(sh(&f.top, &["for-each-ref", "refs/taurus/"]), "");
    // Closing twice is an answer, not a failure.
    assert!(f.trees.close(&opened.run).await.unwrap().kept.is_none());
}

#[tokio::test]
async fn a_signing_config_does_not_stop_a_snapshot() {
    let f = fixture();
    // A key agent that would fail, which is what a passphrase prompt with
    // nobody there comes to. Today's git doesn't read `commit.gpgSign` for
    // `commit-tree` at all, and this holds that, whatever a later git does.
    sh(&f.top, &["config", "commit.gpgsign", "true"]);
    sh(&f.top, &["config", "gpg.program", "/nonexistent/gpg"]);
    let opened = f.trees.open(&f.top, &who()).await.unwrap();
    std::fs::write(opened.root.join("a.txt"), "two\n").unwrap();
    assert!(f.trees.close(&opened.run).await.unwrap().kept.is_some());
}

#[tokio::test]
async fn listed_ignored_files_are_copied_in_and_nothing_else() {
    let f = fixture();
    std::fs::write(f.top.join(".env"), "SECRET=1\n").unwrap();
    std::fs::create_dir_all(f.top.join("node_modules/dep")).unwrap();
    std::fs::write(f.top.join("node_modules/dep/index.js"), "x").unwrap();
    // Listed, but tracked, so it's in the snapshot and not copied.
    std::fs::write(f.top.join(INCLUDE_FILE), ".env\na.txt\nhosts.link\n").unwrap();
    // Ignored and listed, but a link: copying it would copy what it points at.
    std::fs::write(f.top.join(".git/info/exclude"), "hosts.link\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hosts", f.top.join("hosts.link")).unwrap();

    let opened = f.trees.open(&f.top, &who()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(opened.root.join(".env")).unwrap(),
        "SECRET=1\n"
    );
    assert!(!opened.root.join("node_modules").exists());
    assert!(std::fs::symlink_metadata(opened.root.join("hosts.link")).is_err());
    let note = opened.note.clone().unwrap();
    assert!(note.contains(".env") && !note.contains("a.txt"), "{note}");

    // A copied file the delegate edits isn't something it changed in the
    // repository: it's ignored there too.
    std::fs::write(opened.root.join(".env"), "SECRET=2\n").unwrap();
    assert!(f.trees.close(&opened.run).await.unwrap().kept.is_none());
    assert_eq!(
        std::fs::read_to_string(f.top.join(".env")).unwrap(),
        "SECRET=1\n"
    );
}

#[tokio::test]
async fn a_folder_outside_git_is_refused_with_a_reason() {
    let plain = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let trees = Worktrees::new(home.path());
    let why = trees.open(plain.path(), &who()).await.unwrap_err();
    assert!(why.contains("isn't in a git repository"), "{why}");
}

#[tokio::test]
async fn submodules_and_filters_are_refused_with_a_reason() {
    let f = fixture();
    std::fs::write(
        f.top.join(".gitattributes"),
        "*.psd filter=lfs diff=lfs merge=lfs -text\n",
    )
    .unwrap();
    sh(&f.top, &["add", ".gitattributes"]);
    let why = f.trees.open(&f.top, &who()).await.unwrap_err();
    assert!(why.contains("Git LFS"), "{why}");

    std::fs::write(
        f.top.join(".gitattributes"),
        "# filter=lfs\n*.secret filter=crypt\n",
    )
    .unwrap();
    let why = f.trees.open(&f.top, &who()).await.unwrap_err();
    assert!(why.contains("(crypt)"), "{why}");

    std::fs::remove_file(f.top.join(".gitattributes")).unwrap();
    sh(&f.top, &["rm", "-q", "--cached", ".gitattributes"]);
    std::fs::write(f.top.join(".gitmodules"), "").unwrap();
    let why = f.trees.open(&f.top, &who()).await.unwrap_err();
    assert!(why.contains("submodules"), "{why}");
}

#[test]
fn a_run_name_cannot_reach_outside_the_records() {
    let trees = Worktrees::new("/nowhere");
    assert!(trees.record("../../etc/passwd").is_err());
    assert!(trees.record("abc").is_err());
    assert!(!valid_run("abcdef12345/"));
    assert!(valid_run("abcdef123456"));
}

#[tokio::test]
async fn a_run_whose_owner_died_is_closed_on_the_next_start_and_its_work_kept() {
    let f = fixture();
    let opened = f.trees.open(&f.top, &who()).await.unwrap();
    std::fs::write(opened.root.join("a.txt"), "written before the crash\n").unwrap();
    // The process ends: its locks go with it.
    f.trees.held.lock().unwrap().clear();

    let next = Worktrees::new(f.trees.home.clone());
    // Another test's git, forked in the instant before it execs, can hold the
    // lock for a moment after it's let go. Tidying errs towards leaving a run
    // alone, so it may take more than one look; a real start sees the lock of
    // a process that's gone.
    let mut record = next.record(&opened.run).unwrap();
    for _ in 0..50 {
        assert!(next.tidy().await.is_empty());
        record = next.record(&opened.run).unwrap();
        if record.state != State::Running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(record.state, State::Kept);
    assert!(!opened.root.exists());
    let result = Worktrees::result_ref(&opened.run);
    assert_eq!(
        sh(&f.top, &["show", &format!("{result}:a.txt")]),
        "written before the crash"
    );
}

#[tokio::test]
async fn a_close_that_fails_lets_go_so_the_next_start_retries_it() {
    let f = fixture();
    let opened = f.trees.open(&f.top, &who()).await.unwrap();
    std::fs::write(opened.root.join("a.txt"), "work worth keeping\n").unwrap();
    // git can't read the copy, so the close stops partway.
    let dot_git = opened.root.join(".git");
    let pointer = std::fs::read_to_string(&dot_git).unwrap();
    std::fs::write(&dot_git, "not a pointer\n").unwrap();
    assert!(f.trees.close(&opened.run).await.is_err());
    assert!(
        !f.trees.owned(&opened.run),
        "a failed close kept the lock, so tidy would never retry it"
    );
    assert_eq!(f.trees.record(&opened.run).unwrap().state, State::Running);

    // Once git can read it again, the next tidy closes it with the work kept.
    std::fs::write(&dot_git, pointer).unwrap();
    let mut state = State::Running;
    for _ in 0..50 {
        f.trees.tidy().await;
        state = f.trees.record(&opened.run).unwrap().state;
        if state != State::Running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(state, State::Kept);
}

#[tokio::test]
async fn a_run_another_process_has_open_is_left_alone() {
    let f = fixture();
    let opened = f.trees.open(&f.top, &who()).await.unwrap();
    // A second process starting while the first is still working.
    let other = Worktrees::new(f.trees.home.clone());
    assert!(other.tidy().await.is_empty());
    assert_eq!(other.record(&opened.run).unwrap().state, State::Running);
    assert!(opened.root.exists());
    f.trees.close(&opened.run).await.unwrap();
}

#[tokio::test]
async fn a_run_being_opened_is_never_mistaken_for_one_a_crash_left() {
    let f = fixture();
    let trees = Arc::new(f.trees);
    // Another start tidying over and over while runs are opened: any gap
    // between a run's record and its lock is a run closed under its delegate.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tidier = {
        let home = trees.home.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let other = Worktrees::new(home);
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                other.tidy().await;
                tokio::task::yield_now().await;
            }
        })
    };
    for _ in 0..10 {
        let opened = trees.open(&f.top, &who()).await.unwrap();
        assert!(opened.root.is_dir(), "closed while it was being opened");
        assert_eq!(trees.record(&opened.run).unwrap().state, State::Running);
        std::fs::write(opened.root.join("a.txt"), "two\n").unwrap();
        assert!(trees.close(&opened.run).await.unwrap().kept.is_some());
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    tidier.await.unwrap();
}

/// A run that has stopped, having done `work` in its copy.
async fn kept_run(f: &Fixture, work: impl FnOnce(&Path)) -> String {
    let opened = f.trees.open(&f.top, &who()).await.unwrap();
    work(&opened.root);
    let closed = f.trees.close(&opened.run).await.unwrap();
    assert!(closed.kept.is_some(), "the work changed nothing");
    opened.run
}

fn read(path: PathBuf) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[tokio::test]
async fn applying_takes_what_the_workspace_left_alone_and_records_it_for_undo() {
    let f = fixture();
    let run = kept_run(&f, |root| {
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::create_dir_all(root.join("new/dir")).unwrap();
        std::fs::write(root.join("new/dir/b.sh"), "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                root.join("new/dir/b.sh"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        std::fs::remove_file(root.join("app/src/lib.rs")).unwrap();
    })
    .await;
    let logs = TempDir::new().unwrap();
    let recorder =
        taurus_tools::CheckpointStore::new(logs.path()).begin_turn("s1", &f.top, "apply");

    let plan = f
        .trees
        .apply(&run, Some("s1"), Some(&recorder), false)
        .await
        .unwrap();

    assert!(plan.applied, "{plan:?}");
    assert_eq!(read(f.top.join("a.txt")), "two\n");
    assert!(!f.top.join("app/src/lib.rs").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(f.top.join("new/dir/b.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "{mode:o}");
    }
    assert_eq!(
        recorder.changed_paths().await,
        ["a.txt", "app/src/lib.rs", "new/dir/b.sh"]
    );
    // The index and branch are the person's, untouched.
    assert_eq!(sh(&f.top, &["diff", "--cached"]), "");
    assert_eq!(sh(&f.top, &["branch", "--show-current"]), "main");
    // Once is all.
    let again = f
        .trees
        .apply(&run, Some("s1"), None, false)
        .await
        .unwrap_err();
    assert!(again.contains("already been applied"), "{again}");
}

#[tokio::test]
async fn both_sides_changing_a_file_merges_when_the_changes_do_not_overlap() {
    let f = fixture();
    std::fs::write(f.top.join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    sh(&f.top, &["commit", "-qam", "five lines"]);
    let run = kept_run(&f, |root| {
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\nfour\nFIVE\n").unwrap();
    })
    .await;
    std::fs::write(f.top.join("a.txt"), "ONE\ntwo\nthree\nfour\nfive\n").unwrap();

    let plan = f.trees.apply(&run, Some("s1"), None, false).await.unwrap();
    assert_eq!(plan.files[0].action, ApplyAction::Merge);
    assert_eq!(read(f.top.join("a.txt")), "ONE\ntwo\nthree\nfour\nFIVE\n");
}

#[tokio::test]
async fn one_conflict_refuses_the_whole_apply_and_writes_no_markers() {
    let f = fixture();
    let run = kept_run(&f, |root| {
        std::fs::write(root.join("a.txt"), "theirs\n").unwrap();
        std::fs::write(root.join("clean.txt"), "would be fine\n").unwrap();
    })
    .await;
    std::fs::write(f.top.join("a.txt"), "mine\n").unwrap();

    let plan = f.trees.apply(&run, Some("s1"), None, false).await.unwrap();
    assert!(!plan.applied);
    let conflicts: Vec<&str> = plan.conflicts().map(|c| c.path.as_str()).collect();
    assert_eq!(conflicts, ["a.txt"]);
    assert!(
        plan.describe().contains("Nothing was applied"),
        "{}",
        plan.describe()
    );
    assert_eq!(read(f.top.join("a.txt")), "mine\n");
    assert!(!f.top.join("clean.txt").exists(), "half applied");
    // Still there to try again, or apply by hand.
    assert_eq!(f.trees.record(&run).unwrap().state, State::Kept);
}

#[tokio::test]
async fn a_file_that_decides_what_runs_is_never_applied() {
    let f = fixture();
    let run = kept_run(&f, |root| {
        std::fs::create_dir_all(root.join(".taurus")).unwrap();
        std::fs::write(
            root.join(".taurus/permissions.json"),
            r#"{"allowed":["run_command"]}"#,
        )
        .unwrap();
    })
    .await;
    let plan = f.trees.apply(&run, Some("s1"), None, false).await.unwrap();
    assert!(!plan.applied);
    assert!(plan.files[0]
        .why
        .as_deref()
        .unwrap()
        .contains("decides what runs"));
    assert!(!f.top.join(".taurus/permissions.json").exists());
}

#[tokio::test]
async fn a_look_ahead_writes_nothing_and_another_conversation_cannot_apply() {
    let f = fixture();
    let run = kept_run(&f, |root| {
        std::fs::write(root.join("a.txt"), "two\n").unwrap()
    })
    .await;
    let plan = f.trees.apply(&run, Some("s1"), None, true).await.unwrap();
    assert_eq!(plan.files[0].action, ApplyAction::Take);
    assert!(!plan.applied);
    assert_eq!(read(f.top.join("a.txt")), "one\n");

    let other = f
        .trees
        .apply(&run, Some("s2"), None, false)
        .await
        .unwrap_err();
    assert!(other.contains("another conversation"), "{other}");
    assert_eq!(read(f.top.join("a.txt")), "one\n");
}

#[tokio::test]
async fn discarding_lets_git_forget_the_work() {
    let f = fixture();
    let run = kept_run(&f, |root| {
        std::fs::write(root.join("a.txt"), "two\n").unwrap()
    })
    .await;
    f.trees.discard(&run, Some("s1")).await.unwrap();
    assert_eq!(sh(&f.top, &["for-each-ref", "refs/taurus/"]), "");
    assert_eq!(f.trees.record(&run).unwrap().state, State::Discarded);
    let after = f
        .trees
        .apply(&run, Some("s1"), None, false)
        .await
        .unwrap_err();
    assert!(after.contains("discarded"), "{after}");
}

#[test]
fn the_worktrees_are_beside_the_config_home_and_named_for_it() {
    assert_eq!(
        Worktrees::home_beside(Path::new("/u/.taurus")),
        Path::new("/u/.taurus-worktrees")
    );
    // Two homes in one folder don't share.
    assert_ne!(
        Worktrees::home_beside(Path::new("/tmp/.tmpA")),
        Worktrees::home_beside(Path::new("/tmp/.tmpB"))
    );
}
