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
use crate::skill::{parse_skill_md, validate, Skill, SkillError, SkillSummary, SkillTier};

pub const SKILL_FILE: &str = "SKILL.md";

#[derive(Default, Debug)]
pub struct SkillCatalog {
    skills: BTreeMap<String, Skill>,
}

/// A source directory to scan, and the tier its skills belong to.
#[derive(Clone, Debug)]
pub struct SkillSource {
    pub tier: SkillTier,
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
                match load_skill(&dir, source.tier) {
                    Ok(skill) => {
                        debug!(name = skill.name(), ?source.tier, "loaded skill");
                        catalog.skills.insert(skill.name().to_string(), skill);
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

    /// The section appended to the system prompt. `None` when there are no
    /// skills, so an empty library costs nothing.
    pub fn prompt_section(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let mut out = String::from(
            "# Skills\n\n\
             Procedures written down from earlier work. Each line says when the skill applies. \
             If one matches the task, call `load_skill` with its name to read the full procedure \
             before you start; the line below is only an index, not the instructions.\n\n",
        );
        for skill in self.skills.values() {
            out.push_str(&skill.catalog_line());
            if skill.degraded.is_some() {
                out.push_str(" [scripts unavailable here: follow the written steps]");
            }
            out.push('\n');
        }
        Some(out)
    }
}

fn load_skill(dir: &Path, tier: SkillTier) -> Result<Skill, SkillError> {
    let path = dir.join(SKILL_FILE);
    let text = std::fs::read_to_string(&path)?;
    let (frontmatter, body) = parse_skill_md(&text, &path)?;
    validate(&frontmatter, &path.display().to_string())?;

    // The skill directory name is not authoritative, but a mismatch means one
    // of the two is a typo and the model will reference the wrong one.
    if let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) {
        if dir_name != frontmatter.name {
            return Err(SkillError::Invalid {
                path: path.display().to_string(),
                message: format!(
                    "directory is named '{dir_name}' but the skill is named '{}'; they must match",
                    frontmatter.name
                ),
            });
        }
    }

    // Resolve interpreters once, at load time: the alternative is discovering
    // that python is missing halfway through a task.
    let missing = interpreter::missing_interpreters(
        frontmatter.scripts.iter().map(|s| s.interpreter.as_str()),
    );
    let degraded = (!missing.is_empty()).then(|| missing.join("; "));

    Ok(Skill {
        frontmatter,
        body,
        dir: dir.to_path_buf(),
        tier,
        degraded,
    })
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

    fn sources(dirs: Vec<(SkillTier, &Path)>) -> Vec<SkillSource> {
        dirs.into_iter()
            .map(|(tier, dir)| SkillSource {
                tier,
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
        assert_eq!(skill.frontmatter.when_to_use, "project version");
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
    fn a_name_that_disagrees_with_its_directory_is_rejected() {
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
        assert!(catalog.is_empty());
        assert!(problems[0].to_string().contains("must match"));
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
    fn an_empty_library_contributes_nothing_to_the_prompt() {
        let catalog = SkillCatalog::default();
        assert!(catalog.prompt_section().is_none());
    }
}
