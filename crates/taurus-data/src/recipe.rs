//! A recipe: a named, committed, re-runnable chain of SQL steps.
//!
//! # Why this is a file and not a conversation
//!
//! `query_data` answers a question. A recipe answers it the same way next
//! month, on next month's export, without anybody remembering what was decided
//! — which is the difference between having looked at some data and having a
//! dataset. So a recipe is a file in the project, next to the code, reviewed in
//! a diff and committed like anything else that decides what the software does.
//!
//! That is also why it is SQL rather than a description of SQL. The steps are
//! the thing being reviewed; a format that wrapped them in JSON would put an
//! escaping layer between the reviewer and the only part that matters, and
//! `SELECT DISTINCT * FROM input` pasted into any client is worth more than a
//! representation only this program can read.
//!
//! # The shape of one
//!
//! ```sql
//! ---
//! source: data/interactions.csv
//! output: data/interactions_clean.parquet
//! ---
//!
//! -- step: drop exact duplicates
//! SELECT DISTINCT * FROM input
//!
//! -- step: keep only the rows that name a user
//! SELECT * FROM input WHERE user_id IS NOT NULL
//! ```
//!
//! YAML frontmatter and a body, which is how a `SKILL.md` is authored — the
//! same shape for the same reason, so somebody who has written one of those
//! already knows how to write this.
//!
//! # `input`, and what else a step can see
//!
//! Every step reads from **`input`**, which is the previous step's rows. For
//! the first step it is the `source` dataset.
//!
//! Every *other* loaded dataset is also in scope, under the name
//! `load_dataset` gave it. That is what makes a recipe more than a filter: a
//! step can join the rows being cleaned against a lookup table, and enrichment
//! is the ordinary case rather than a special one. `source` stays reachable
//! under its own name too, so a later step can compare what is left against
//! what it started with.
//!
//! `input` is therefore reserved. A dataset actually called `input` is shadowed
//! inside a recipe, which is the one collision this arrangement can produce and
//! is worth knowing rather than worth forbidding.
//!
//! # Why a table can be named by path
//!
//! `source:` takes either a loaded dataset's name or a path to a file, and
//! `tables:` binds extra names to extra files. That looks like two ways of
//! saying one thing and it is not: the dataset list lives in the harness's
//! config home and is not committed, so a recipe that could only name loaded
//! datasets would be a committed file that does nothing on a fresh clone until
//! somebody works out which files to load first. [`crate::catalog`] flagged
//! this exact trade when it chose where to keep the list, and said the answer
//! would have to change the moment a recipe existed. This is that answer.
//!
//! Anything with a data extension is a path; anything else is a name. A path
//! goes through the workspace guard like every other, so a committed recipe
//! cannot reach out of the project it is committed to.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a workspace keeps its recipes.
///
/// Inside `.taurus`, with the skills, rather than somewhere of its own. A
/// recipe is the same kind of thing a skill is — a committed instruction the
/// agent follows — and `.gitignore` un-ignores this directory for the same
/// stated reason it un-ignores `skills/`.
pub const RECIPE_DIR: &str = ".taurus/recipes";

/// The name every step reads from. See the note at the top of this file.
pub const INPUT: &str = "input";

/// A name a recipe binds to a file of its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "RecipeTable")]
pub struct Binding {
    /// What the steps call it.
    pub name: String,
    /// Workspace-relative path to the file behind it.
    pub path: String,
}

/// One step, as written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "RecipeStep")]
pub struct Step {
    /// What the `-- step:` line said, or `step 2` where it said nothing.
    pub title: String,
    /// The statement, with its marker line and any trailing semicolon removed.
    pub sql: String,
}

/// What the frontmatter declares.
///
/// Unknown fields are refused rather than ignored. A `name:` that did nothing
/// would be somebody believing they had renamed their recipe, and the failure
/// would arrive as "no recipe called that" from a completely different place.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    source: String,
    output: String,
    #[serde(default)]
    description: Option<String>,
    /// Extra tables the steps may name, as `name: path`.
    #[serde(default)]
    tables: std::collections::BTreeMap<String, String>,
}

/// A parsed recipe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "Recipe")]
pub struct Recipe {
    /// The filename without its extension, and nothing else.
    ///
    /// Not a frontmatter field. Two places to write a name is two places to
    /// disagree, and the one somebody types on a command line is the file's.
    pub name: String,
    /// The dataset the first step's `input` is bound to.
    pub source: String,
    /// Where the last step's rows are written, workspace-relative.
    pub output: String,
    /// One line about what it does, for a list. Optional.
    pub description: Option<String>,
    /// Extra tables the steps may name, beyond `source` and whatever the
    /// workspace has loaded. Sorted by name, so a listing does not reshuffle.
    pub tables: Vec<Binding>,
    pub steps: Vec<Step>,
    /// Workspace-relative path of the recipe file itself.
    pub path: String,
}

impl Recipe {
    /// What the output file's format will be, from its extension.
    pub fn format(&self) -> Result<crate::engine::Format, RecipeError> {
        crate::engine::Format::of(Path::new(&self.output)).map_err(|_| RecipeError::Invalid {
            path: self.path.clone(),
            message: format!(
                "`output: {}` does not end in a format this can write. Use .parquet — it keeps \
                 the column types — or .csv, .tsv, or .ndjson.",
                self.output
            ),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RecipeError {
    #[error(
        "{path}: a recipe starts with a --- line, then `source:` and `output:`, then another --- \
         line, then its steps."
    )]
    NoFrontmatter { path: String },

    #[error("{path}: the frontmatter is not valid YAML: {source}")]
    BadYaml {
        path: String,
        #[source]
        source: serde_yaml_ng::Error,
    },

    #[error("{path}: {message}")]
    Invalid { path: String, message: String },

    #[error("no recipe called '{name}'. {available}")]
    NotFound { name: String, available: String },

    #[error(
        "'{raw}' cannot name a recipe: a recipe is a file in {RECIPE_DIR}, so its name is letters, \
         digits, dashes and underscores and nothing else."
    )]
    BadName { raw: String },

    #[error("could not read {path}: {detail}")]
    Unreadable { path: String, detail: String },
}

/// Every recipe in a workspace, by name.
///
/// A recipe that will not parse is left out of the list and reported beside it,
/// rather than failing the listing. One torn file should cost the reader that
/// file, not the other four — the same call [`crate::catalog::load`] makes, for
/// the same reason.
pub fn load(workspace: &Path) -> (Vec<Recipe>, Vec<String>) {
    let dir = workspace.join(RECIPE_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (Vec::new(), Vec::new());
    };

    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("sql") {
                return None;
            }
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .collect();
    // Alphabetical, because a directory listing's own order differs between
    // filesystems and a list that reshuffles between two looks is one nobody
    // reads twice.
    names.sort();

    let mut recipes = Vec::new();
    let mut problems = Vec::new();
    for name in names {
        match find(workspace, &name) {
            Ok(recipe) => recipes.push(recipe),
            Err(error) => problems.push(error.to_string()),
        }
    }
    (recipes, problems)
}

/// One recipe by name, or the error that lists the others.
pub fn find(workspace: &Path, name: &str) -> Result<Recipe, RecipeError> {
    // Before the join, not after. The name arrives from a tool argument and a
    // command line, and `../../etc/passwd` is a path rather than a recipe —
    // resolving it and then checking would already have decided which file to
    // open.
    if !is_name(name) {
        return Err(RecipeError::BadName {
            raw: name.to_string(),
        });
    }

    let relative = format!("{RECIPE_DIR}/{name}.sql");
    let path = workspace.join(&relative);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            let (all, _) = load(workspace);
            RecipeError::NotFound {
                name: name.to_string(),
                available: available(&all),
            }
        } else {
            RecipeError::Unreadable {
                path: relative.clone(),
                detail: error.to_string(),
            }
        }
    })?;

    parse(&text, name, &relative)
}

/// What to say after "no recipe called x".
fn available(all: &[Recipe]) -> String {
    if all.is_empty() {
        return format!(
            "This workspace has none yet. A recipe is a .sql file in {RECIPE_DIR} with \
             `source:` and `output:` in a --- frontmatter block and its steps below, each under a \
             `-- step: what it does` line."
        );
    }
    let names: Vec<&str> = all.iter().map(|r| r.name.as_str()).collect();
    format!("Recipes here: {}.", names.join(", "))
}

/// Whether a string can be a recipe's filename.
fn is_name(raw: &str) -> bool {
    !raw.is_empty()
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Splits `---\nyaml\n---\nsteps` and reads both halves.
pub fn parse(text: &str, name: &str, path: &str) -> Result<Recipe, RecipeError> {
    // The same tolerances `parse_skill_md` extends, and for the same reason:
    // these are authored by models and edited by people on three platforms.
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| RecipeError::NoFrontmatter {
            path: path.to_string(),
        })?;
    let (yaml, body) = rest
        .split_once("\n---\n")
        .or_else(|| rest.split_once("\n---"))
        .ok_or_else(|| RecipeError::NoFrontmatter {
            path: path.to_string(),
        })?;

    let frontmatter: Frontmatter =
        serde_yaml_ng::from_str(yaml).map_err(|source| RecipeError::BadYaml {
            path: path.to_string(),
            source,
        })?;

    let invalid = |message: String| RecipeError::Invalid {
        path: path.to_string(),
        message,
    };

    let source = frontmatter.source.trim().to_string();
    if source.is_empty() {
        return Err(invalid(
            "`source:` is empty. It names what the first step reads — a file, like \
             data/events.csv, or a loaded dataset by name. Naming the file is what makes the \
             recipe work on a fresh clone."
                .into(),
        ));
    }
    let output = frontmatter.output.trim().to_string();
    if output.is_empty() {
        return Err(invalid(
            "`output:` is empty. It is where the last step's rows are written, relative to the \
             workspace — something like data/clean.parquet."
                .into(),
        ));
    }

    let mut tables = Vec::with_capacity(frontmatter.tables.len());
    for (table, file) in frontmatter.tables {
        let table = table.trim().to_string();
        let file = file.trim().to_string();
        if table == INPUT {
            return Err(invalid(format!(
                "`tables:` binds `{INPUT}`, which is the name every step reads the previous step \
                 from. Call it something else."
            )));
        }
        if file.is_empty() {
            return Err(invalid(format!(
                "`tables:` binds `{table}` to nothing. Each entry is `name: path/to/file.csv`."
            )));
        }
        // Checked here so a typo in an extension is reported when the file is
        // read, not after a scan has already run.
        crate::engine::Format::of(Path::new(&file)).map_err(|_| {
            invalid(format!(
                "`tables:` binds `{table}` to {file}, which is not a file this can read. It reads \
                 .csv, .tsv, .parquet, and newline-delimited .ndjson/.jsonl/.json."
            ))
        })?;
        tables.push(Binding {
            name: table,
            path: file,
        });
    }

    let steps = steps(body, &invalid)?;

    let recipe = Recipe {
        name: name.to_string(),
        source,
        output,
        description: frontmatter
            .description
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        tables,
        steps,
        path: path.to_string(),
    };
    // Checked here rather than at run time, so a recipe with an unwritable
    // output says so when it is read — in a listing, in the pane, in the CLI —
    // rather than after a scan of a large file has already happened.
    recipe.format()?;
    Ok(recipe)
}

/// The body, cut at its `-- step:` markers.
fn steps(body: &str, invalid: &dyn Fn(String) -> RecipeError) -> Result<Vec<Step>, RecipeError> {
    let mut steps: Vec<Step> = Vec::new();
    // Text before the first marker. Blank and comment lines are the ordinary
    // gap between the frontmatter and the first step; anything else is SQL
    // nobody labelled, which is a step that would silently not run.
    let mut preamble = String::new();

    for line in body.lines() {
        match marker(line) {
            Some(title) => {
                let title = if title.is_empty() {
                    format!("step {}", steps.len() + 1)
                } else {
                    title
                };
                steps.push(Step {
                    title,
                    sql: String::new(),
                });
            }
            None => match steps.last_mut() {
                Some(step) => {
                    step.sql.push_str(line);
                    step.sql.push('\n');
                }
                None => {
                    preamble.push_str(line);
                    preamble.push('\n');
                }
            },
        }
    }

    let stray = preamble
        .lines()
        .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with("--"));
    if let Some(stray) = stray {
        return Err(invalid(format!(
            "there is SQL before the first step: `{}`. Every step needs a `-- step: what it does` \
             line above it, which is what names it in the run.",
            stray.trim()
        )));
    }

    for step in &mut steps {
        // A trailing semicolon goes here. It is how everybody writes SQL and it
        // is not part of the statement being planned — leaving it on would make
        // a correct recipe fail on a punctuation mark.
        step.sql = step.sql.trim().trim_end_matches(';').trim_end().to_string();
        if step.sql.is_empty() {
            return Err(invalid(format!(
                "the step `{}` has no SQL under it. Remove the line, or give it a statement — a \
                 step that does nothing still costs a pass over the data.",
                step.title
            )));
        }
    }

    if steps.is_empty() {
        return Err(invalid(format!(
            "there are no steps. A step is a `-- step: what it does` line with one SELECT under \
             it, reading from `{INPUT}`."
        )));
    }
    Ok(steps)
}

/// The title on a `-- step:` line, if this is one.
///
/// Lenient about spacing and case, because this is punctuation rather than
/// syntax: `--step:`, `-- Step:`, and `--   step :` are all somebody writing
/// the same thing.
fn marker(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("--")?.trim_start();
    let (word, rest) = rest.split_at(rest.char_indices().nth(4).map(|(i, _)| i)?);
    if !word.eq_ignore_ascii_case("step") {
        return None;
    }
    Some(rest.trim_start().strip_prefix(':')?.trim().to_string())
}

/// Whether a `source:` names a file rather than a loaded dataset.
///
/// One rule, and a rule somebody can hold in their head: anything with a data
/// extension is a path, anything else is a name. `interactions` is a dataset,
/// `data/interactions.csv` and `interactions.csv` are files.
pub fn is_path(source: &str) -> bool {
    crate::engine::Format::of(Path::new(source)).is_ok()
}

/// Every table a recipe's steps can see, and which one `input` starts as.
///
/// Three layers, each overriding the one before it, and the order is the
/// argument:
///
/// 1. Whatever the workspace has loaded, so a step can join against something
///    somebody was already looking at.
/// 2. The recipe's own `tables:`, because a committed file saying which files
///    it reads is more trustworthy than whatever happens to be loaded today —
///    if both define `items`, the recipe means the one it named.
/// 3. The `source:`, when it is a path rather than a name.
///
/// Paths go through the workspace guard rather than a join, for the reason
/// every path in this crate does and one more: this one comes out of a file
/// that arrived with a `git pull`.
pub fn resolve(
    recipe: &Recipe,
    workspace: &Path,
    loaded: Vec<(String, crate::engine::Source)>,
) -> Result<(Vec<(String, crate::engine::Source)>, String), crate::engine::DataError> {
    let mut tables = loaded;
    let mut put = |name: String, path: &str| -> Result<(), crate::engine::DataError> {
        let resolved = taurus_tools::path_guard::resolve(workspace, path)
            .map_err(|e| crate::engine::DataError::Failed(format!("{}: {e}", recipe.path)))?;
        if !resolved.is_file() {
            return Err(crate::engine::DataError::Failed(format!(
                "{} reads {path}, which is not a file in this workspace.",
                recipe.path
            )));
        }
        let source = crate::engine::Source::at(resolved)?;
        match tables.iter_mut().find(|(existing, _)| *existing == name) {
            Some(entry) => entry.1 = source,
            None => tables.push((name, source)),
        }
        Ok(())
    };

    for binding in &recipe.tables {
        put(binding.name.clone(), &binding.path)?;
    }

    let start = if is_path(&recipe.source) {
        let name = crate::catalog::suggest_name(Path::new(&recipe.source));
        put(name.clone(), &recipe.source)?;
        name
    } else {
        recipe.source.clone()
    };

    if !tables.iter().any(|(name, _)| *name == start) {
        return Err(crate::engine::DataError::NoSuchDataset {
            name: start,
            available: if tables.is_empty() {
                "Nothing is loaded in this workspace. A recipe can also name a file directly — \
                 `source: data/events.csv` — which is what makes one work on a fresh clone."
                    .into()
            } else {
                format!(
                    "Loaded here: {}. A recipe can also name a file directly — `source: \
                     data/events.csv`.",
                    tables
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        });
    }
    Ok((tables, start))
}

/// Where a recipe file goes, workspace-relative, for a name.
pub fn path_for(name: &str) -> PathBuf {
    PathBuf::from(RECIPE_DIR).join(format!("{name}.sql"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const GOOD: &str = "---\n\
        source: interactions\n\
        output: data/clean.parquet\n\
        description: drop duplicates and the rows with no user\n\
        ---\n\
        \n\
        -- step: drop exact duplicates\n\
        SELECT DISTINCT * FROM input\n\
        \n\
        -- step: keep only the rows that name a user\n\
        SELECT * FROM input WHERE user_id IS NOT NULL;\n";

    fn parsed(text: &str) -> Result<Recipe, RecipeError> {
        parse(text, "clean", ".taurus/recipes/clean.sql")
    }

    #[test]
    fn a_recipe_reads_its_header_and_its_steps() {
        let recipe = parsed(GOOD).unwrap();
        assert_eq!(recipe.name, "clean");
        assert_eq!(recipe.source, "interactions");
        assert_eq!(recipe.output, "data/clean.parquet");
        assert_eq!(
            recipe.description.as_deref(),
            Some("drop duplicates and the rows with no user")
        );
        assert_eq!(recipe.steps.len(), 2);
        assert_eq!(recipe.steps[0].title, "drop exact duplicates");
        assert_eq!(recipe.steps[0].sql, "SELECT DISTINCT * FROM input");
    }

    /// Everybody ends a statement with one and it is not part of the statement.
    #[test]
    fn a_trailing_semicolon_is_not_part_of_the_step() {
        let recipe = parsed(GOOD).unwrap();
        assert_eq!(
            recipe.steps[1].sql,
            "SELECT * FROM input WHERE user_id IS NOT NULL"
        );
    }

    #[test]
    fn the_name_is_the_filename_and_the_frontmatter_cannot_argue_with_it() {
        let text = GOOD.replace("source:", "name: something_else\nsource:");
        let message = parsed(&text).unwrap_err().to_string();
        // A `name:` that was ignored would be somebody believing they had
        // renamed their recipe.
        assert!(message.contains("name"), "{message}");
    }

    #[test]
    fn a_file_with_no_frontmatter_is_told_what_one_looks_like() {
        let message = parsed("SELECT 1").unwrap_err().to_string();
        assert!(message.contains("source:"), "{message}");
        assert!(message.contains("output:"), "{message}");
    }

    #[test]
    fn a_recipe_with_no_steps_says_what_a_step_is() {
        let text = "---\nsource: a\noutput: b.parquet\n---\n\n";
        let message = parsed(text).unwrap_err().to_string();
        assert!(message.contains("-- step:"), "{message}");
    }

    /// The case worth an error rather than a shrug: SQL that looks like it runs
    /// and does not.
    #[test]
    fn sql_above_the_first_step_is_refused_and_quoted_back() {
        let text = "---\nsource: a\noutput: b.parquet\n---\n\n\
                    SELECT * FROM input\n\n-- step: one\nSELECT 1\n";
        let message = parsed(text).unwrap_err().to_string();
        assert!(message.contains("SELECT * FROM input"), "{message}");
        assert!(message.contains("-- step:"), "{message}");
    }

    #[test]
    fn a_comment_above_the_first_step_is_ordinary() {
        let text = "---\nsource: a\noutput: b.parquet\n---\n\n\
                    -- this file is generated\n\n-- step: one\nSELECT 1\n";
        assert_eq!(parsed(text).unwrap().steps.len(), 1);
    }

    #[test]
    fn a_step_with_no_sql_under_it_is_refused() {
        let text = "---\nsource: a\noutput: b.parquet\n---\n\n-- step: nothing\n\n";
        assert!(parsed(text).is_err());
    }

    #[test]
    fn a_step_marker_is_read_however_it_is_spaced_or_cased() {
        let text = "---\nsource: a\noutput: b.parquet\n---\n\
                    --Step:one\nSELECT 1\n--  step : two\nSELECT 2\n";
        let recipe = parsed(text).unwrap();
        assert_eq!(recipe.steps.len(), 2);
        assert_eq!(recipe.steps[0].title, "one");
        assert_eq!(recipe.steps[1].title, "two");
    }

    /// An ordinary SQL comment is not a step marker, and a step's own body is
    /// allowed to contain comments.
    #[test]
    fn a_plain_comment_inside_a_step_stays_inside_it() {
        let text = "---\nsource: a\noutput: b.parquet\n---\n\
                    -- step: one\n-- the id is a string here\nSELECT 1\n";
        let recipe = parsed(text).unwrap();
        assert_eq!(recipe.steps.len(), 1);
        assert!(recipe.steps[0].sql.contains("the id is a string"));
    }

    #[test]
    fn an_unnamed_step_is_numbered_rather_than_blank() {
        let text = "---\nsource: a\noutput: b.parquet\n---\n\
                    -- step: first\nSELECT 1\n-- step:\nSELECT 2\n";
        assert_eq!(parsed(text).unwrap().steps[1].title, "step 2");
    }

    #[test]
    fn an_output_this_cannot_write_is_refused_when_the_file_is_read() {
        let text = "---\nsource: a\noutput: data/clean.xlsx\n---\n-- step: one\nSELECT 1\n";
        let message = parsed(text).unwrap_err().to_string();
        assert!(message.contains(".parquet"), "{message}");
    }

    #[test]
    fn a_source_with_a_data_extension_is_a_file_and_anything_else_is_a_name() {
        assert!(is_path("data/interactions.csv"));
        assert!(is_path("interactions.parquet"));
        assert!(!is_path("interactions"));
        assert!(!is_path("user_events_2024"));
    }

    #[test]
    fn a_recipe_can_bind_names_to_files_of_its_own() {
        let text = "---\nsource: data/events.csv\noutput: out.parquet\n\
                    tables:\n  items: data/items.csv\n  users: data/users.parquet\n\
                    ---\n-- step: one\nSELECT 1\n";
        let recipe = parsed(text).unwrap();
        // Sorted, so a listing does not reshuffle between two looks.
        assert_eq!(
            recipe.tables,
            vec![
                Binding {
                    name: "items".into(),
                    path: "data/items.csv".into()
                },
                Binding {
                    name: "users".into(),
                    path: "data/users.parquet".into()
                },
            ]
        );
    }

    #[test]
    fn a_recipe_cannot_bind_the_name_every_step_reads_from() {
        let text = "---\nsource: a\noutput: b.parquet\ntables:\n  input: x.csv\n---\n\
                    -- step: one\nSELECT 1\n";
        let message = parsed(text).unwrap_err().to_string();
        assert!(message.contains("input"), "{message}");
    }

    #[test]
    fn a_bound_table_that_is_not_a_readable_file_is_refused_when_the_recipe_is_read() {
        let text = "---\nsource: a\noutput: b.parquet\ntables:\n  items: notes.txt\n---\n\
                    -- step: one\nSELECT 1\n";
        let message = parsed(text).unwrap_err().to_string();
        assert!(message.contains("items"), "{message}");
        assert!(message.contains(".csv"), "{message}");
    }

    /// The point of the whole arrangement: a committed recipe that names its
    /// own files runs on a fresh clone, where the dataset list is empty
    /// because the dataset list is not committed.
    #[test]
    fn a_recipe_naming_its_own_file_needs_nothing_loaded() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/events.csv"), "a\n1\n").unwrap();

        let text = "---\nsource: data/events.csv\noutput: out.parquet\n---\n\
                    -- step: one\nSELECT * FROM input\n";
        let recipe = parse(text, "clean", ".taurus/recipes/clean.sql").unwrap();

        let (tables, start) = resolve(&recipe, &root, Vec::new()).unwrap();
        assert_eq!(start, "events");
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].0, "events");
    }

    /// A recipe that says which file it means beats whatever is loaded today.
    #[test]
    fn a_recipes_own_binding_wins_over_a_loaded_dataset_of_the_same_name() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/right.csv"), "a\n1\n").unwrap();
        std::fs::write(root.join("data/wrong.csv"), "a\n2\n").unwrap();

        let text = "---\nsource: data/right.csv\noutput: out.parquet\n\
                    tables:\n  items: data/right.csv\n---\n-- step: one\nSELECT 1\n";
        let recipe = parse(text, "clean", ".taurus/recipes/clean.sql").unwrap();

        let loaded = vec![(
            "items".to_string(),
            crate::engine::Source::at(root.join("data/wrong.csv")).unwrap(),
        )];
        let (tables, _) = resolve(&recipe, &root, loaded).unwrap();
        let items = tables.iter().find(|(n, _)| n == "items").unwrap();
        assert!(items.1.path.ends_with("right.csv"), "{:?}", items.1.path);
        // Overridden, not duplicated: two entries under one name would have the
        // engine register whichever came last and nobody could say which.
        assert_eq!(tables.iter().filter(|(n, _)| n == "items").count(), 1);
    }

    /// A path in a committed file is still a path somebody pulled.
    #[test]
    fn a_source_that_climbs_out_of_the_workspace_is_refused() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let text = "---\nsource: ../../etc/passwd.csv\noutput: out.parquet\n---\n\
                    -- step: one\nSELECT 1\n";
        let recipe = parse(text, "clean", ".taurus/recipes/clean.sql").unwrap();
        assert!(resolve(&recipe, &root, Vec::new()).is_err());
    }

    #[test]
    fn a_source_naming_a_dataset_nothing_loaded_says_a_path_would_also_work() {
        let dir = TempDir::new().unwrap();
        let text = "---\nsource: interactions\noutput: out.parquet\n---\n\
                    -- step: one\nSELECT 1\n";
        let recipe = parse(text, "clean", ".taurus/recipes/clean.sql").unwrap();
        let message = resolve(&recipe, dir.path(), Vec::new())
            .unwrap_err()
            .to_string();
        assert!(message.contains("interactions"), "{message}");
        assert!(message.contains("source: data/events.csv"), "{message}");
    }

    #[test]
    fn a_name_that_is_a_path_is_refused_before_anything_is_opened() {
        let dir = TempDir::new().unwrap();
        let message = find(dir.path(), "../../etc/passwd")
            .unwrap_err()
            .to_string();
        assert!(message.contains("letters"), "{message}");
    }

    #[test]
    fn a_workspace_with_no_recipe_directory_has_no_recipes() {
        let dir = TempDir::new().unwrap();
        let (recipes, problems) = load(dir.path());
        assert!(recipes.is_empty());
        assert!(problems.is_empty());
    }

    #[test]
    fn a_missing_recipe_in_an_empty_workspace_says_how_to_write_one() {
        let dir = TempDir::new().unwrap();
        let message = find(dir.path(), "clean").unwrap_err().to_string();
        assert!(message.contains(RECIPE_DIR), "{message}");
        assert!(message.contains("-- step:"), "{message}");
    }

    #[test]
    fn a_missing_recipe_lists_the_ones_that_do_exist() {
        let dir = TempDir::new().unwrap();
        write(&dir, "clean", GOOD);
        write(&dir, "enrich", GOOD);
        let message = find(dir.path(), "clen").unwrap_err().to_string();
        assert!(message.contains("clean, enrich"), "{message}");
    }

    #[test]
    fn recipes_are_listed_alphabetically_whatever_order_the_filesystem_gives() {
        let dir = TempDir::new().unwrap();
        write(&dir, "zeta", GOOD);
        write(&dir, "alpha", GOOD);
        let (recipes, _) = load(dir.path());
        let names: Vec<&str> = recipes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["alpha", "zeta"]);
    }

    /// One torn file should cost the reader that file and not the others.
    #[test]
    fn a_recipe_that_will_not_parse_is_reported_beside_the_ones_that_do() {
        let dir = TempDir::new().unwrap();
        write(&dir, "good", GOOD);
        write(&dir, "torn", "not a recipe at all");
        let (recipes, problems) = load(dir.path());
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].name, "good");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("torn.sql"), "{:?}", problems);
    }

    #[test]
    fn a_file_that_is_not_sql_is_not_a_recipe() {
        let dir = TempDir::new().unwrap();
        write(&dir, "good", GOOD);
        std::fs::write(dir.path().join(RECIPE_DIR).join("README.md"), "hello").unwrap();
        let (recipes, problems) = load(dir.path());
        assert_eq!(recipes.len(), 1);
        assert!(problems.is_empty());
    }

    fn write(dir: &TempDir, name: &str, text: &str) {
        let recipes = dir.path().join(RECIPE_DIR);
        std::fs::create_dir_all(&recipes).unwrap();
        std::fs::write(recipes.join(format!("{name}.sql")), text).unwrap();
    }
}
