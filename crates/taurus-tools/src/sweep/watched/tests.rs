//! The picture against a real watcher, checked the only way that matters: it
//! has to say exactly what a walk of the same tree says.

use super::*;
use std::time::{Duration, Instant};

/// A workspace the watcher will accept, and a slot already answering from it.
struct Watched {
    slot: Slot,
    root: PathBuf,
    _dir: tempfile::TempDir,
}

impl Watched {
    fn new() -> Self {
        Self::with(|_| {})
    }

    /// With `prepare` run on the root before watching starts.
    fn with(prepare: impl FnOnce(&Path)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        prepare(&root);

        let slot = Slot::default();
        slot.files(&root);
        // Generous, because starting can be slow on a development machine
        // whose `target/` holds a great many files; see `start`.
        let deadline = Instant::now() + Duration::from_secs(120);
        while slot.watching(&root) == Watching::Starting {
            assert!(Instant::now() < deadline, "the watcher never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        if refusal().is_none() {
            assert_eq!(slot.watching(&root), Watching::Live);
        }
        // The first look takes the picture; every look after answers from it.
        slot.files(&root);
        Watched {
            slot,
            root,
            _dir: dir,
        }
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Asserts the slot's answer is the walk's, and returns it as
    /// workspace-relative names.
    fn agrees(&self) -> Vec<String> {
        let names = |files: Option<Vec<Stamped>>| {
            let mut names: Vec<(String, u64)> = files
                .expect("under the file cap")
                .into_iter()
                .map(|(path, len, _)| {
                    let name = path.strip_prefix(&self.root).unwrap();
                    (name.to_string_lossy().replace('\\', "/"), len)
                })
                .collect();
            names.sort();
            names
        };
        let watched = names(self.slot.files(&self.root));
        let walked = names(super::super::sweepable(&self.root));
        assert_eq!(watched, walked, "the watcher's answer is not the walk's");
        watched.into_iter().map(|(name, _)| name).collect()
    }

    fn answered(&self) -> usize {
        match &self.slot.inner().phase {
            Phase::Live { live, .. } => live.answered,
            _ => 0,
        }
    }
}

#[test]
fn a_workspace_that_is_not_a_repository_walks_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let slot = Slot::default();
    slot.files(&root);
    let deadline = Instant::now() + Duration::from_secs(120);
    while slot.watching(&root) == Watching::Starting {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let Watching::Off(reason) = slot.watching(&root) else {
        panic!("watched a folder with no .git");
    };
    assert!(
        reason.contains("not a git repository") || reason.contains("Windows"),
        "{reason}"
    );
}

#[cfg(not(windows))]
mod live {
    use super::*;

    #[test]
    fn edits_creations_and_deletions_match_the_walk() {
        let w = Watched::with(|root| {
            std::fs::write(root.join("keep.txt"), "kept").unwrap();
            std::fs::write(root.join("edit.txt"), "short").unwrap();
            std::fs::write(root.join("gone.txt"), "doomed").unwrap();
        });
        w.write("edit.txt", "a good deal longer than it was");
        w.write("new.txt", "fresh");
        std::fs::remove_file(w.root.join("gone.txt")).unwrap();

        assert_eq!(w.agrees(), ["edit.txt", "keep.txt", "new.txt"]);
        assert!(
            w.answered() > 0,
            "the answer came from a walk, not the watcher"
        );
    }

    #[test]
    fn a_new_directory_is_entered_with_everything_already_in_it() {
        let w = Watched::new();
        w.write("src/deep/er/lib.rs", "fn main() {}");
        w.write("src/mod.rs", "mod deep;");
        assert_eq!(w.agrees(), ["src/deep/er/lib.rs", "src/mod.rs"]);
    }

    #[test]
    fn a_new_directory_an_ignore_rule_excludes_is_not_entered() {
        let w = Watched::with(|root| std::fs::write(root.join(".gitignore"), "build/\n").unwrap());
        w.write("build/out.o", "object code");
        w.write("main.c", "int main;");
        assert_eq!(w.agrees(), [".gitignore", "main.c"]);
    }

    #[test]
    fn a_file_an_ignore_rule_excludes_by_name_is_still_covered() {
        // `.env` beside the code: the walk does not yield it, but the flat
        // read of its directory does, and so must the picture.
        let w = Watched::with(|root| std::fs::write(root.join(".gitignore"), ".env\n").unwrap());
        w.write(".env", "SECRET=1");
        assert_eq!(w.agrees(), [".env", ".gitignore"]);
    }

    #[test]
    fn a_removed_directory_takes_its_files_with_it() {
        let w = Watched::with(|root| {
            std::fs::create_dir_all(root.join("old/inner")).unwrap();
            std::fs::write(root.join("old/inner/a.txt"), "a").unwrap();
            std::fs::write(root.join("old/b.txt"), "b").unwrap();
            std::fs::write(root.join("stay.txt"), "s").unwrap();
        });
        std::fs::remove_dir_all(w.root.join("old")).unwrap();
        assert_eq!(w.agrees(), ["stay.txt"]);
    }

    #[test]
    fn a_renamed_directory_moves_its_files() {
        let w = Watched::with(|root| {
            std::fs::create_dir_all(root.join("before/sub")).unwrap();
            std::fs::write(root.join("before/sub/x.txt"), "x").unwrap();
        });
        std::fs::rename(w.root.join("before"), w.root.join("after")).unwrap();
        assert_eq!(w.agrees(), ["after/sub/x.txt"]);

        // And the renamed directory is still watched under its new name.
        w.write("after/sub/y.txt", "y");
        assert_eq!(w.agrees(), ["after/sub/x.txt", "after/sub/y.txt"]);
    }

    #[test]
    fn editing_an_ignore_file_changes_what_is_covered() {
        let w = Watched::with(|root| {
            std::fs::create_dir(root.join("logs")).unwrap();
            std::fs::write(root.join("logs/run.log"), "log").unwrap();
        });
        assert_eq!(w.agrees(), ["logs/run.log"]);
        w.write(".gitignore", "logs/\n");
        assert_eq!(w.agrees(), [".gitignore"]);
    }

    #[test]
    fn nothing_under_git_or_the_harness_directory_is_covered() {
        let w = Watched::new();
        w.write(".git/HEAD", "ref: refs/heads/main");
        w.write(".taurus/settings.json", "{}");
        w.write("real.txt", "r");
        assert_eq!(w.agrees(), ["real.txt"]);
    }

    #[test]
    fn a_missed_change_switches_the_watcher_off_and_names_the_file() {
        let w = Watched::new();
        // Plant a file in the picture that is not on disk, as a watcher that
        // missed its deletion would leave it.
        {
            let mut inner = w.slot.inner();
            let Phase::Live { live, .. } = &mut inner.phase else {
                panic!("not live");
            };
            let picture = live.picture.as_mut().expect("a picture");
            picture.files.insert(w.root.join("phantom.txt"), (7, None));
            live.answered = VERIFY_EVERY - 1;
        }

        // The look that verifies answers with the walk, not the bad picture.
        let files = w.slot.files(&w.root).unwrap();
        assert!(files
            .iter()
            .all(|(path, ..)| !path.ends_with("phantom.txt")));

        let Watching::Off(reason) = w.slot.watching(&w.root) else {
            panic!("still trusting a watcher that missed a change");
        };
        assert!(reason.contains("phantom.txt"), "{reason}");
    }

    #[test]
    fn a_check_that_finds_nothing_wrong_keeps_watching() {
        let w = Watched::new();
        for n in 0..VERIFY_EVERY + 1 {
            w.write(&format!("f{n}.txt"), &"x".repeat(n + 1));
            w.agrees();
        }
        assert_eq!(w.slot.watching(&w.root), Watching::Live);
    }

    #[test]
    fn clearing_stops_watching() {
        let w = Watched::new();
        w.slot.clear();
        assert_eq!(w.slot.watching(&w.root), Watching::No);
    }

    #[test]
    fn a_watcher_that_finishes_starting_after_a_clear_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let slot = Slot::default();
        slot.files(&root);
        slot.clear();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(slot.watching(&root), Watching::No);
    }
}
