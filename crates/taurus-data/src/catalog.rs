//! What datasets a workspace has, and where that list is kept.
//!
//! # Why this is not in the project
//!
//! It sits in the harness's own config home, keyed by workspace, beside the
//! session transcripts and the search index — not in the workspace's `.taurus`
//! directory with `permissions.json` and `settings.json`.
//!
//! That is the same call [`taurus_host::memory`] makes for notes, and for the
//! same reason: this is a record of what somebody has been working on, not a
//! setting they are configuring. Putting it in the project would make loading a
//! dataset a write to a file the user is protecting, which means a permission
//! prompt in front of the most ordinary action this feature has — and then a
//! diff in the Changes drawer, and a line in the commit, for having *looked* at
//! a CSV.
//!
//! The cost is honest and worth naming: the list does not travel with the
//! repository, so a teammate who clones it has to load the files again. That is
//! the right trade while an entry is only a pointer. It stops being the right
//! trade the moment an entry carries a recipe — a recipe is exactly the thing
//! you want committed — which is why this is written down here rather than
//! assumed to be settled.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::engine::{DataError, Format};

/// The file, under [`data_dir`].
const CATALOG_FILE: &str = "datasets.json";

/// A dataset this workspace knows about.
///
/// A pointer and nothing more. The rows are in the file, the shape is whatever
/// the file says today, and the profile is computed when asked — so an entry
/// cannot go stale in the one way a cached summary would, which is by being
/// quietly wrong about a file somebody rewrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "Dataset")]
pub struct Dataset {
    /// How everything else names it: the tools, the pane, the transcript card.
    ///
    /// Unique within a workspace, lowercase, and derived from the filename
    /// unless the caller said otherwise. See [`suggest_name`].
    pub name: String,
    /// Workspace-relative, with forward slashes, as [`taurus_tools`] displays
    /// every other path.
    pub path: String,
    pub format: Format,
}

/// Where one workspace's dataset list lives.
///
/// Takes the home and the key rather than working them out, because working
/// them out lives in `taurus-host` and this crate sits below it — the same
/// arrangement [`taurus_index::index_dir`] uses.
pub fn data_dir(home: &Path, workspace_key: &str) -> PathBuf {
    home.join("data").join(workspace_key)
}

/// Every dataset in this workspace, in the order they were loaded.
///
/// A file that will not parse reads as an empty list rather than an error. The
/// list is rebuilt by loading the files again, which is cheap and obvious;
/// refusing to open the pane because one line is torn would be a worse answer
/// than showing nothing and letting somebody load again.
pub fn load(dir: &Path) -> Vec<Dataset> {
    let Ok(text) = std::fs::read_to_string(dir.join(CATALOG_FILE)) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "the dataset list would not parse; starting from an empty one");
        Vec::new()
    })
}

/// One dataset by name, or the error that names it.
///
/// Returns the error rather than an `Option` because every caller turns a miss
/// into exactly this message, and one of them would eventually phrase it
/// differently.
pub fn find(dir: &Path, name: &str) -> Result<Dataset, DataError> {
    let all = load(dir);
    all.iter()
        .find(|d| d.name == name)
        .cloned()
        .ok_or_else(|| DataError::NoSuchDataset {
            name: name.to_string(),
            available: available(&all),
        })
}

/// What to say after "no dataset named x".
///
/// The list, not just the refusal. A model that guessed a name is one line
/// away from the right one, and the alternative is that it guesses again.
fn available(all: &[Dataset]) -> String {
    if all.is_empty() {
        return "Nothing is loaded in this workspace yet — load_dataset takes a file path.".into();
    }
    let names: Vec<&str> = all.iter().map(|d| d.name.as_str()).collect();
    format!("Loaded here: {}.", names.join(", "))
}

/// Adds a dataset, or replaces the one already under that name.
///
/// Replacing rather than refusing: loading the same file twice is something
/// somebody does after editing it, and an error saying it is already there
/// would be the harness disagreeing with an action that had no other reading.
/// Re-loading a *different* file under a name already taken is the case
/// [`suggest_name`] keeps from arising in the first place.
pub fn register(dir: &Path, dataset: Dataset) -> Result<(), DataError> {
    let mut all = load(dir);
    match all.iter_mut().find(|d| d.name == dataset.name) {
        Some(existing) => *existing = dataset,
        None => all.push(dataset),
    }
    save(dir, &all)
}

/// Removes a dataset, reporting whether there was one to remove.
///
/// The file it pointed at is untouched, and that is the whole of what this
/// does. Nothing here deletes anything in the workspace — a list of pointers
/// that could delete its targets would make forgetting a dataset a destructive
/// act, which is not how a list should behave.
pub fn forget(dir: &Path, name: &str) -> Result<bool, DataError> {
    let mut all = load(dir);
    let before = all.len();
    all.retain(|d| d.name != name);
    if all.len() == before {
        return Ok(false);
    }
    save(dir, &all)?;
    Ok(true)
}

/// Writes the list, whole, through a temporary file.
///
/// Temp-and-rename rather than a plain overwrite, so an interrupted write
/// leaves the previous list rather than half of the new one. A rename within
/// one directory is atomic on every platform this ships to.
fn save(dir: &Path, all: &[Dataset]) -> Result<(), DataError> {
    let path = dir.join(CATALOG_FILE);
    let unsaved = |detail: String| DataError::NotSaved {
        path: path.display().to_string(),
        detail,
    };

    std::fs::create_dir_all(dir).map_err(|e| unsaved(e.to_string()))?;
    let text = serde_json::to_string_pretty(all).map_err(|e| unsaved(e.to_string()))?;
    let temporary = dir.join(format!("{CATALOG_FILE}.tmp"));
    std::fs::write(&temporary, text).map_err(|e| unsaved(e.to_string()))?;
    std::fs::rename(&temporary, &path).map_err(|e| unsaved(e.to_string()))
}

/// What a file will be called, from the filename alone.
///
/// Derived rather than invented because the filename is what somebody will say
/// out loud, and because a name the model makes up is a name it will get wrong
/// on the next turn. Reduced to lowercase word characters so it survives being
/// typed into a tool argument.
///
/// **A pure function of the path, and that is load-bearing.** It is called
/// twice for one tool call, from two places that cannot see the same things: by
/// the tool as it runs, and by [`taurus_tools::Tool::view`] beforehand, which
/// is handed the raw input and nothing else — no workspace, no catalog, no
/// disk. A version of this that deduplicated against what was already loaded
/// would answer differently in the two places, and the transcript card would
/// point at a dataset by a name nothing had.
///
/// So a collision is not resolved here. `train/data.csv` and `eval/data.csv`
/// both want `data`, and the loader refuses the second rather than quietly
/// making it `data_2` — see [`taken_by`]. That is the better answer anyway: a
/// suffix is a name nobody chose, and being asked to pick one produces `train`
/// and `eval` instead.
pub fn suggest_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let base = sanitize(stem);
    if base.is_empty() {
        "dataset".to_string()
    } else {
        base
    }
}

/// Normalizes a name a caller chose, or says why it cannot be one.
///
/// The same reduction [`suggest_name`] applies, so a name is the same name
/// however it arrived. Refusing outright would be the stricter choice and the
/// worse one: the model passes `"User Events"` and the useful response is a
/// dataset called `user_events`, reported back so the next call uses it.
pub fn normalize_name(raw: &str) -> Result<String, DataError> {
    let name = sanitize(raw);
    if name.is_empty() {
        return Err(DataError::Failed(format!(
            "'{raw}' has no letters or digits in it, so it cannot name a dataset. Use something              like 'events' or 'user_profiles'."
        )));
    }
    Ok(name)
}

/// The dataset already under this name, when it points somewhere else.
///
/// `None` when the name is free, and also when it is held by *this same file* —
/// re-loading a file after editing it is the ordinary reason to load one twice,
/// and refusing that would be the harness arguing with an action that has no
/// other reading.
pub fn taken_by(dir: &Path, name: &str, path: &str) -> Option<Dataset> {
    load(dir)
        .into_iter()
        .find(|d| d.name == name && d.path != path)
}

/// Lowercase, word characters only, no run of underscores longer than one.
fn sanitize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn dataset(name: &str, path: &str) -> Dataset {
        Dataset {
            name: name.into(),
            path: path.into(),
            format: Format::Csv,
        }
    }

    #[test]
    fn a_workspace_with_no_list_yet_has_no_datasets() {
        let dir = TempDir::new().unwrap();
        assert!(load(dir.path()).is_empty());
    }

    #[test]
    fn a_registered_dataset_reads_back() {
        let dir = TempDir::new().unwrap();
        register(dir.path(), dataset("events", "data/events.csv")).unwrap();
        assert_eq!(load(dir.path()), vec![dataset("events", "data/events.csv")]);
    }

    #[test]
    fn loading_the_same_name_again_replaces_rather_than_duplicates() {
        // What happens when somebody edits the file and loads it again. Two
        // rows with one name would make `find` answer whichever came first.
        let dir = TempDir::new().unwrap();
        register(dir.path(), dataset("events", "data/events.csv")).unwrap();
        register(dir.path(), dataset("events", "data/events-v2.csv")).unwrap();
        let all = load(dir.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].path, "data/events-v2.csv");
    }

    #[test]
    fn the_order_things_were_loaded_in_is_kept() {
        let dir = TempDir::new().unwrap();
        register(dir.path(), dataset("a", "a.csv")).unwrap();
        register(dir.path(), dataset("b", "b.csv")).unwrap();
        register(dir.path(), dataset("c", "c.csv")).unwrap();
        let names: Vec<_> = load(dir.path()).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn a_missing_dataset_in_an_empty_workspace_says_how_to_get_one() {
        let dir = TempDir::new().unwrap();
        let message = find(dir.path(), "nope").unwrap_err().to_string();
        assert!(message.contains("nope"), "{message}");
        assert!(message.contains("load_dataset"), "{message}");
    }

    /// The half that matters for a model: a wrong guess should be one line away
    /// from a right one, not an invitation to guess again.
    #[test]
    fn a_missing_dataset_lists_the_ones_that_do_exist() {
        let dir = TempDir::new().unwrap();
        register(dir.path(), dataset("events", "e.csv")).unwrap();
        register(dir.path(), dataset("items", "i.csv")).unwrap();
        let message = find(dir.path(), "event").unwrap_err().to_string();
        assert!(message.contains("events, items"), "{message}");
    }

    #[test]
    fn forgetting_reports_whether_there_was_anything_to_forget() {
        let dir = TempDir::new().unwrap();
        register(dir.path(), dataset("events", "e.csv")).unwrap();
        assert!(forget(dir.path(), "events").unwrap());
        assert!(!forget(dir.path(), "events").unwrap());
        assert!(load(dir.path()).is_empty());
    }

    #[test]
    fn a_torn_list_reads_as_empty_rather_than_failing() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(CATALOG_FILE), "{ not json").unwrap();
        assert!(load(dir.path()).is_empty());
        // And it is recoverable: a register over the top writes a whole file.
        register(dir.path(), dataset("events", "e.csv")).unwrap();
        assert_eq!(load(dir.path()).len(), 1);
    }

    #[test]
    fn a_name_comes_from_the_filename() {
        assert_eq!(suggest_name(Path::new("/w/data/Events.csv")), "events");
        assert_eq!(
            suggest_name(Path::new("/w/user events 2024.parquet")),
            "user_events_2024"
        );
    }

    /// The property the transcript card depends on: the same path gives the
    /// same name from anywhere, with nothing else consulted.
    #[test]
    fn a_name_depends_on_the_path_and_nothing_else() {
        let once = suggest_name(Path::new("/w/train/data.csv"));
        let again = suggest_name(Path::new("/w/train/data.csv"));
        assert_eq!(once, again);
        assert_eq!(once, suggest_name(Path::new("/elsewhere/train/data.csv")));
    }

    #[test]
    fn a_filename_with_nothing_usable_in_it_still_gets_a_name() {
        assert_eq!(suggest_name(Path::new("/w/---.csv")), "dataset");
    }

    #[test]
    fn a_chosen_name_is_reduced_the_same_way_a_derived_one_is() {
        assert_eq!(normalize_name("User Events").unwrap(), "user_events");
        assert_eq!(normalize_name("EVENTS").unwrap(), "events");
    }

    #[test]
    fn a_chosen_name_with_no_word_characters_is_refused_with_an_example() {
        let message = normalize_name("///").unwrap_err().to_string();
        assert!(message.contains("///"), "{message}");
        assert!(message.contains("events"), "{message}");
    }

    #[test]
    fn a_name_held_by_another_file_is_reported_and_one_held_by_the_same_file_is_not() {
        // `train/data.csv` and `eval/data.csv` both want `data`. The first is a
        // collision worth refusing; re-loading the same file after editing it
        // is not a collision at all.
        let dir = TempDir::new().unwrap();
        register(dir.path(), dataset("data", "train/data.csv")).unwrap();
        assert!(taken_by(dir.path(), "data", "eval/data.csv").is_some());
        assert!(taken_by(dir.path(), "data", "train/data.csv").is_none());
        assert!(taken_by(dir.path(), "other", "eval/data.csv").is_none());
    }
}
