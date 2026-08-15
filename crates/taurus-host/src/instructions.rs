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
}

impl InstructionsOrigin {
    /// What the drawer tags the row with.
    pub fn label(self) -> &'static str {
        match self {
            Self::Agents => "AGENTS.md",
            Self::Claude => "CLAUDE.md",
            Self::Taurus => "TAURUS.md",
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
}

/// Everywhere instructions are read from, lowest precedence first.
///
/// The two tiers mirror [`config::skill_sources`] exactly, but the project
/// paths are the repository root rather than a directory inside it. That is
/// where these files actually live in the wild — a repo's brief is `AGENTS.md`
/// beside the README, not `.agents/AGENTS.md` — and reading anywhere else would
/// find nothing in the projects this feature exists for.
pub fn sources(workspace: Option<&Path>) -> Vec<InstructionsSource> {
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
    }

    sources
}

/// Reads every source that exists, in order.
///
/// Returns problems alongside, rather than instead: a file with a broken import
/// still contributes everything else it says.
pub fn load(sources: Vec<InstructionsSource>) -> (Vec<Instructions>, Vec<String>) {
    let mut loaded = Vec::new();
    let mut problems = Vec::new();
    // Two entries with the same bytes are one instruction, however many files
    // carry it. `CLAUDE.md` symlinked to `AGENTS.md` is the common shape, and
    // saying everything twice is worse than saying it once — a model given a
    // duplicated rule weights it twice as heavily as the author meant.
    let mut seen: HashSet<String> = HashSet::new();

    for source in sources {
        let Ok(raw) = std::fs::read_to_string(&source.path) else {
            continue;
        };
        let (body, import_problems) = resolve_imports(&raw, &source.path);
        problems.extend(import_problems);

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
        });
    }

    (loaded, problems)
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
fn resolve_imports(raw: &str, file: &Path) -> (String, Vec<String>) {
    let dir = file.parent().unwrap_or(Path::new("."));
    let mut out = String::with_capacity(raw.len());
    let mut problems = Vec::new();

    for line in raw.lines() {
        match import_target(line) {
            Some(target) => {
                let path = dir.join(target);
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

    (out, problems)
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
    let cut = body[..end].rfind('\n').map_or(end, |i| i);
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
        out.push_str(&format!(
            "\n## {} ({tier})\n\n{}\n",
            entry.source.origin.label(),
            entry.body
        ));
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
        let sources = sources(Some(Path::new("/tmp/project")));
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

        let (loaded, problems) = load(vec![
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

        let (loaded, _) = load(vec![
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

        let (loaded, problems) = load(vec![source(claude, InstructionsOrigin::Claude)]);
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
        let (loaded, problems) = load(vec![source(file, InstructionsOrigin::Agents)]);
        assert!(problems.is_empty());
        assert!(loaded[0].body.contains("Ask @alice before releasing."));
        assert!(loaded[0].body.contains("Use @param in doc comments."));
    }

    #[test]
    fn an_import_of_a_missing_file_is_reported_rather_than_passed_through() {
        let tmp = tempdir();
        let dir = tmp.path();
        let file = write(dir, "CLAUDE.md", "real rule\n@missing.md\n");
        let (loaded, problems) = load(vec![source(file, InstructionsOrigin::Claude)]);
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
        let (loaded, problems) = load(vec![
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

        let (loaded, problems) = load(vec![source(file, InstructionsOrigin::Agents)]);
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
        let (loaded, _) = load(vec![source(file, InstructionsOrigin::Agents)]);
        let section = section(&loaded).unwrap();
        assert!(section.contains("project's win"), "{section}");
        assert!(section.contains("(project)"), "{section}");
    }
}
