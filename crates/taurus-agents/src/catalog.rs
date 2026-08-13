//! Agent discovery.
//!
//! Unlike skills, an agent is a single file: it carries a prompt and nothing
//! else, so there is no directory for it to be the root of. That makes the file
//! stem the natural name, and a stem that disagrees with the frontmatter means
//! one of the two is a typo — the same check, for the same reason, that the
//! skill catalog runs against directory names.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::agent::{
    parse_agent_md, validate, AgentDefinition, AgentError, AgentSummary, AgentTier,
};
use crate::builtin;

/// A source directory to scan, and the tier its agents belong to.
#[derive(Clone, Debug)]
pub struct AgentSource {
    pub tier: AgentTier,
    pub dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct AgentCatalog {
    agents: BTreeMap<String, AgentDefinition>,
}

/// The built-ins, and only the built-ins. A host that has not scanned yet still
/// has a working `explorer` and `worker`.
impl Default for AgentCatalog {
    fn default() -> Self {
        Self {
            agents: builtin::definitions()
                .into_iter()
                .map(|agent| (agent.name().to_string(), agent))
                .collect(),
        }
    }
}

impl AgentCatalog {
    /// Scans every source in order, on top of the built-ins. Later sources
    /// shadow earlier ones by name, so a project agent overrides a user agent
    /// and either overrides a built-in.
    ///
    /// Returns the catalog plus any problems found, because one malformed file
    /// must not hide the rest of the roster — and must not take the built-ins
    /// with it.
    pub fn discover(sources: &[AgentSource]) -> (Self, Vec<AgentError>) {
        let mut catalog = Self::default();
        let mut problems = Vec::new();

        for source in sources {
            if !source.dir.is_dir() {
                continue;
            }
            let entries = match std::fs::read_dir(&source.dir) {
                Ok(entries) => entries,
                Err(e) => {
                    problems.push(AgentError::Io(e));
                    continue;
                }
            };
            // read_dir yields in whatever order the filesystem feels like, and
            // the roster is user-visible, so sort before loading.
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|e| e == "md"))
                .filter(|path| path.is_file())
                .collect();
            paths.sort();

            for path in paths {
                match load_agent(&path, source.tier) {
                    Ok(mut agent) => {
                        debug!(name = agent.name(), ?source.tier, "loaded agent");
                        agent.shadows = catalog.agents.get(agent.name()).map(|prior| prior.tier);
                        catalog.agents.insert(agent.name().to_string(), agent);
                    }
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "skipping malformed agent");
                        problems.push(e);
                    }
                }
            }
        }
        (catalog, problems)
    }

    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.agents.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.agents.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = &AgentDefinition> {
        self.agents.values()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut AgentDefinition> {
        self.agents.values_mut()
    }

    /// Drops an agent that cannot safely run here. Used when a file's entire
    /// tool list turned out to be unavailable: see `Host::reload`.
    pub fn remove(&mut self, name: &str) -> Option<AgentDefinition> {
        self.agents.remove(name)
    }

    pub fn summaries(&self) -> Vec<AgentSummary> {
        self.agents.values().map(AgentSummary::from).collect()
    }

    /// A snapshot for the spawn tool, which freezes the roster for a turn.
    pub fn to_vec(&self) -> Vec<AgentDefinition> {
        self.agents.values().cloned().collect()
    }

    /// Characters the roster costs on every request.
    ///
    /// Each agent's line is fixed overhead in the spawn tool's description, the
    /// same argument `disabled_tools` makes about tool schemas. Measured so the
    /// host can say what it costs rather than waiting for it to bite.
    pub fn roster_cost(&self) -> usize {
        self.agents
            .values()
            .map(|agent| agent.roster_line().chars().count() + 1)
            .sum()
    }
}

fn load_agent(path: &Path, tier: AgentTier) -> Result<AgentDefinition, AgentError> {
    let display = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|e| AgentError::Invalid {
        path: display.clone(),
        message: format!("could not be read: {e}"),
    })?;
    let (frontmatter, system_prompt) = parse_agent_md(&text, path)?;
    validate(&frontmatter, &system_prompt, &display)?;

    // The stem is authoritative: `spawn_subagent` is called by name, and a file
    // whose name and frontmatter disagree means one of them is what the author
    // will type and the other is what works.
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if stem != frontmatter.name {
            return Err(AgentError::Invalid {
                path: display,
                message: format!(
                    "the file is named '{stem}.md' but the agent is named '{}'; rename one to \
                     match the other",
                    frontmatter.name
                ),
            });
        }
    }

    Ok(AgentDefinition {
        frontmatter,
        system_prompt,
        tier,
        path: Some(path.to_path_buf()),
        shadows: None,
        degraded: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_agent(root: &Path, stem: &str, body: &str, extra: &str) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join(format!("{stem}.md")),
            format!("---\nname: {stem}\ndescription: does {stem}\n{extra}---\n\n{body}\n"),
        )
        .unwrap();
    }

    fn sources(dirs: Vec<(AgentTier, &Path)>) -> Vec<AgentSource> {
        dirs.into_iter()
            .map(|(tier, dir)| AgentSource {
                tier,
                dir: dir.to_path_buf(),
            })
            .collect()
    }

    #[test]
    fn the_builtins_survive_a_machine_with_no_agents_directory() {
        // The regression this guards against is losing shipped behaviour to a
        // discovery bug, which is worse than a new feature failing to appear.
        let missing = TempDir::new().unwrap().path().join("nope");
        let (catalog, problems) =
            AgentCatalog::discover(&sources(vec![(AgentTier::User, &missing)]));
        assert!(problems.is_empty());
        assert_eq!(catalog.len(), 2);
        assert!(catalog.contains("explorer"));
        assert!(catalog.contains("worker"));
    }

    #[test]
    fn discovers_agents_in_a_directory() {
        let dir = TempDir::new().unwrap();
        write_agent(dir.path(), "alpha", "Be alpha.", "");
        write_agent(dir.path(), "beta", "Be beta.", "");

        let (catalog, problems) =
            AgentCatalog::discover(&sources(vec![(AgentTier::User, dir.path())]));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(catalog.len(), 4, "two built-ins plus two files");
        assert_eq!(catalog.get("beta").unwrap().system_prompt, "Be beta.");
        assert_eq!(catalog.get("beta").unwrap().tier, AgentTier::User);
    }

    #[test]
    fn parses_crlf_and_bom_from_disk() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("alpha.md"),
            "\u{feff}---\r\nname: alpha\r\ndescription: d\r\n---\r\n\r\nBe alpha.\r\n",
        )
        .unwrap();
        let (catalog, problems) =
            AgentCatalog::discover(&sources(vec![(AgentTier::User, dir.path())]));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(catalog.get("alpha").unwrap().system_prompt, "Be alpha.");
    }

    #[test]
    fn a_project_agent_shadows_a_user_agent_of_the_same_name() {
        let user = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        write_agent(user.path(), "shared", "User version.", "");
        write_agent(project.path(), "shared", "Project version.", "");

        let (catalog, _) = AgentCatalog::discover(&sources(vec![
            (AgentTier::User, user.path()),
            (AgentTier::Project, project.path()),
        ]));
        let agent = catalog.get("shared").unwrap();
        assert_eq!(agent.tier, AgentTier::Project);
        assert_eq!(agent.system_prompt, "Project version.");
        assert_eq!(agent.shadows, Some(AgentTier::User));
    }

    #[test]
    fn a_user_agent_shadows_a_builtin_of_the_same_name() {
        let dir = TempDir::new().unwrap();
        write_agent(dir.path(), "explorer", "My explorer, with the shell.", "");

        let (catalog, problems) =
            AgentCatalog::discover(&sources(vec![(AgentTier::User, dir.path())]));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            catalog.len(),
            2,
            "it replaced the built-in, not added to it"
        );
        let agent = catalog.get("explorer").unwrap();
        assert_eq!(agent.tier, AgentTier::User);
        assert_eq!(agent.shadows, Some(AgentTier::Builtin));
    }

    #[test]
    fn one_malformed_agent_does_not_hide_the_others() {
        let dir = TempDir::new().unwrap();
        write_agent(dir.path(), "good", "Be good.", "");
        std::fs::write(dir.path().join("bad.md"), "no frontmatter here").unwrap();

        let (catalog, problems) =
            AgentCatalog::discover(&sources(vec![(AgentTier::User, dir.path())]));
        assert!(catalog.contains("good"));
        assert!(catalog.contains("explorer"), "and not the built-ins either");
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn a_name_that_disagrees_with_its_file_is_rejected() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("file-name.md"),
            "---\nname: different-name\ndescription: d\n---\nbody",
        )
        .unwrap();

        let (catalog, problems) =
            AgentCatalog::discover(&sources(vec![(AgentTier::User, dir.path())]));
        assert!(!catalog.contains("different-name"));
        assert!(problems[0].to_string().contains("rename one to match"));
    }

    #[test]
    fn a_non_markdown_file_is_ignored_silently() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not an agent").unwrap();
        std::fs::create_dir_all(dir.path().join("subdir")).unwrap();
        let (catalog, problems) =
            AgentCatalog::discover(&sources(vec![(AgentTier::User, dir.path())]));
        assert_eq!(catalog.len(), 2);
        assert!(problems.is_empty());
    }

    #[test]
    fn the_roster_cost_grows_with_the_roster() {
        let empty = AgentCatalog::default().roster_cost();
        let dir = TempDir::new().unwrap();
        write_agent(dir.path(), "alpha", "Be alpha.", "");
        let (catalog, _) = AgentCatalog::discover(&sources(vec![(AgentTier::User, dir.path())]));
        assert!(catalog.roster_cost() > empty);
    }
}
