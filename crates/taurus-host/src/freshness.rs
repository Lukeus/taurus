//! Whether the files behind a piece of config have moved since it was read.
//!
//! Taurus does not watch config files. A watcher delivers an edit whenever the
//! editor happens to save, which is routinely the middle of a running turn —
//! the one moment when nothing underneath a turn should change. The roster a
//! turn delegates against, and the brief it was given, have to be the ones it
//! started with.
//!
//! So config is re-read at turn boundaries instead. Nothing is in flight there,
//! and the turn about to start is the earliest one that could have used the
//! change anyway, so an edit lands exactly one turn later than a watcher would
//! have delivered it and without the race. What that costs is a check on every
//! message, which means the check has to be much cheaper than the work it
//! guards.
//!
//! This is that check: the length and modification time of every file in a set.
//! It is the comparison [`taurus_tools::sweep`] makes about the workspace and
//! it is blind in the same place — a rewrite to the same length within one
//! filesystem tick looks like nothing happened. That is the right trade here
//! for the same reason it is there: the alternative is reading and parsing
//! every agent file on every message in order to learn that none of them
//! changed, and the failure mode is an edit that lands one turn later still.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A cheap fingerprint of a set of files.
///
/// Compared, never inspected. Two of these are equal when every file was the
/// same length at the same modification time and the same files were there at
/// all — which is why absence is recorded rather than skipped: a `CLAUDE.md`
/// that did not exist and now does is exactly the change worth noticing, and a
/// fingerprint that only listed what it found would miss it.
///
/// Two kinds of thing can be watched, because config comes in both shapes. A
/// *file* is watched by name, whether or not it exists. A *directory* is
/// watched by rule — every entry matching a suffix — so that a file nobody has
/// written yet is still covered. Agents and Copilot's scoped instructions both
/// arrive that way, and a fingerprint that could only name files it already
/// knew would never notice the first one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Freshness {
    /// What to look at, kept so the same set can be re-stated later.
    watched: Vec<Watched>,
    /// What was there when it was last looked at.
    stamps: Vec<(PathBuf, Stat)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Watched {
    File(PathBuf),
    Dir {
        root: PathBuf,
        suffix: String,
        recursive: bool,
    },
    /// One level down and no further — see [`Freshness::of_child_dirs`].
    ChildDirs {
        root: PathBuf,
        suffix: String,
    },
}

/// What one file looked like, or `None` for one that was not there.
///
/// Length and modification time. The time is itself optional because not every
/// platform and filesystem reports one; where it is missing this degrades to
/// comparing length alone, which is weaker but is also all the caller would
/// have had.
type Stat = Option<(u64, Option<SystemTime>)>;

impl Freshness {
    /// Of these files, present or not, in the order given.
    pub fn of_files<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Self {
        Self::of(
            paths
                .into_iter()
                .map(|path| Watched::File(path.to_path_buf()))
                .collect(),
        )
    }

    /// Of every file under each directory whose name ends in `suffix`.
    ///
    /// The suffix rather than an extension, because the conventions being read
    /// use doubled ones: `.instructions.md` and `.agent.md` are both `.md` to
    /// [`Path::extension`], and a directory holding one kind must not rescan
    /// for the other.
    ///
    /// Match what the scan being guarded actually reads. A fingerprint covering
    /// more would rescan for a file the scan can never load; one covering less
    /// would never notice a new one.
    pub fn of_dirs<'a>(
        dirs: impl IntoIterator<Item = &'a Path>,
        suffix: &str,
        recursive: bool,
    ) -> Self {
        Self::of(
            dirs.into_iter()
                .map(|root| Watched::Dir {
                    root: root.to_path_buf(),
                    suffix: suffix.to_string(),
                    recursive,
                })
                .collect(),
        )
    }

    /// Every `suffix` file sitting directly inside a child of each directory.
    ///
    /// The shape a skill library has: a source directory holds one folder per
    /// skill, and the file that makes a folder a skill is the `SKILL.md`
    /// immediately inside it. Neither of the two above fits — the flat form
    /// finds nothing, because no `SKILL.md` sits at the root, and the recursive
    /// form walks every `scripts/`, `references/` and `assets/` directory of
    /// every skill installed, on every message, to look for a file that is
    /// never in one.
    ///
    /// So this reads exactly what the scan it guards reads: the roots' children,
    /// one level, no deeper. A skill added, removed, or renamed changes the set
    /// of paths; a skill *edited* changes its stamp. Neither needs the tree
    /// underneath to be walked at all.
    pub fn of_child_dirs<'a>(dirs: impl IntoIterator<Item = &'a Path>, suffix: &str) -> Self {
        Self::of(
            dirs.into_iter()
                .map(|root| Watched::ChildDirs {
                    root: root.to_path_buf(),
                    suffix: suffix.to_string(),
                })
                .collect(),
        )
    }

    /// Both fingerprints as one.
    ///
    /// Config is rarely all of one shape: a brief is six named files plus
    /// whatever they import plus a directory of scoped ones, and it is the
    /// whole of that which decides whether re-reading would produce anything
    /// different.
    pub fn and(mut self, other: Self) -> Self {
        self.watched.extend(other.watched);
        self.stamps.extend(other.stamps);
        self
    }

    /// The same files as they stand now.
    ///
    /// What makes a fingerprint comparable to itself. The set of files a piece
    /// of config depends on is only partly knowable in advance — instructions
    /// find their imports by being read — so the check re-states the list the
    /// last read produced rather than rebuilding it from scratch. Directories
    /// are re-scanned rather than re-stated, which is what makes a file that
    /// did not exist last time still get noticed.
    pub fn refreshed(&self) -> Self {
        Self::of(self.watched.clone())
    }

    fn of(watched: Vec<Watched>) -> Self {
        let stamps = watched.iter().flat_map(Watched::stamps).collect();
        Self { watched, stamps }
    }
}

impl Watched {
    fn stamps(&self) -> Vec<(PathBuf, Stat)> {
        match self {
            Self::File(path) => vec![(path.clone(), stat(path))],
            Self::Dir {
                root,
                suffix,
                recursive,
            } => {
                let mut found = Vec::new();
                collect(root, suffix, *recursive, &mut found);
                stamped(found)
            }
            Self::ChildDirs { root, suffix } => {
                let mut found = Vec::new();
                if let Ok(entries) = std::fs::read_dir(root) {
                    for entry in entries.flatten() {
                        let child = entry.path();
                        if child.is_dir() {
                            collect(&child, suffix, false, &mut found);
                        }
                    }
                }
                stamped(found)
            }
        }
    }
}

/// A set of paths as the sorted, stat-ed pairs a fingerprint is made of.
///
/// The sort is not cosmetic: a directory listing's order is the filesystem's
/// business, and two reads of an unchanged directory have to compare equal.
fn stamped(mut found: Vec<PathBuf>) -> Vec<(PathBuf, Stat)> {
    found.sort();
    found
        .into_iter()
        .map(|path| {
            let stamp = stat(&path);
            (path, stamp)
        })
        .collect()
}

/// Every matching file under `root`, which may not be there at all.
///
/// A missing directory contributes nothing rather than failing: `~/.copilot`
/// does not exist until someone installs Copilot, and the entries appearing
/// later is itself the change that gets noticed.
fn collect(root: &Path, suffix: &str, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                if recursive {
                    collect(&path, suffix, recursive, out);
                }
            }
            _ => {
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(suffix))
                {
                    out.push(path);
                }
            }
        }
    }
}

/// `None` for a file that is not there, which is a state and not a failure.
fn stat(path: &Path) -> Stat {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    /// Writes a file, creating its parent.
    ///
    /// Every test here rewrites a file to a *different length*, which is a
    /// requirement rather than a habit — see the module note. A same-length
    /// rewrite within one filesystem tick is invisible by design, so content of
    /// equal length would turn these into tests of the host's timestamp
    /// resolution: passing on a nanosecond filesystem and failing on CI.
    fn write(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn of(dir: &Path, names: &[&str]) -> Freshness {
        let paths: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
        Freshness::of_files(paths.iter().map(PathBuf::as_path))
    }

    #[test]
    fn nothing_moving_compares_equal() {
        // The whole point. A fingerprint that differed from itself would make
        // the gate it guards permanently open, which is the same as not having
        // one — and costs a full re-read on every message.
        let dir = TempDir::new().unwrap();
        write(dir.path(), "AGENTS.md", "Use tabs.\n");

        assert_eq!(
            of(dir.path(), &["AGENTS.md"]),
            of(dir.path(), &["AGENTS.md"])
        );
    }

    #[test]
    fn an_edit_compares_different() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "AGENTS.md", "Use tabs.\n");
        let before = of(dir.path(), &["AGENTS.md"]);

        write(dir.path(), "AGENTS.md", "Use spaces, always.\n");
        assert_ne!(before, of(dir.path(), &["AGENTS.md"]));
    }

    #[test]
    fn a_file_appearing_compares_different() {
        // Absence is a state, not a gap in the record. A workspace that had no
        // brief and now has one is the first moment the feature does anything
        // for that project, and a fingerprint listing only what it found would
        // read the same before and after.
        let dir = TempDir::new().unwrap();
        let before = of(dir.path(), &["AGENTS.md"]);

        write(dir.path(), "AGENTS.md", "Ship it.\n");
        assert_ne!(before, of(dir.path(), &["AGENTS.md"]));
    }

    #[test]
    fn a_file_vanishing_compares_different() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "AGENTS.md", "Use tabs.\n");
        let before = of(dir.path(), &["AGENTS.md"]);

        std::fs::remove_file(dir.path().join("AGENTS.md")).unwrap();
        assert_ne!(before, of(dir.path(), &["AGENTS.md"]));
    }

    #[test]
    fn refreshing_re_stats_the_same_files() {
        // What makes a fingerprint comparable to itself over time: the set of
        // files is carried forward rather than rebuilt, because the caller
        // often cannot rebuild it — an instruction file's imports are only
        // knowable from having read it.
        let dir = TempDir::new().unwrap();
        write(dir.path(), "RULES.md", "Use tabs.\n");
        let before = of(dir.path(), &["RULES.md"]);
        assert_eq!(before, before.refreshed());

        write(dir.path(), "RULES.md", "Use spaces, always.\n");
        assert_ne!(before, before.refreshed());
    }

    #[test]
    fn an_agent_directory_notices_a_new_file() {
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let before = Freshness::of_dirs([agents.as_path()], ".md", false);

        write(&agents, "reviewer.md", "---\nname: reviewer\n---\n");
        assert_ne!(before, Freshness::of_dirs([agents.as_path()], ".md", false));
    }

    #[test]
    fn an_agent_directory_ignores_what_the_scan_ignores() {
        // A fingerprint that covered more than the scan reads would rescan for
        // a file the roster can never contain — every message, forever, for a
        // stray note in the folder.
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        write(&agents, "reviewer.md", "---\nname: reviewer\n---\n");
        let before = Freshness::of_dirs([agents.as_path()], ".md", false);

        write(&agents, "notes.txt", "not an agent");
        write(&agents, ".DS_Store", "junk");
        assert_eq!(before, Freshness::of_dirs([agents.as_path()], ".md", false));
    }

    #[test]
    fn an_agent_directory_that_is_not_there_yet_is_noticed_when_it_arrives() {
        // `~/.taurus/agents` does not exist until someone writes their first
        // agent, so "missing" has to be an ordinary state that later changes.
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        let before = Freshness::of_dirs([agents.as_path()], ".md", false);
        assert_eq!(before, Freshness::of_dirs([agents.as_path()], ".md", false));

        write(&agents, "reviewer.md", "---\nname: reviewer\n---\n");
        assert_ne!(before, Freshness::of_dirs([agents.as_path()], ".md", false));
    }

    #[test]
    fn two_reads_of_one_directory_agree_however_it_was_listed() {
        // read_dir yields in whatever order the filesystem feels like. Unsorted,
        // this would compare unequal at random and rescan on a coin flip.
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        for name in ["c.md", "a.md", "b.md", "d.md", "e.md"] {
            write(&agents, name, "---\nname: x\n---\n");
        }

        assert_eq!(
            Freshness::of_dirs([agents.as_path()], ".md", false),
            Freshness::of_dirs([agents.as_path()], ".md", false)
        );
    }
    #[test]
    fn a_skill_library_notices_a_folder_appearing_one_level_down() {
        // The layout every skill library uses: a source directory of folders,
        // each holding the file that makes it a skill. Neither of the other
        // two shapes sees it — flat looks only at the root, recursive walks
        // every asset directory of every skill to find it.
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let before = Freshness::of_child_dirs([root.as_path()], "SKILL.md");

        write(&root, "summarize/SKILL.md", "---\nname: summarize\n---\n");

        assert_ne!(before, before.refreshed(), "a new skill was not noticed");
    }

    #[test]
    fn an_edited_skill_is_noticed_and_a_rewritten_asset_is_not() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("skills");
        write(&root, "summarize/SKILL.md", "---\nname: summarize\n---\n");
        write(&root, "summarize/scripts/run.py", "print(1)\n");
        let before = Freshness::of_child_dirs([root.as_path()], "SKILL.md");

        // What the scan behind this fingerprint actually reads.
        write(
            &root,
            "summarize/SKILL.md",
            "---\nname: summarize\ndescription: x\n---\n",
        );
        assert_ne!(
            before,
            before.refreshed(),
            "an edited SKILL.md was not noticed"
        );

        // And what it does not: a script is read when it is run, not when the
        // catalog is built, so a fingerprint that moved on this would rescan
        // every skill in the library for nothing.
        let after = Freshness::of_child_dirs([root.as_path()], "SKILL.md");
        write(&root, "summarize/scripts/run.py", "print(1); print(2)\n");
        assert_eq!(after, after.refreshed(), "an edited script forced a rescan");
    }

    #[test]
    fn a_removed_skill_is_noticed() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("skills");
        write(&root, "doomed/SKILL.md", "---\nname: doomed\n---\n");
        let before = Freshness::of_child_dirs([root.as_path()], "SKILL.md");

        std::fs::remove_dir_all(root.join("doomed")).unwrap();

        assert_ne!(
            before,
            before.refreshed(),
            "a deleted skill was not noticed"
        );
    }

    #[test]
    fn a_skill_directory_that_does_not_exist_yet_is_still_watched() {
        // `.taurus/skills` is created by the first person to write a skill into
        // it. A fingerprint that could only watch directories already there
        // would never notice that first one.
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("never-created-yet");
        let before = Freshness::of_child_dirs([root.as_path()], "SKILL.md");
        assert_eq!(before, before.refreshed());

        write(&root, "first/SKILL.md", "---\nname: first\n---\n");

        assert_ne!(before, before.refreshed());
    }
}
