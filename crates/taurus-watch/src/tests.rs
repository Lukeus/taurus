//! Against a real watcher on the platform running the tests.

use super::*;

/// A temporary git repository, as far as the watcher is concerned: a `.git`
/// directory is all it looks for.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

fn paths(since: Since) -> Vec<PathBuf> {
    match since {
        Since::Paths(paths) => paths,
        Since::Everything(reason) => panic!("expected paths, got everything: {reason}"),
    }
}

#[test]
fn a_workspace_that_is_not_a_repository_is_not_watched() {
    let dir = tempfile::tempdir().unwrap();
    let Err(Unwatched(reason)) = Watch::start(dir.path()) else {
        panic!("watched a folder with no .git");
    };
    assert!(reason.contains("not a git repository"), "{reason}");
}

#[test]
fn a_write_is_reported_once_the_watcher_has_caught_up() {
    let dir = repo();
    let watch = Watch::start(dir.path()).unwrap();
    let mark = watch.mark();

    let file = dir.path().join("a.txt");
    std::fs::write(&file, "one").unwrap();

    let (since, _) = watch.changes(mark, SYNC_TIMEOUT);
    assert!(paths(since).contains(&file), "the write was not reported");
}

#[test]
fn paths_come_back_under_the_root_as_given() {
    // On macOS a temporary directory is under `/var`, which FSEvents reports as
    // `/private/var`. A consumer comparing against its own paths would find
    // nothing in common.
    let dir = repo();
    let watch = Watch::start(dir.path()).unwrap();
    let mark = watch.mark();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();

    let (since, _) = watch.changes(mark, SYNC_TIMEOUT);
    let found = paths(since);
    assert!(!found.is_empty());
    for path in found {
        assert!(path.starts_with(dir.path()), "{}", path.display());
    }
}

#[test]
fn the_next_mark_carries_on_from_the_answer() {
    let dir = repo();
    let watch = Watch::start(dir.path()).unwrap();
    std::fs::write(dir.path().join("first.txt"), "").unwrap();
    let (_, mark) = watch.changes(watch.mark(), SYNC_TIMEOUT);

    let second = dir.path().join("second.txt");
    std::fs::write(&second, "").unwrap();
    let (since, _) = watch.changes(mark, SYNC_TIMEOUT);
    assert_eq!(paths(since), [second]);
}

#[test]
fn nothing_inside_the_git_directory_is_reported() {
    let dir = repo();
    let watch = Watch::start(dir.path()).unwrap();
    let mark = watch.mark();
    std::fs::write(dir.path().join(".git").join("HEAD"), "ref: refs/heads/main").unwrap();

    let (since, _) = watch.changes(mark, SYNC_TIMEOUT);
    assert_eq!(paths(since), Vec::<PathBuf>::new());
}

#[test]
fn cookies_are_cleaned_up_after_each_question() {
    let dir = repo();
    let watch = Watch::start(dir.path()).unwrap();
    for _ in 0..3 {
        watch.changes(watch.mark(), SYNC_TIMEOUT);
    }
    let left = std::fs::read_dir(dir.path().join(".git").join(COOKIES))
        .unwrap()
        .count();
    assert_eq!(left, 0);
}

#[test]
fn a_worktree_puts_its_cookies_in_the_git_directory_it_names() {
    let main = tempfile::tempdir().unwrap();
    let gitdir = main.path().join("worktrees").join("w");
    std::fs::create_dir_all(&gitdir).unwrap();
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(
        tree.path().join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();

    let watch = Watch::start(tree.path()).unwrap();
    let mark = watch.mark();
    let file = tree.path().join("c.txt");
    std::fs::write(&file, "").unwrap();

    let (since, _) = watch.changes(mark, SYNC_TIMEOUT);
    assert!(paths(since).contains(&file));
    assert!(gitdir.join(COOKIES).is_dir());
}

#[test]
fn a_subdirectory_is_seen_where_the_platform_watches_it() {
    let dir = repo();
    let sub = dir.path().join("src");
    std::fs::create_dir(&sub).unwrap();
    let watch = Watch::start(dir.path()).unwrap();
    // Asked for everywhere. A no-op except on Linux, where nothing below the
    // root is watched until it is named.
    watch.watch_dirs([sub.as_path()]);

    let mark = watch.mark();
    let file = sub.join("lib.rs");
    std::fs::write(&file, "").unwrap();
    let (since, _) = watch.changes(mark, SYNC_TIMEOUT);
    assert!(paths(since).contains(&file));
}

#[cfg(target_os = "linux")]
#[test]
fn on_linux_a_directory_nobody_named_is_not_watched() {
    let dir = repo();
    let ignored = dir.path().join("target");
    std::fs::create_dir(&ignored).unwrap();
    let watch = Watch::start(dir.path()).unwrap();

    let mark = watch.mark();
    std::fs::write(ignored.join("out.o"), "").unwrap();
    let (since, _) = watch.changes(mark, SYNC_TIMEOUT);
    assert_eq!(paths(since), Vec::<PathBuf>::new());
}

#[test]
fn a_cookie_that_cannot_be_written_is_answered_with_everything() {
    let dir = repo();
    let watch = Watch::start(dir.path()).unwrap();
    std::fs::remove_dir_all(dir.path().join(".git").join(COOKIES)).unwrap();

    let (since, _) = watch.changes(watch.mark(), SYNC_TIMEOUT);
    assert!(matches!(since, Since::Everything(_)), "{since:?}");
}
