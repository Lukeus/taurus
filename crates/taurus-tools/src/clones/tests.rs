use super::*;

#[test]
fn a_blob_name_that_leaves_the_store_is_refused() {
    let store = Path::new("/store");
    assert!(blob_path(store, "s1.blobs/abc123").is_some());
    for bad in [
        "../s1.blobs/abc",
        "s1.blobs/../../etc/passwd",
        "s1.blobs/",
        "s1/abc",
        "s1.blobs/a/b",
        "/abs.blobs/x",
        ".blobs/x",
    ] {
        assert!(blob_path(store, bad).is_none(), "{bad}");
    }
}

#[test]
fn a_restore_replaces_the_file_whole() {
    let dir = tempfile::tempdir().unwrap();
    let blob = dir.path().join("blob");
    std::fs::write(&blob, b"original").unwrap();
    let target = dir.path().join("deep/er/file.bin");

    restore(&blob, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"original");

    std::fs::write(&target, b"changed since").unwrap();
    restore(&blob, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    // Nothing left beside it.
    assert_eq!(
        std::fs::read_dir(target.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn a_restore_from_a_missing_blob_says_so_and_leaves_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("file");
    std::fs::write(&target, b"current").unwrap();
    let error = restore(&dir.path().join("gone"), &target).unwrap_err();
    assert!(error.to_string().contains("kept copy is gone"), "{error}");
    assert_eq!(std::fs::read(&target).unwrap(), b"current");
}

#[cfg(unix)]
#[test]
fn a_stage_left_by_a_process_that_exited_is_cleared() {
    let dir = tempfile::tempdir().unwrap();
    // No process has this id: Linux and macOS both cap pids well below it.
    let dead = dir.path().join("4000000-0-abc");
    let mine = dir.path().join(format!("{}-0-abc", std::process::id()));
    std::fs::write(&dead, b"").unwrap();
    std::fs::write(&mine, b"").unwrap();

    prune_stage(dir.path());
    assert!(!dead.exists());
    assert!(mine.exists());
}

#[test]
fn a_store_on_the_same_volume_clones_where_the_filesystem_can() {
    // A probe, not an assertion that it can: CI's ext4 can't, and there the
    // answer must be `None` rather than a cloner that copies.
    let store = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let Some(cloner) = Cloner::new(store.path(), workspace.path(), DEFAULT_LIMIT_BYTES) else {
        return;
    };
    let file = workspace.path().join("a.bin");
    std::fs::write(&file, [1u8, 2, 3]).unwrap();
    let staged = cloner.stage(&file, 3).expect("a stage");
    assert_eq!(std::fs::read(staged.path()).unwrap(), [1, 2, 3]);
    let path = staged.path().to_path_buf();
    drop(staged);
    settle();
    assert!(!path.exists());
}

#[test]
fn keeping_counts_every_blob_already_in_the_store() {
    let store = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let Some(cloner) = Cloner::new(store.path(), workspace.path(), 10) else {
        return;
    };
    let file = workspace.path().join("a.bin");
    std::fs::write(&file, [7u8; 6]).unwrap();
    assert!(matches!(cloner.keep(&file, 6, "s1"), State::Clone { .. }));
    // Six held, six more would be twelve: past ten, whichever conversation.
    assert!(matches!(cloner.keep(&file, 6, "s2"), State::Opaque { .. }));
}
