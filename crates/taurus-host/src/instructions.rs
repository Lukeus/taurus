//! Standing instructions, read from the files other agents already use.
//!
//! A skill is a procedure the model loads when it needs one. Instructions are
//! the opposite: a short standing brief that applies to every turn in a
//! workspace — this project's conventions, how the user wants work done, what
//! not to touch. Every comparable tool reads such a file, and the ones people
//! already have on disk are `AGENTS.md` and `CLAUDE.md`.
//!
//! So Taurus reads those rather than asking for a seventh copy, on the same
//! rule the skill library follows: a file installed for another client works
//! here without being moved. The precedence order is the skill order too —
//! personal before project, borrowed before `.taurus` — with one deliberate
//! difference. Skills of the same name shadow each other, because two
//! procedures called `deploy` are rival answers to one question. Instructions
//! *accumulate*: "I prefer terse commit messages" and "this repo pins its
//! toolchain" are both true at once, and dropping either because the other
//! exists would be a silent loss.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config::{self, AGENTS_DIR_NAME, CLAUDE_DIR_NAME};

/// How much of one file reaches the prompt.
///
/// Generous for the case that exists — a written-by-hand brief is a page or
/// two — and a ceiling for the case that would otherwise be silent. These bytes
/// are paid on every request of every turn, so a checked-in 200 KB handbook
/// would spend an 8k model's whole context before it read a line of code.
const MAX_BYTES: usize = 12 * 1024;

/// How much brief, in total, before it is worth saying something.
///
/// [`MAX_BYTES`] bounds one file. This bounds the pile, which only became
/// possible to grow without noticing when Copilot's scoped instructions turned
/// the source list from six named files into six named files and a directory.
/// Reported and then tolerated, like every other size limit here: it is the
/// user's context window and their decision.
const BUDGET_BYTES: usize = 24 * 1024;

/// Whose instructions these are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InstructionsTier {
    /// From the user's home directory: true in every workspace.
    User,
    /// From the workspace: travels with the project.
    Project,
}

/// Which convention the file follows, which is also which client wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InstructionsOrigin {
    Agents,
    Claude,
    Taurus,
    /// GitHub Copilot's, which is two conventions rather than one: a single
    /// `.github/copilot-instructions.md` that applies to everything, and a
    /// directory of `*.instructions.md` files that each declare the paths they
    /// are about. Both are read; see [`Instructions::applies_to`] for what
    /// Taurus can and cannot do with the second.
    Copilot,
}

impl InstructionsOrigin {
    /// What the drawer tags the row with.
    pub fn label(self) -> &'static str {
        match self {
            Self::Agents => "AGENTS.md",
            Self::Claude => "CLAUDE.md",
            Self::Taurus => "TAURUS.md",
            // Only ever the whole-workspace file. A scoped one is labelled by
            // its own name, because there are many of them and
            // `copilot-instructions.md` is not what any of them is called.
            Self::Copilot => "copilot-instructions.md",
        }
    }
}

/// One place instructions may live, whether or not anything is there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InstructionsSource {
    pub tier: InstructionsTier,
    pub origin: InstructionsOrigin,
    pub path: PathBuf,
}

/// A file that was there and had something in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Instructions {
    pub source: InstructionsSource,
    pub body: String,
    /// The file was longer than [`MAX_BYTES`] and the prompt has only its head.
    pub truncated: bool,
    /// The glob a Copilot `*.instructions.md` file declares it is about, and
    /// `None` for every brief that simply applies.
    ///
    /// Copilot attaches these when it is about to touch a matching file. Taurus
    /// has no such moment — a brief is assembled once per turn, before anyone
    /// knows which files the turn will read — so the glob is carried into the
    /// prompt as a sentence instead, and the model applies it when it applies.
    /// That is weaker than Copilot's rule and stronger than dropping the file,
    /// which are the only two other options.
    pub applies_to: Option<String>,
}

/// Everywhere instructions are read from, lowest precedence first.
///
/// The two tiers mirror [`config::skill_sources`] exactly, but the project
/// paths are the repository root rather than a directory inside it. That is
/// where these files actually live in the wild — a repo's brief is `AGENTS.md`
/// beside the README, not `.agents/AGENTS.md` — and reading anywhere else would
/// find nothing in the projects this feature exists for.
pub fn sources(workspace: Option<&Path>) -> Vec<InstructionsSource> {
    all_sources(crate::trust::for_reading(workspace))
}

/// The same, without the trust gate. For [`crate::trust::pending`], which has
/// to count what trusting a workspace would add to the brief.
pub(crate) fn all_sources(workspace: Option<&Path>) -> Vec<InstructionsSource> {
    let home = config::home_root();
    let mut sources = Vec::new();

    let mut push = |tier, origin, path| {
        sources.push(InstructionsSource { tier, origin, path });
    };

    push(
        InstructionsTier::User,
        InstructionsOrigin::Agents,
        home.join(AGENTS_DIR_NAME).join("AGENTS.md"),
    );
    push(
        InstructionsTier::User,
        InstructionsOrigin::Claude,
        home.join(CLAUDE_DIR_NAME).join("CLAUDE.md"),
    );
    push(
        InstructionsTier::User,
        InstructionsOrigin::Taurus,
        config::home_dir().join("TAURUS.md"),
    );
    for path in scoped_files(&home.join(config::COPILOT_DIR_NAME).join("instructions")) {
        push(InstructionsTier::User, InstructionsOrigin::Copilot, path);
    }

    if let Some(workspace) = workspace {
        push(
            InstructionsTier::Project,
            InstructionsOrigin::Agents,
            workspace.join("AGENTS.md"),
        );
        push(
            InstructionsTier::Project,
            InstructionsOrigin::Claude,
            workspace.join("CLAUDE.md"),
        );
        push(
            InstructionsTier::Project,
            InstructionsOrigin::Taurus,
            config::workspace_dir(workspace).join("TAURUS.md"),
        );
        push(
            InstructionsTier::Project,
            InstructionsOrigin::Copilot,
            workspace
                .join(config::GITHUB_DIR_NAME)
                .join("copilot-instructions.md"),
        );
        for path in scoped_files(&workspace.join(config::GITHUB_DIR_NAME).join("instructions")) {
            push(InstructionsTier::Project, InstructionsOrigin::Copilot, path);
        }
    }

    sources
}

/// The directories Copilot's scoped instruction files are read from.
///
/// Separate from [`sources`], which names the files found in them right now.
/// The freshness check needs the folders themselves, so that a file written
/// into an empty one is noticed rather than being invisible until something
/// else moved. See [`crate::freshness`].
pub fn scoped_dirs(workspace: Option<&Path>) -> Vec<PathBuf> {
    all_scoped_dirs(crate::trust::for_reading(workspace))
}

/// The same, without the trust gate. See [`all_sources`].
pub(crate) fn all_scoped_dirs(workspace: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![config::home_root()
        .join(config::COPILOT_DIR_NAME)
        .join("instructions")];
    if let Some(workspace) = workspace {
        dirs.push(workspace.join(config::GITHUB_DIR_NAME).join("instructions"));
    }
    dirs
}

/// The suffix that makes a file one of Copilot's scoped instruction files.
pub const SCOPED_SUFFIX: &str = ".instructions.md";

/// Every `*.instructions.md` under `dir`, sorted, recursing as Copilot does.
///
/// Sorted because this decides prompt order, and a section that reshuffles
/// itself between two identical turns is a diff nobody made. A directory that
/// is not there yields nothing: `~/.copilot` does not exist until someone
/// installs Copilot, and the freshness check is what notices it appearing.
fn scoped_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&path, out),
                _ => {
                    if path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with(SCOPED_SUFFIX))
                    {
                        out.push(path);
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, &mut found);
    found.sort();
    found
}

/// Everything one pass over the sources produced.
pub struct Loaded {
    pub instructions: Vec<Instructions>,
    /// Reported alongside rather than instead: a file with a broken import
    /// still contributes everything else it says.
    pub problems: Vec<String>,
    /// Every file this pass depended on, whether or not it was there.
    ///
    /// The caller fingerprints these to decide whether to read again — see
    /// [`crate::freshness`]. It is not the source list: instructions inline one
    /// level of `@path` imports, so a global `CLAUDE.md` whose whole content is
    /// `@RTK.md` depends on a file the source list never names, and watching
    /// only the sources would let the file holding all of its text change
    /// unnoticed. Absent paths are in here too, because a `CLAUDE.md` that did
    /// not exist and now does is exactly the change worth catching.
    pub read: Vec<PathBuf>,
}

/// Reads every source that exists, in order.
pub fn load(sources: Vec<InstructionsSource>) -> Loaded {
    let mut loaded = Vec::new();
    let mut problems = Vec::new();
    let mut read = Vec::new();
    // Two entries with the same bytes are one instruction, however many files
    // carry it. `CLAUDE.md` symlinked to `AGENTS.md` is the common shape, and
    // saying everything twice is worse than saying it once — a model given a
    // duplicated rule weights it twice as heavily as the author meant.
    let mut seen: HashSet<String> = HashSet::new();

    for source in sources {
        // Before the read, and whether or not it succeeds. A source that is not
        // there is a dependency on it staying not there.
        read.push(source.path.clone());
        let Ok(raw) = std::fs::read_to_string(&source.path) else {
            continue;
        };
        // Only for a file whose name says it has frontmatter. Stripping a
        // leading `---` block from every brief would eat a `CLAUDE.md` that
        // opens on a horizontal rule, which is prose the author wrote.
        let (raw, applies_to) = match is_scoped(&source.path) {
            true => split_frontmatter(&raw),
            false => (raw.as_str(), None),
        };

        // Copilot attaches a scoped file when it is about to touch a matching
        // path, and attaches one with no `applyTo` never — those are for
        // pulling into a request by hand. Carrying one into every turn would
        // be Taurus asserting something about the file that its own tool does
        // not.
        if is_scoped(&source.path) && applies_to.is_none() {
            problems.push(format!(
                "{} declares no applyTo, so Copilot does not apply it automatically and \
                 neither does Taurus; give it `applyTo: \"**\"` to make it a standing brief",
                source.path.display()
            ));
            continue;
        }

        let (body, import_problems, imported) = resolve_imports(raw, &source.path);
        problems.extend(import_problems);
        read.extend(imported);

        let body = body.trim().to_string();
        if body.is_empty() {
            continue;
        }
        if !seen.insert(body.clone()) {
            continue;
        }

        let (body, truncated) = truncate(body);
        if truncated {
            problems.push(format!(
                "{} is larger than {} KB; only its first {} KB reaches the model",
                source.path.display(),
                MAX_BYTES / 1024,
                MAX_BYTES / 1024
            ));
        }
        loaded.push(Instructions {
            source,
            body,
            truncated,
            applies_to,
        });
    }

    // Every one of these is paid on every request of every turn. Six named
    // files were self-limiting; a directory of them is not, and a repository
    // that has grown twenty scoped rules should hear about it from Taurus
    // rather than from its context window.
    let total: usize = loaded.iter().map(|i| i.body.len()).sum();
    if total > BUDGET_BYTES {
        problems.push(format!(
            "the standing brief is {} KB across {} files, over the {} KB this budgets for; \
             every request of every turn pays it",
            total / 1024,
            loaded.len(),
            BUDGET_BYTES / 1024
        ));
    }

    Loaded {
        instructions: loaded,
        problems,
        read,
    }
}

/// Inlines `@path` import lines, one level deep.
///
/// Claude Code's format lets a file be a list of pointers, and real ones are:
/// a global `CLAUDE.md` whose entire content is `@RTK.md` is a file Taurus
/// would otherwise read as a single meaningless line. Resolving it is what
/// makes "reads the file you already have" true rather than nearly true.
///
/// One level only. An import that itself imports is a cycle risk and a context
/// risk for no case anyone has, so the nested line is left as written — visible
/// rather than silently followed or silently dropped.
///
/// A line qualifies only when the whole of it, trimmed, is `@` followed by a
/// path. That keeps `email me @ support` and a doc comment mentioning `@param`
/// out of it: those are prose, and rewriting prose into a file read would be a
/// worse failure than not supporting imports at all.
/// The third return is every path an import line pointed at, resolved but not
/// necessarily found — a target that is missing today and appears tomorrow has
/// to count as a change, so it is depended on either way.
fn resolve_imports(raw: &str, file: &Path) -> (String, Vec<String>, Vec<PathBuf>) {
    let dir = file.parent().unwrap_or(Path::new("."));
    let mut out = String::with_capacity(raw.len());
    let mut problems = Vec::new();
    let mut imported = Vec::new();

    for line in raw.lines() {
        match import_target(line) {
            Some(target) => {
                let path = dir.join(target);
                imported.push(path.clone());
                match std::fs::read_to_string(&path) {
                    Ok(imported) => {
                        out.push_str(imported.trim());
                        out.push('\n');
                    }
                    Err(_) => {
                        // Named rather than passed through: the line is a
                        // pointer, and a pointer at nothing tells the model
                        // less than nothing. Reporting it is how the user finds
                        // out the brief they wrote is not arriving.
                        problems.push(format!(
                            "{} imports {}, which does not exist",
                            file.display(),
                            target
                        ));
                    }
                }
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    (out, problems, imported)
}

/// Whether this file is one of Copilot's scoped instruction files.
fn is_scoped(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(SCOPED_SUFFIX))
}

/// Splits a leading `---` block off, returning the rest and its `applyTo`.
///
/// A hand-rolled read of one field rather than a YAML parse, because one field
/// is all that is wanted and the value is a glob — routinely `**/*.{ts,tsx}`,
/// which is a string that a strict parser and a lenient one disagree about. The
/// block is skipped whole either way, so a file carrying `description:` and
/// anything else Copilot has does not leak into the prompt.
fn split_frontmatter(raw: &str) -> (&str, Option<String>) {
    let Some(rest) = raw.strip_prefix("---") else {
        return (raw, None);
    };
    let Some(rest) = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
    else {
        return (raw, None);
    };
    let Some(end) = rest.find("\n---") else {
        // An opening fence and no close is a malformed file, not a file with a
        // very long header. Passing it through leaves the text visible, which
        // is how the author finds out.
        return (raw, None);
    };
    let (front, after) = rest.split_at(end);

    let applies_to = front.lines().find_map(|line| {
        let value = line.strip_prefix("applyTo:")?.trim();
        let value = value
            .strip_prefix('\'')
            .and_then(|v| v.strip_suffix('\''))
            .or_else(|| value.strip_prefix('"').and_then(|v| v.strip_suffix('"')))
            .unwrap_or(value);
        (!value.is_empty()).then(|| value.to_string())
    });

    let body = after
        .trim_start_matches('\n')
        .strip_prefix("---")
        .unwrap_or(after);
    (body, applies_to)
}

/// The path an import line points at, if the line is one.
fn import_target(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let target = trimmed.strip_prefix('@')?.trim();
    // A path, not a sentence. One token, and one that looks like a file.
    if target.is_empty() || target.contains(char::is_whitespace) {
        return None;
    }
    Some(target)
}

/// Cuts to [`MAX_BYTES`] on a line boundary, so the tail is not half a sentence.
fn truncate(body: String) -> (String, bool) {
    if body.len() <= MAX_BYTES {
        return (body, false);
    }
    let mut end = MAX_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    let cut = body[..end].rfind('\n').unwrap_or(end);
    (body[..cut].to_string(), true)
}

/// The prompt section, or `None` when nothing was found.
///
/// Each file is labelled with where it came from. A model told only the rules
/// cannot tell a personal preference from a project requirement, and the two
/// deserve different weight when they disagree — which, given one comes from
/// the user's home directory and the other from a repository they may have just
/// cloned, they eventually will.
pub fn section(loaded: &[Instructions]) -> Option<String> {
    if loaded.is_empty() {
        return None;
    }

    let mut out = String::from(
        "# Instructions\n\nStanding instructions for this machine and this project. They apply to \
         every turn. Where two disagree, the project's win.\n",
    );
    for entry in loaded {
        let tier = match entry.source.tier {
            InstructionsTier::User => "personal",
            InstructionsTier::Project => "project",
        };
        // A scoped file is labelled by its own name: there can be many of
        // them, and none of them is called `copilot-instructions.md`.
        let name = match entry.applies_to.is_some() {
            true => entry
                .source
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_else(|| entry.source.origin.label())
                .to_string(),
            false => entry.source.origin.label().to_string(),
        };
        let scope = match &entry.applies_to {
            // The whole of what a per-turn brief can do with a per-file rule:
            // say which files it is about and let the model apply it when it
            // is working on one.
            Some(glob) => format!(", applies to files matching `{glob}`"),
            None => String::new(),
        };
        out.push_str(&format!("\n## {name} ({tier}{scope})\n\n{}\n", entry.body));
        if entry.truncated {
            out.push_str(
                "\n[this file was longer than the prompt can carry; the rest was not included]\n",
            );
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
        path
    }

    fn source(path: PathBuf, origin: InstructionsOrigin) -> InstructionsSource {
        InstructionsSource {
            tier: InstructionsTier::Project,
            origin,
            path,
        }
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a scratch directory")
    }

    #[test]
    fn the_project_root_is_where_project_files_are_looked_for() {
        // Not `.agents/AGENTS.md`: a repository's brief sits beside its README,
        // and looking anywhere else finds nothing in the projects this exists
        // for.
        // The ungated list: this test is about where project files are looked
        // for, not about whether a given project is allowed to be read.
        let sources = all_sources(Some(Path::new("/tmp/project")));
        let project: Vec<_> = sources
            .iter()
            .filter(|s| s.tier == InstructionsTier::Project)
            .map(|s| s.path.display().to_string())
            .collect();
        assert!(
            project.iter().any(|p| p.ends_with("project/AGENTS.md")),
            "{project:?}"
        );
        assert!(
            project.iter().any(|p| p.ends_with("project/CLAUDE.md")),
            "{project:?}"
        );
    }

    #[test]
    fn with_no_workspace_only_personal_instructions_are_read() {
        assert!(sources(None)
            .iter()
            .all(|s| s.tier == InstructionsTier::User));
    }

    #[test]
    fn both_tiers_are_kept_rather_than_one_shadowing_the_other() {
        // The deliberate difference from skills. "I prefer terse commits" and
        // "this repo pins its toolchain" are both true at once.
        let tmp = tempdir();
        let dir = tmp.path();
        let user = write(dir, "user.md", "prefer terse commits");
        let project = write(dir, "project.md", "the toolchain is pinned");

        let Loaded {
            instructions: loaded,
            problems,
            ..
        } = load(vec![
            InstructionsSource {
                tier: InstructionsTier::User,
                origin: InstructionsOrigin::Claude,
                path: user,
            },
            source(project, InstructionsOrigin::Agents),
        ]);

        assert!(problems.is_empty());
        assert_eq!(loaded.len(), 2);
        let section = section(&loaded).unwrap();
        assert!(section.contains("prefer terse commits"));
        assert!(section.contains("the toolchain is pinned"));
    }

    #[test]
    fn a_file_repeated_verbatim_is_only_said_once() {
        // CLAUDE.md symlinked to AGENTS.md is the common shape. A rule the
        // model is told twice is a rule it weights twice.
        let tmp = tempdir();
        let dir = tmp.path();
        let a = write(dir, "AGENTS.md", "always run the tests");
        let b = write(dir, "CLAUDE.md", "always run the tests");

        let Loaded {
            instructions: loaded,
            ..
        } = load(vec![
            source(a, InstructionsOrigin::Agents),
            source(b, InstructionsOrigin::Claude),
        ]);
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            section(&loaded)
                .unwrap()
                .matches("always run the tests")
                .count(),
            1
        );
    }

    #[test]
    fn an_import_line_is_replaced_by_the_file_it_names() {
        // The case on a real machine: a global CLAUDE.md whose whole content is
        // `@RTK.md`. Unresolved, Taurus reads one meaningless line.
        let tmp = tempdir();
        let dir = tmp.path();
        write(dir, "RTK.md", "# RTK\n\nUse `rtk` for git.");
        let claude = write(dir, "CLAUDE.md", "@RTK.md\n");

        let Loaded {
            instructions: loaded,
            problems,
            ..
        } = load(vec![source(claude, InstructionsOrigin::Claude)]);
        assert!(problems.is_empty());
        assert!(loaded[0].body.contains("Use `rtk` for git."));
        assert!(!loaded[0].body.contains("@RTK.md"));
    }

    #[test]
    fn prose_that_merely_contains_an_at_sign_is_left_alone() {
        // Rewriting a sentence into a file read is a worse failure than not
        // supporting imports at all.
        let tmp = tempdir();
        let dir = tmp.path();
        let file = write(
            dir,
            "AGENTS.md",
            "Ask @alice before releasing.\nUse @param in doc comments.\n",
        );
        let Loaded {
            instructions: loaded,
            problems,
            ..
        } = load(vec![source(file, InstructionsOrigin::Agents)]);
        assert!(problems.is_empty());
        assert!(loaded[0].body.contains("Ask @alice before releasing."));
        assert!(loaded[0].body.contains("Use @param in doc comments."));
    }

    #[test]
    fn an_import_of_a_missing_file_is_reported_rather_than_passed_through() {
        let tmp = tempdir();
        let dir = tmp.path();
        let file = write(dir, "CLAUDE.md", "real rule\n@missing.md\n");
        let Loaded {
            instructions: loaded,
            problems,
            ..
        } = load(vec![source(file, InstructionsOrigin::Claude)]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("missing.md"), "{problems:?}");
        // Everything else the file said still arrives.
        assert!(loaded[0].body.contains("real rule"));
    }

    #[test]
    fn an_empty_or_absent_file_contributes_nothing() {
        let tmp = tempdir();
        let dir = tmp.path();
        let blank = write(dir, "AGENTS.md", "   \n\n");
        let Loaded {
            instructions: loaded,
            problems,
            ..
        } = load(vec![
            source(blank, InstructionsOrigin::Agents),
            source(dir.join("nope.md"), InstructionsOrigin::Taurus),
        ]);
        assert!(loaded.is_empty());
        assert!(problems.is_empty());
        assert!(section(&loaded).is_none());
    }

    #[test]
    fn an_oversized_file_is_cut_and_says_so() {
        let tmp = tempdir();
        let dir = tmp.path();
        let huge = format!("{}\n", "x".repeat(MAX_BYTES * 2));
        let file = write(dir, "AGENTS.md", &huge);

        let Loaded {
            instructions: loaded,
            problems,
            ..
        } = load(vec![source(file, InstructionsOrigin::Agents)]);
        assert!(loaded[0].truncated);
        assert!(loaded[0].body.len() <= MAX_BYTES);
        assert_eq!(problems.len(), 1);
        // A window that does not announce itself reads as the whole file.
        assert!(section(&loaded)
            .unwrap()
            .contains("longer than the prompt can carry"));
    }

    #[test]
    fn the_section_says_which_instructions_outrank_which() {
        let tmp = tempdir();
        let dir = tmp.path();
        let file = write(dir, "AGENTS.md", "a rule");
        let Loaded {
            instructions: loaded,
            ..
        } = load(vec![source(file, InstructionsOrigin::Agents)]);
        let section = section(&loaded).unwrap();
        assert!(section.contains("project's win"), "{section}");
        assert!(section.contains("(project)"), "{section}");
    }

    #[test]
    fn a_scoped_file_carries_the_paths_it_is_about_into_the_prompt() {
        // Copilot attaches these when it is about to touch a matching file.
        // Taurus assembles a brief once a turn, before anyone knows what the
        // turn will touch, so the glob goes in as a sentence and the model
        // applies it when it applies.
        let dir = tempdir();
        let file = write(
            dir.path(),
            "python.instructions.md",
            "---\napplyTo: \"**/*.py\"\ndescription: Python rules\n---\n\nUse four spaces.\n",
        );

        let loaded = load(vec![source(file.clone(), InstructionsOrigin::Copilot)]);
        assert_eq!(loaded.instructions.len(), 1);
        assert_eq!(
            loaded.instructions[0].applies_to.as_deref(),
            Some("**/*.py")
        );
        assert_eq!(loaded.instructions[0].body, "Use four spaces.");
        assert!(
            !loaded.instructions[0].body.contains("description"),
            "the frontmatter is not prose: {:?}",
            loaded.instructions[0].body
        );

        let section = section(&loaded.instructions).unwrap();
        assert!(section.contains("python.instructions.md"), "{section}");
        assert!(
            section.contains("applies to files matching `**/*.py`"),
            "{section}"
        );
    }

    #[test]
    fn a_scoped_file_with_no_apply_to_is_left_out_and_says_why() {
        // Copilot does not apply one of these automatically either — they are
        // for pulling into a request by hand. Carrying it into every turn would
        // be Taurus claiming something its own source does not.
        let dir = tempdir();
        let file = write(
            dir.path(),
            "manual.instructions.md",
            "---\ndescription: only when asked\n---\n\nDo the thing.\n",
        );

        let loaded = load(vec![source(file.clone(), InstructionsOrigin::Copilot)]);
        assert!(loaded.instructions.is_empty());
        assert_eq!(loaded.problems.len(), 1);
        assert!(
            loaded.problems[0].contains("applyTo"),
            "{:?}",
            loaded.problems
        );
    }

    #[test]
    fn a_brief_that_opens_on_a_horizontal_rule_keeps_it() {
        // Frontmatter is stripped only from files whose name says they have
        // some. A `CLAUDE.md` opening on `---` is an author's prose, and eating
        // it would delete the first paragraph of their brief.
        let dir = tempdir();
        let file = write(dir.path(), "CLAUDE.md", "---\n\nUse tabs.\n");

        let loaded = load(vec![source(file.clone(), InstructionsOrigin::Claude)]);
        assert_eq!(loaded.instructions.len(), 1);
        assert!(
            loaded.instructions[0].body.starts_with("---"),
            "{:?}",
            loaded.instructions[0].body
        );
    }

    #[test]
    fn an_apply_to_is_read_however_it_is_quoted() {
        // The value is a glob, and globs are written `**`, `'**/*.rs'`, and
        // `"**/*.{ts,tsx}"` in the wild.
        for written in ["applyTo: **", "applyTo: '**'", "applyTo: \"**\""] {
            let raw = format!("---\n{written}\n---\n\nBe brief.\n");
            let (body, applies) = split_frontmatter(&raw);
            assert_eq!(applies.as_deref(), Some("**"), "{written}");
            assert_eq!(body.trim(), "Be brief.", "{written}");
        }
    }

    #[test]
    fn an_unclosed_frontmatter_fence_is_left_alone() {
        // A malformed file is not a file with a very long header. Passing the
        // text through is how its author finds out.
        let (body, applies) = split_frontmatter("---\napplyTo: **\n\nnever closed\n");
        assert!(body.starts_with("---"));
        assert_eq!(applies, None);
    }
}
