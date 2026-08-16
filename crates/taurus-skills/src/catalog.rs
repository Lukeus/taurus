//! Skill discovery and the prompt catalog.
//!
//! The catalog is the progressive-disclosure boundary. Only one line per skill
//! reaches the system prompt; the procedure itself is fetched by the model when
//! it decides a skill applies. With a 50-skill library and an 8k local context,
//! that difference is what makes skills viable at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::interpreter;
use crate::skill::{
    parse_skill_md, validate, Skill, SkillError, SkillOrigin, SkillScript, SkillSummary, SkillTier,
};

pub const SKILL_FILE: &str = "SKILL.md";

#[derive(Default, Debug)]
pub struct SkillCatalog {
    skills: BTreeMap<String, Skill>,
}

/// A source directory to scan, and where its skills sit in the precedence order.
#[derive(Clone, Debug)]
pub struct SkillSource {
    pub tier: SkillTier,
    pub origin: SkillOrigin,
    pub dir: PathBuf,
}

impl SkillCatalog {
    /// Scans every source in order. Later sources shadow earlier ones by name,
    /// so a project skill overrides a user skill of the same name.
    ///
    /// Returns the catalog plus any problems found, because one malformed
    /// skill must not hide the rest of the library.
    pub fn discover(sources: &[SkillSource]) -> (Self, Vec<SkillError>) {
        let mut catalog = Self::default();
        let mut problems = Vec::new();

        for source in sources {
            if !source.dir.is_dir() {
                continue;
            }
            let entries = match std::fs::read_dir(&source.dir) {
                Ok(entries) => entries,
                Err(e) => {
                    problems.push(SkillError::Io(e));
                    continue;
                }
            };
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                match load_skill(&dir, source.tier, source.origin) {
                    Ok(skill) => {
                        debug!(name = skill.name(), ?source.tier, ?source.origin, "loaded skill");
                        let name = skill.name().to_string();
                        if let Some(shadowed) = catalog.skills.insert(name.clone(), skill) {
                            // Not a problem — overriding by name is the whole
                            // point of the tiers — but silently dropping a
                            // skill someone installed is how you spend an
                            // afternoon wondering why an edit had no effect.
                            warn!(
                                skill = %name,
                                shadowed = %shadowed.dir.display(),
                                winner = %dir.display(),
                                "skill shadows another of the same name"
                            );
                        }
                    }
                    Err(SkillError::Io(_)) => {
                        // A directory with no SKILL.md is not a skill; that is
                        // normal, not an error worth showing the user.
                    }
                    Err(e) => {
                        warn!(dir = %dir.display(), error = %e, "skipping malformed skill");
                        problems.push(e);
                    }
                }
            }
        }
        (catalog, problems)
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.skills.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.skills.keys().map(String::as_str)
    }

    pub fn summaries(&self) -> Vec<SkillSummary> {
        self.skills.values().map(SkillSummary::from).collect()
    }

    /// Every loaded skill's directory.
    ///
    /// Handed to the tool layer as extra readable roots. A skill's procedure
    /// routinely says "see references/REFERENCE.md", and a user skill lives
    /// outside the workspace, so without this the model is told to read a file
    /// the path guard will refuse — which reads as a broken tool rather than a
    /// boundary doing its job.
    pub fn dirs(&self) -> Vec<PathBuf> {
        self.skills.values().map(|s| s.dir.clone()).collect()
    }

    /// Skills a person can run directly as `/name`, in catalog order.
    ///
    /// Half the slash namespace: sub-agents hold the other half, and which one
    /// a typed name resolves to is settled a layer up, in `taurus-host`.
    pub fn commands(&self) -> impl Iterator<Item = &Skill> {
        self.skills
            .values()
            .filter(|s| s.frontmatter.user_invocable)
    }

    /// The section appended to the system prompt. `None` when there are no
    /// skills, so an empty library costs nothing.
    pub fn prompt_section(&self) -> Option<String> {
        // A skill the model may not choose is left out entirely rather than
        // listed and refused: the catalog is what it picks from, and an entry
        // it cannot act on is a turn spent finding that out.
        if !self.skills.values().any(|s| s.model_invocable()) {
            return None;
        }
        let mut out = String::from(
            "# Skills\n\n\
             Procedures written down from earlier work. Each line says when the skill applies. \
             If one matches the task, call `load_skill` with its name to read the full procedure \
             before you start; the line below is only an index, not the instructions.\n\n",
        );
        for skill in self.skills.values().filter(|s| s.model_invocable()) {
            out.push_str(&skill.catalog_line());
            if skill.degraded.is_some() {
                out.push_str(" [scripts unavailable here: follow the written steps]");
            }
            out.push('\n');
        }
        Some(out)
    }
}

/// Directories a skill may bundle resources in, per the specification.
const RESOURCE_DIRS: [&str; 3] = ["scripts", "references", "assets"];

/// How many bundled files are listed before the rest are summarized. A skill
/// with two hundred assets should not spend the model's context introducing
/// them.
const MAX_RESOURCES: usize = 40;

fn load_skill(dir: &Path, tier: SkillTier, origin: SkillOrigin) -> Result<Skill, SkillError> {
    let path = dir.join(SKILL_FILE);
    let text = std::fs::read_to_string(&path)?;
    let parsed = parse_skill_md(&text, &path)?;
    let mut frontmatter = parsed.frontmatter;
    let mut warnings = parsed.warnings;
    warnings.extend(validate(&frontmatter, &path.display().to_string())?);

    // The specification requires these to match, and a mismatch does mean one
    // of the two is a typo. It is still only a naming problem: the skill is
    // keyed by its frontmatter name everywhere, so it loads and works.
    if let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) {
        if dir_name != frontmatter.name {
            warnings.push(format!(
                "directory is named '{dir_name}' but the skill is named '{}'; the skill is used \
                 by its name",
                frontmatter.name
            ));
        }
    }

    let resources = bundled_resources(dir);
    frontmatter
        .scripts
        .extend(undeclared_scripts(&frontmatter.scripts, &resources));

    // Resolve interpreters once, at load time: the alternative is discovering
    // that python is missing halfway through a task.
    let missing = interpreter::missing_interpreters(
        frontmatter.scripts.iter().map(|s| s.interpreter.as_str()),
    );
    let degraded = (!missing.is_empty()).then(|| missing.join("; "));

    Ok(Skill {
        frontmatter,
        body: parsed.body,
        dir: dir.to_path_buf(),
        tier,
        origin,
        resources,
        warnings,
        degraded,
    })
}

/// Files bundled in a skill's conventional subdirectories, as paths relative to
/// the skill root.
///
/// One level deep, which is as far as the specification asks skills to nest and
/// as far as a listing stays readable.
fn bundled_resources(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for name in RESOURCE_DIRS {
        let Ok(entries) = std::fs::read_dir(dir.join(name)) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().is_file() {
                continue;
            }
            if let Some(file) = entry.file_name().to_str() {
                // Editor leftovers and OS clutter are not resources.
                if file.starts_with('.') {
                    continue;
                }
                found.push(format!("{name}/{file}"));
            }
        }
    }
    // Directory order is arbitrary and would reshuffle the model's context and
    // the drawer's list between runs for no reason.
    found.sort();
    found.truncate(MAX_RESOURCES);
    found
}

/// Scripts present in `scripts/` that the frontmatter never mentioned.
///
/// Taurus's own skills declare their scripts; skills written for other clients
/// just drop a file in `scripts/` and refer to it in prose. Without this those
/// scripts exist on disk and are unreachable through `run_skill_script`, which
/// reads as the tool being broken rather than the skill being written for
/// somebody else.
///
/// Declared entries win: an author who wrote down an interpreter meant it.
fn undeclared_scripts(declared: &[SkillScript], resources: &[String]) -> Vec<SkillScript> {
    resources
        .iter()
        .filter(|path| path.starts_with("scripts/"))
        .filter(|path| !declared.iter().any(|s| &&s.path == path))
        .filter_map(|path| {
            let extension = Path::new(path).extension()?.to_str()?;
            Some(SkillScript {
                path: path.clone(),
                interpreter: interpreter::for_extension(extension)?.to_string(),
                description: "found in scripts/; not described in the skill".to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_skill(root: &Path, name: &str, when: &str, extra: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SKILL_FILE),
            format!(
                "---\nname: {name}\ndescription: does {name}\nwhen_to_use: {when}\n{extra}---\n\n\
                 Body of {name}.\n"
            ),
        )
        .unwrap();
    }

    /// Writes a skill in the shape another client would leave behind: the two
    /// required fields, no `when_to_use`.
    fn write_borrowed_skill(root: &Path, name: &str, description: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SKILL_FILE),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nBody of {name}.\n"),
        )
        .unwrap();
    }

    fn sources(dirs: Vec<(SkillTier, &Path)>) -> Vec<SkillSource> {
        dirs.into_iter()
            .map(|(tier, dir)| SkillSource {
                tier,
                origin: SkillOrigin::Taurus,
                dir: dir.to_path_buf(),
            })
            .collect()
    }

    #[test]
    fn discovers_skills_in_a_directory() {
        let dir = TempDir::new().unwrap();
        write_skill(dir.path(), "alpha", "when alpha", "");
        write_skill(dir.path(), "beta", "when beta", "");

        let (catalog, problems) =
            SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(catalog.len(), 2);
        assert!(catalog.contains("alpha"));
        assert_eq!(catalog.get("beta").unwrap().body.trim(), "Body of beta.");
    }

    #[test]
    fn a_project_skill_shadows_a_user_skill_of_the_same_name() {
        let user = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        write_skill(user.path(), "shared", "user version", "");
        write_skill(project.path(), "shared", "project version", "");

        let (catalog, _) = SkillCatalog::discover(&sources(vec![
            (SkillTier::User, user.path()),
            (SkillTier::Project, project.path()),
        ]));
        assert_eq!(catalog.len(), 1);
        let skill = catalog.get("shared").unwrap();
        assert_eq!(skill.tier, SkillTier::Project);
        assert_eq!(skill.trigger(), "project version");
    }

    #[test]
    fn one_malformed_skill_does_not_hide_the_others() {
        let dir = TempDir::new().unwrap();
        write_skill(dir.path(), "good", "when good", "");
        let bad = dir.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join(SKILL_FILE), "no frontmatter here").unwrap();

        let (catalog, problems) =
            SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        assert!(catalog.contains("good"));
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn a_directory_without_a_skill_file_is_ignored_silently() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("not-a-skill")).unwrap();
        let (catalog, problems) =
            SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        assert!(catalog.is_empty());
        assert!(problems.is_empty());
    }

    #[test]
    fn a_name_that_disagrees_with_its_directory_loads_with_a_warning() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("folder-name");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join(SKILL_FILE),
            "---\nname: different-name\ndescription: d\nwhen_to_use: w\n---\nbody",
        )
        .unwrap();

        let (catalog, problems) =
            SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        assert!(problems.is_empty(), "a naming slip is not a load failure");
        let skill = catalog.get("different-name").expect("keyed by its name");
        assert_eq!(skill.warnings.len(), 1);
        assert!(skill.warnings[0].contains("folder-name"));
    }

    #[test]
    fn bundled_resources_are_listed_but_not_read() {
        let dir = TempDir::new().unwrap();
        write_borrowed_skill(dir.path(), "bundled", "Use when testing bundles.");
        let root = dir.path().join("bundled");
        for (sub, file) in [
            ("references", "REFERENCE.md"),
            ("assets", "template.docx"),
            ("scripts", "extract.py"),
        ] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
            std::fs::write(root.join(sub).join(file), "contents").unwrap();
        }
        // Editor and OS leftovers are not resources.
        std::fs::write(root.join("references/.DS_Store"), "").unwrap();

        let (catalog, problems) =
            SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        assert!(problems.is_empty(), "{problems:?}");

        let skill = catalog.get("bundled").unwrap();
        assert_eq!(
            skill.resources,
            [
                "assets/template.docx",
                "references/REFERENCE.md",
                "scripts/extract.py"
            ]
        );
        assert!(
            !skill.body.contains("contents"),
            "a resource must cost nothing until the procedure asks for it"
        );
    }

    #[test]
    fn a_script_the_frontmatter_never_declared_is_still_runnable() {
        let dir = TempDir::new().unwrap();
        write_borrowed_skill(dir.path(), "undeclared", "Use when testing scripts.");
        let scripts = dir.path().join("undeclared/scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("extract.py"), "print('hi')").unwrap();
        // Not a script in any language Taurus can run; it stays a resource.
        std::fs::write(scripts.join("notes.txt"), "just notes").unwrap();

        let (catalog, _) = SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        let skill = catalog.get("undeclared").unwrap();

        assert_eq!(skill.frontmatter.scripts.len(), 1);
        assert_eq!(skill.frontmatter.scripts[0].path, "scripts/extract.py");
        assert_eq!(skill.frontmatter.scripts[0].interpreter, "python3");
    }

    #[test]
    fn a_declared_script_is_not_discovered_twice() {
        let dir = TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "declared",
            "when declared",
            "scripts:\n  - path: scripts/extract.py\n    interpreter: python3\n    \
             description: The author's own words\n",
        );
        let scripts = dir.path().join("declared/scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("extract.py"), "print('hi')").unwrap();

        let (catalog, _) = SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        let skill = catalog.get("declared").unwrap();

        assert_eq!(skill.frontmatter.scripts.len(), 1);
        assert_eq!(
            skill.frontmatter.scripts[0].description, "The author's own words",
            "what the author wrote wins over what was guessed"
        );
    }

    #[test]
    fn a_missing_interpreter_degrades_rather_than_rejects() {
        let dir = TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "needs-tooling",
            "when tooling",
            "scripts:\n  - path: go.bf\n    interpreter: brainfuck\n",
        );
        let (catalog, problems) =
            SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        assert!(problems.is_empty());
        let skill = catalog.get("needs-tooling").unwrap();
        assert!(skill.degraded.is_some(), "skill should be marked degraded");
    }

    #[test]
    fn the_prompt_section_lists_when_to_use_and_not_the_body() {
        let dir = TempDir::new().unwrap();
        write_skill(dir.path(), "alpha", "when the user asks about alpha", "");
        let (catalog, _) = SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));

        let section = catalog.prompt_section().unwrap();
        assert!(section.contains("- alpha: when the user asks about alpha"));
        assert!(section.contains("load_skill"));
        assert!(
            !section.contains("Body of alpha"),
            "the body must stay out of the system prompt"
        );
    }

    #[test]
    fn the_prompt_section_flags_degraded_skills() {
        let dir = TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "needs-tooling",
            "when tooling",
            "scripts:\n  - path: go.bf\n    interpreter: brainfuck\n",
        );
        let (catalog, _) = SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        assert!(catalog
            .prompt_section()
            .unwrap()
            .contains("scripts unavailable here"));
    }

    #[test]
    fn a_skill_from_another_client_loads_and_reaches_the_prompt() {
        let dir = TempDir::new().unwrap();
        write_borrowed_skill(
            dir.path(),
            "pdf-processing",
            "Extract PDF text and fill forms. Use when handling PDFs.",
        );

        let (catalog, problems) = SkillCatalog::discover(&[SkillSource {
            tier: SkillTier::User,
            origin: SkillOrigin::Claude,
            dir: dir.path().to_path_buf(),
        }]);
        assert!(problems.is_empty(), "{problems:?}");

        let skill = catalog.get("pdf-processing").unwrap();
        assert_eq!(skill.origin, SkillOrigin::Claude);
        assert!(catalog.prompt_section().unwrap().contains(
            "- pdf-processing: Extract PDF text and fill forms. Use when handling PDFs."
        ));
    }

    #[test]
    fn a_taurus_skill_shadows_a_borrowed_one_in_the_same_tier() {
        let borrowed = TempDir::new().unwrap();
        let native = TempDir::new().unwrap();
        write_borrowed_skill(borrowed.path(), "shared", "the borrowed one");
        write_skill(native.path(), "shared", "the native one", "");

        // Source order is the precedence order: the shared conventions are read
        // first, `.taurus` last.
        let (catalog, _) = SkillCatalog::discover(&[
            SkillSource {
                tier: SkillTier::User,
                origin: SkillOrigin::Claude,
                dir: borrowed.path().to_path_buf(),
            },
            SkillSource {
                tier: SkillTier::User,
                origin: SkillOrigin::Taurus,
                dir: native.path().to_path_buf(),
            },
        ]);
        assert_eq!(catalog.len(), 1);
        let skill = catalog.get("shared").unwrap();
        assert_eq!(skill.origin, SkillOrigin::Taurus);
        assert_eq!(skill.trigger(), "the native one");
    }

    #[test]
    fn the_invocation_flags_decide_which_way_in_a_skill_has() {
        let dir = TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "user-only",
            "when asked",
            "disable-model-invocation: true\n",
        );
        write_skill(
            dir.path(),
            "model-only",
            "when needed",
            "user-invocable: false\n",
        );
        let (catalog, problems) =
            SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        assert!(problems.is_empty(), "{problems:?}");

        // The model sees only what it may choose.
        let prompt = catalog.prompt_section().unwrap();
        assert!(prompt.contains("- model-only:"));
        assert!(
            !prompt.contains("- user-only:"),
            "a skill the model cannot open must not be listed for it"
        );

        // And the user only what they may run.
        let commands: Vec<&str> = catalog.commands().map(|s| s.name()).collect();
        assert_eq!(commands, ["user-only"]);
    }

    #[test]
    fn a_library_of_only_user_skills_contributes_nothing_to_the_prompt() {
        let dir = TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "user-only",
            "when asked",
            "disable-model-invocation: true\n",
        );
        let (catalog, _) = SkillCatalog::discover(&sources(vec![(SkillTier::User, dir.path())]));
        assert!(
            catalog.prompt_section().is_none(),
            "an empty Skills heading is worse than none"
        );
    }

    #[test]
    fn an_empty_library_contributes_nothing_to_the_prompt() {
        let catalog = SkillCatalog::default();
        assert!(catalog.prompt_section().is_none());
    }
}
