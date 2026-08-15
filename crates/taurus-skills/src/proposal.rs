//! Auto-generated skills: proposal, validation, and the write on approval.
//!
//! Nothing here touches the skill library. A proposal is inert data until the
//! user approves it in the UI, which is the whole point: a model that can
//! silently write its own future instructions can also silently poison them.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::catalog::{SkillCatalog, SKILL_FILE};
use crate::interpreter;
use crate::skill::{validate, SkillFrontmatter, SkillScript};

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProposedScript {
    pub path: String,
    pub interpreter: String,
    #[serde(default)]
    pub description: String,
    /// Full source of the script, shown to the user before approval.
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillProposal {
    pub id: String,
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    /// The procedure, as markdown.
    pub body: String,
    #[serde(default)]
    pub scripts: Vec<ProposedScript>,
    /// Why the agent thinks this is worth keeping. Shown in the review card;
    /// never written to disk.
    #[serde(default)]
    pub rationale: String,
    /// True when a skill of this name already exists and would be replaced.
    #[serde(default)]
    pub replaces_existing: bool,
}

impl SkillProposal {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        when_to_use: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            description: description.into(),
            when_to_use: when_to_use.into(),
            body: body.into(),
            scripts: Vec::new(),
            rationale: String::new(),
            replaces_existing: false,
        }
    }

    fn frontmatter(&self) -> SkillFrontmatter {
        SkillFrontmatter {
            name: self.name.clone(),
            description: self.description.clone(),
            // Always written, even though the format makes it optional: a
            // skill Taurus authors should carry the sharper trigger line, and
            // other clients ignore the field rather than choke on it.
            when_to_use: Some(self.when_to_use.clone()),
            version: 1,
            compatibility: None,
            // A skill Taurus writes is reachable both ways: the model chose to
            // write it down, and the user can call for it by name.
            disable_model_invocation: false,
            user_invocable: true,
            allowed_tools: Vec::new(),
            scripts: self
                .scripts
                .iter()
                .map(|s| SkillScript {
                    path: s.path.clone(),
                    interpreter: s.interpreter.clone(),
                    description: s.description.clone(),
                })
                .collect(),
        }
    }
}

/// Where an approved proposal gets written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SaveTarget {
    /// `<workspace>/.taurus/skills` — travels with the project.
    Project,
    /// `~/.taurus/skills` — available in every workspace.
    User,
}

/// Command lines that are never appropriate in a generated skill.
///
/// This is a floor, not a sandbox: the user still reviews the script, and
/// `run_skill_script` still goes through the permission gate. It exists so an
/// obviously destructive proposal never reaches a review card, where a tired
/// user might wave it through.
const DENIED_PATTERNS: &[(&str, &str)] = &[
    ("rm -rf /", "recursive delete from the filesystem root"),
    ("rm -fr /", "recursive delete from the filesystem root"),
    ("mkfs", "filesystem creation"),
    ("dd if=", "raw device write"),
    (":(){", "fork bomb"),
    ("shutdown", "shutting the machine down"),
    ("reboot", "rebooting the machine"),
    ("format c:", "formatting a drive"),
    (
        "remove-item -recurse -force c:\\",
        "recursive delete of the system drive",
    ),
    ("history -c", "clearing shell history"),
    ("chmod -r 777 /", "making the filesystem world-writable"),
];

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ProposalRejected(pub String);

/// Checks a proposal before it reaches the user.
///
/// Everything here is something a human reviewer would either miss or find
/// tedious to check: schema conformance, name collisions, near-duplicates of
/// skills that already exist, and scripts that do something categorically
/// unacceptable.
pub fn validate_proposal(
    proposal: &SkillProposal,
    catalog: &SkillCatalog,
) -> Result<(), ProposalRejected> {
    let frontmatter = proposal.frontmatter();
    // Loading from disk tolerates these, because a skill someone already has
    // installed is better read than refused. Nothing is already installed here:
    // the file does not exist yet, and writing one that only loads by leniency
    // would be choosing to create the problem the leniency exists to survive.
    let warnings =
        validate(&frontmatter, "proposal").map_err(|e| ProposalRejected(e.to_string()))?;
    if let Some(first) = warnings.first() {
        return Err(ProposalRejected(first.clone()));
    }

    if proposal.body.trim().len() < 40 {
        return Err(ProposalRejected(
            "the procedure is too short to be useful; write the actual steps".into(),
        ));
    }

    // Every declared script must ship its source, and vice versa: a skill that
    // references a file it did not include is broken the moment it is used.
    for script in &proposal.scripts {
        if script.content.trim().is_empty() {
            return Err(ProposalRejected(format!(
                "script '{}' has no content",
                script.path
            )));
        }
        if let Err(reason) = interpreter::resolve(&script.interpreter) {
            // Unsupported is fatal; merely missing here is not, since the skill
            // may be used on another machine.
            if reason.contains("unsupported") {
                return Err(ProposalRejected(reason));
            }
        }
        let lowered = script.content.to_lowercase();
        for (pattern, why) in DENIED_PATTERNS {
            if lowered.contains(pattern) {
                return Err(ProposalRejected(format!(
                    "script '{}' contains {why}; refusing to propose it",
                    script.path
                )));
            }
        }
    }

    if let Some(existing) = near_duplicate(proposal, catalog) {
        return Err(ProposalRejected(format!(
            "'{existing}' already covers this. Extend that skill instead of adding another."
        )));
    }

    Ok(())
}

/// Finds an existing skill that covers the same ground.
///
/// Deliberately shallow — token overlap on the trigger line, which is what
/// decides when a skill fires. Two skills with different names and near
/// identical triggers are the failure mode that makes a library useless, and
/// this catches it without an embedding model.
///
/// Compared against the resolved trigger rather than the raw field, so a
/// proposal is also checked against skills borrowed from another client, which
/// carry no `when_to_use` at all.
fn near_duplicate(proposal: &SkillProposal, catalog: &SkillCatalog) -> Option<String> {
    let proposed = token_set(&proposal.when_to_use);
    if proposed.is_empty() {
        return None;
    }
    for name in catalog.names() {
        let skill = catalog.get(name)?;
        if skill.name() == proposal.name {
            // Same name is an update, handled separately by `replaces_existing`.
            continue;
        }
        let existing = token_set(&skill.trigger());
        let shared = proposed.iter().filter(|t| existing.contains(*t)).count();
        let smaller = proposed.len().min(existing.len());
        if smaller >= 3 && shared * 4 >= smaller * 3 {
            return Some(name.to_string());
        }
    }
    None
}

fn token_set(text: &str) -> std::collections::BTreeSet<String> {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "to", "of", "for", "in", "on", "is", "are", "when", "user",
        "asks", "about", "with", "it", "that", "this", "be", "as", "at", "by",
    ];
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2 && !STOPWORDS.contains(w))
        .map(String::from)
        .collect()
}

/// Writes an approved proposal into a skill directory.
///
/// Called only after the user accepts. Returns the directory created.
pub fn save(proposal: &SkillProposal, skills_root: &Path) -> std::io::Result<PathBuf> {
    let dir = skills_root.join(&proposal.name);
    std::fs::create_dir_all(&dir)?;

    let frontmatter = proposal.frontmatter();
    let yaml = serde_yaml_ng::to_string(&frontmatter)
        .unwrap_or_else(|_| format!("name: {}\n", proposal.name));
    let contents = format!("---\n{yaml}---\n\n{}\n", proposal.body.trim());
    std::fs::write(dir.join(SKILL_FILE), contents)?;

    for script in &proposal.scripts {
        // Validation already rejected traversal, but the write is the last
        // place this can go wrong, so re-check rather than trust.
        let rel = Path::new(&script.path);
        if rel.is_absolute() || script.path.contains("..") {
            continue;
        }
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &script.content)?;
        make_executable(&path);
    }

    Ok(dir)
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut perms = metadata.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// Receives proposals from the agent. Implemented by the app layer, which
/// surfaces them as review cards.
///
/// Submission does not block: the model proposes a skill and keeps working,
/// and the user reviews when they get to it. Blocking the loop on a human
/// decision that is not required for the current task would be the wrong
/// trade.
#[async_trait]
pub trait ProposalSink: Send + Sync {
    async fn submit(&self, proposal: SkillProposal);
}

/// Collects proposals in memory. Used in tests and headless runs.
#[derive(Default)]
pub struct CollectingSink {
    pub proposals: tokio::sync::Mutex<Vec<SkillProposal>>,
}

#[async_trait]
impl ProposalSink for CollectingSink {
    async fn submit(&self, proposal: SkillProposal) {
        self.proposals.lock().await.push(proposal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::SkillSource;
    use crate::skill::{SkillOrigin, SkillTier};
    use tempfile::TempDir;

    fn proposal() -> SkillProposal {
        SkillProposal::new(
            "release-notes",
            "Assemble release notes from merged pull requests",
            "Preparing release notes for a tagged version",
            "1. List merged PRs since the last tag.\n2. Group them by label.\n3. Write the summary.",
        )
    }

    fn catalog_with(entries: &[(&str, &str)]) -> (SkillCatalog, TempDir) {
        let dir = TempDir::new().unwrap();
        for (name, when) in entries {
            let skill_dir = dir.path().join(name);
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join(SKILL_FILE),
                format!("---\nname: {name}\ndescription: d\nwhen_to_use: {when}\n---\nbody here"),
            )
            .unwrap();
        }
        let (catalog, _) = SkillCatalog::discover(&[SkillSource {
            tier: SkillTier::User,
            origin: SkillOrigin::Taurus,
            dir: dir.path().to_path_buf(),
        }]);
        (catalog, dir)
    }

    #[test]
    fn accepts_a_well_formed_proposal() {
        let (catalog, _d) = catalog_with(&[]);
        assert!(validate_proposal(&proposal(), &catalog).is_ok());
    }

    #[test]
    fn rejects_a_bad_name() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.name = "Release Notes".into();
        assert!(validate_proposal(&p, &catalog).is_err());
    }

    #[test]
    fn rejects_a_stub_procedure() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.body = "do it".into();
        let err = validate_proposal(&p, &catalog).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn rejects_a_destructive_script() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.scripts.push(ProposedScript {
            path: "clean.sh".into(),
            interpreter: "bash".into(),
            description: "tidy up".into(),
            content: "#!/bin/bash\nrm -rf / --no-preserve-root\n".into(),
        });
        let err = validate_proposal(&p, &catalog).unwrap_err();
        assert!(err.to_string().contains("recursive delete"));
    }

    #[test]
    fn rejects_a_script_with_no_source() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.scripts.push(ProposedScript {
            path: "run.py".into(),
            interpreter: "python3".into(),
            description: String::new(),
            content: "   ".into(),
        });
        assert!(validate_proposal(&p, &catalog).is_err());
    }

    #[test]
    fn rejects_a_near_duplicate_of_an_existing_skill() {
        let (catalog, _d) = catalog_with(&[(
            "changelog-builder",
            "Preparing release notes for a tagged version",
        )]);
        let err = validate_proposal(&proposal(), &catalog).unwrap_err();
        assert!(err.to_string().contains("changelog-builder"));
    }

    #[test]
    fn allows_a_genuinely_different_skill_alongside_existing_ones() {
        let (catalog, _d) =
            catalog_with(&[("pdf-extract", "Reading tables out of a PDF document")]);
        assert!(validate_proposal(&proposal(), &catalog).is_ok());
    }

    #[test]
    fn saving_produces_a_skill_the_catalog_can_load_back() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.scripts.push(ProposedScript {
            path: "collect.sh".into(),
            interpreter: "bash".into(),
            description: "collect PRs".into(),
            content: "#!/bin/bash\necho collecting\n".into(),
        });
        validate_proposal(&p, &catalog).unwrap();

        let root = TempDir::new().unwrap();
        let dir = save(&p, root.path()).unwrap();
        assert!(dir.join(SKILL_FILE).exists());
        assert!(dir.join("collect.sh").exists());

        // The round trip is the real assertion: a proposal that saves but does
        // not load back is worse than one that is rejected.
        let (reloaded, problems) = SkillCatalog::discover(&[SkillSource {
            tier: SkillTier::Project,
            origin: SkillOrigin::Taurus,
            dir: root.path().to_path_buf(),
        }]);
        assert!(problems.is_empty(), "{problems:?}");
        let skill = reloaded
            .get("release-notes")
            .expect("skill did not load back");
        assert_eq!(
            skill.frontmatter.when_to_use.as_deref(),
            Some(p.when_to_use.as_str())
        );
        assert_eq!(skill.frontmatter.scripts.len(), 1);
        assert!(skill.body.contains("List merged PRs"));
        assert!(
            skill.warnings.is_empty(),
            "a skill Taurus writes must not need leniency to load: {:?}",
            skill.warnings
        );
    }

    #[test]
    fn what_gets_written_is_a_file_other_clients_can_read() {
        let (catalog, _d) = catalog_with(&[]);
        let p = proposal();
        validate_proposal(&p, &catalog).unwrap();

        let root = TempDir::new().unwrap();
        let dir = save(&p, root.path()).unwrap();
        let text = std::fs::read_to_string(dir.join(SKILL_FILE)).unwrap();

        // The two fields the specification requires, and none of the empty
        // ones: a frontmatter full of `null` and `[]` is a worse file to hand
        // somebody than a short one.
        assert!(text.contains("name: release-notes"));
        assert!(text.contains("description:"));
        for empty in ["compatibility:", "allowed_tools:", "scripts:"] {
            assert!(!text.contains(empty), "{empty} should be omitted:\n{text}");
        }
    }

    #[test]
    fn a_proposal_that_would_only_load_by_leniency_is_rejected() {
        // Loading from disk warns and carries on for a name in the wrong case.
        // Writing a new one is a choice, and this is the moment to refuse it.
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.name = "Release_Notes".into();

        let err = validate_proposal(&p, &catalog).unwrap_err();
        assert!(err.to_string().contains("kebab-case"), "{err}");
    }

    #[tokio::test]
    async fn the_collecting_sink_records_submissions() {
        let sink = CollectingSink::default();
        sink.submit(proposal()).await;
        assert_eq!(sink.proposals.lock().await.len(), 1);
    }
}
