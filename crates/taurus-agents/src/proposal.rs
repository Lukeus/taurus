//! Auto-generated agents: proposal, validation, and the write on approval.
//!
//! The same shape as a skill proposal and for the same reason. Nothing here
//! touches the roster: a proposal is inert data until the user approves it,
//! because a model that can silently write its own delegates can also silently
//! write one that misbehaves on every future turn.
//!
//! What makes that a bounded risk rather than an open one is that an agent
//! cannot reach past the session that proposed it. `tools:` only ever narrows —
//! the host refuses an agent naming tools this session does not have, and every
//! call a child makes still goes through the parent's permission engine. The
//! spawn tool is absent from a child's registry, so a proposed agent cannot
//! spawn or propose further ones. The ceiling is structural; this file only has
//! to stop a bad *instruction* reaching the roster unread.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::agent::{validate, AgentFrontmatter};
use crate::catalog::AgentCatalog;

/// Shortest system prompt worth saving.
///
/// The body *is* the agent — an empty one is already refused by [`validate`],
/// but "review the code" is a name with nothing behind it, and the model that
/// wrote it will be the one let down by it later.
const MIN_BODY_CHARS: usize = 40;

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentProposal {
    pub id: String,
    pub name: String,
    /// What the parent reads when choosing a delegate.
    pub description: String,
    /// The system prompt, which is the file's body.
    pub prompt: String,
    /// Tools the agent may call. `None` inherits the parent's set.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    pub max_iterations: u32,
    /// Why the agent thinks this is worth keeping. Shown in the review card;
    /// never written to disk.
    #[serde(default)]
    pub rationale: String,
    /// True when an agent of this name already exists and would be replaced.
    #[serde(default)]
    pub replaces_existing: bool,
}

// `model:` and `provider:` are deliberately absent, and not merely unset. A
// model choosing which model its delegate runs on is choosing what that
// delegate costs, on a provider the user pays for, and it is the one field on
// the format with no bearing on what the agent can *do*. An approved agent
// inherits the session's model, and a user who wants otherwise edits the file —
// where the decision is theirs and visible.

impl AgentProposal {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            description: description.into(),
            prompt: prompt.into(),
            tools: None,
            max_iterations: 20,
            rationale: String::new(),
            replaces_existing: false,
        }
    }

    pub fn frontmatter(&self) -> AgentFrontmatter {
        AgentFrontmatter {
            name: self.name.clone(),
            description: self.description.clone(),
            tools: self.tools.clone(),
            max_iterations: self.max_iterations,
            model: None,
            provider: None,
        }
    }
}

/// Where an approved proposal gets written.
///
/// Renamed on the way to TypeScript: a skill proposal has its own `SaveTarget`
/// with the same two variants, and ts-rs writes one file per exported type. Two
/// types of the same name would have one silently overwrite the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "AgentSaveTarget")]
pub enum SaveTarget {
    /// `<workspace>/.taurus/agents` — travels with the project.
    Project,
    /// `~/.taurus/agents` — available in every workspace.
    User,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ProposalRejected(pub String);

/// Checks a proposal before it reaches the user.
///
/// Everything here is something a human reviewer would either miss or find
/// tedious to check: the format rules, a prompt too thin to be worth a file, a
/// tool the session cannot offer, and a delegate that duplicates one already on
/// the roster.
///
/// `available` is the session's tool names. A proposal naming anything outside
/// them is refused here rather than saved and degraded later — the host would
/// drop the missing tools and run the agent with the rest, which is right for a
/// file a user wrote on another machine and wrong for one being written now,
/// where the mistake can simply be corrected.
pub fn validate_proposal(
    proposal: &AgentProposal,
    catalog: &AgentCatalog,
    available: &[String],
) -> Result<(), ProposalRejected> {
    let frontmatter = proposal.frontmatter();
    validate(&frontmatter, &proposal.prompt, "proposal")
        .map_err(|e| ProposalRejected(e.to_string()))?;

    if proposal.prompt.trim().chars().count() < MIN_BODY_CHARS {
        return Err(ProposalRejected(
            "the system prompt is too short to be worth saving; write what this agent should \
             actually do, and what it should not"
                .into(),
        ));
    }

    if let Some(tools) = &proposal.tools {
        let missing: Vec<&str> = tools
            .iter()
            .map(String::as_str)
            .filter(|name| !available.iter().any(|have| have == name))
            .collect();
        if !missing.is_empty() {
            return Err(ProposalRejected(format!(
                "this session has no tool called {}; scope the agent to tools that exist here, or \
                 leave tools out to inherit",
                missing.join(", ")
            )));
        }
    }

    if let Some(existing) = near_duplicate(proposal, catalog) {
        return Err(ProposalRejected(format!(
            "'{existing}' already covers this. Delegate to that agent instead of adding another."
        )));
    }

    Ok(())
}

/// Finds an existing agent that covers the same ground.
///
/// Token overlap on `description`, which is the field that decides when an
/// agent is picked — the same shallow check a skill proposal runs against
/// `when_to_use`, for the same reason. Two delegates with different names and
/// near-identical descriptions make the roster a coin toss, and every line of
/// it is paid for on every request.
fn near_duplicate(proposal: &AgentProposal, catalog: &AgentCatalog) -> Option<String> {
    let proposed = token_set(&proposal.description);
    if proposed.is_empty() {
        return None;
    }
    for agent in catalog.iter() {
        // Same name is an update, handled separately by `replaces_existing`.
        if agent.name() == proposal.name {
            continue;
        }
        let existing = token_set(&agent.frontmatter.description);
        let shared = proposed.iter().filter(|t| existing.contains(*t)).count();
        let smaller = proposed.len().min(existing.len());
        if smaller >= 3 && shared * 4 >= smaller * 3 {
            return Some(agent.name().to_string());
        }
    }
    None
}

fn token_set(text: &str) -> std::collections::BTreeSet<String> {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "to", "of", "for", "in", "on", "is", "are", "when", "user",
        "asks", "about", "with", "it", "that", "this", "be", "as", "at", "by", "use", "used",
    ];
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2 && !STOPWORDS.contains(w))
        .map(String::from)
        .collect()
}

/// Writes an approved proposal into an agents directory.
///
/// Called only after the user accepts. Returns the file written.
pub fn save(proposal: &AgentProposal, agents_root: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(agents_root)?;

    let frontmatter = proposal.frontmatter();
    let yaml = serde_yaml_ng::to_string(&frontmatter)
        .unwrap_or_else(|_| format!("name: {}\n", proposal.name));
    let contents = format!("---\n{yaml}---\n\n{}\n", proposal.prompt.trim());

    // Validation already rejected anything but kebab-case, so the name cannot
    // carry a separator — but this is the last place it could go wrong, and a
    // filename is built from it.
    let path = agents_root.join(format!("{}.md", proposal.name));
    std::fs::write(&path, contents)?;
    Ok(path)
}

/// Receives proposals from the agent. Implemented by the app layer, which
/// surfaces them as review cards.
///
/// Submission does not block: the model proposes an agent and keeps working,
/// and the user reviews when they get to it. The proposed delegate is of no use
/// to the turn that wrote it — a roster is snapshotted when a turn starts — so
/// there is nothing to wait for even in principle.
#[async_trait]
pub trait AgentProposalSink: Send + Sync {
    async fn submit(&self, proposal: AgentProposal);
}

/// Collects proposals in memory. Used in tests and headless runs.
#[derive(Default)]
pub struct CollectingSink {
    pub proposals: tokio::sync::Mutex<Vec<AgentProposal>>,
}

#[async_trait]
impl AgentProposalSink for CollectingSink {
    async fn submit(&self, proposal: AgentProposal) {
        self.proposals.lock().await.push(proposal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentTier;
    use crate::catalog::AgentSource;
    use tempfile::TempDir;

    const PROMPT: &str = "You review a diff for correctness bugs only. Report each finding with \
                          the file and line it is on, and say so plainly when you find nothing.";

    fn proposal() -> AgentProposal {
        AgentProposal::new(
            "diff-reviewer",
            "Reviews a diff for correctness bugs and reports what it found",
            PROMPT,
        )
    }

    /// A catalog holding only what is passed in — no built-ins, so a duplicate
    /// test is not fighting `explorer` and `worker`.
    fn catalog_with(entries: &[(&str, &str)]) -> (AgentCatalog, TempDir) {
        let dir = TempDir::new().unwrap();
        for (name, description) in entries {
            std::fs::write(
                dir.path().join(format!("{name}.md")),
                format!("---\nname: {name}\ndescription: {description}\n---\n{PROMPT}"),
            )
            .unwrap();
        }
        let (catalog, problems) = AgentCatalog::discover(&[AgentSource {
            tier: AgentTier::User,
            dir: dir.path().to_path_buf(),
        }]);
        assert!(problems.is_empty(), "{problems:?}");
        (catalog, dir)
    }

    fn tools() -> Vec<String> {
        ["read_file", "grep", "glob"].map(String::from).to_vec()
    }

    #[test]
    fn a_sound_proposal_passes() {
        let (catalog, _dir) = catalog_with(&[]);
        assert!(validate_proposal(&proposal(), &catalog, &tools()).is_ok());
    }

    #[test]
    fn a_name_that_is_not_kebab_case_is_refused() {
        let (catalog, _dir) = catalog_with(&[]);
        let mut p = proposal();
        p.name = "Diff Reviewer".into();
        let error = validate_proposal(&p, &catalog, &tools()).unwrap_err();
        assert!(error.to_string().contains("kebab-case"), "{error}");
    }

    #[test]
    fn a_prompt_with_nothing_in_it_is_refused() {
        // An agent is its system prompt. A file whose body says "review code"
        // is a roster line that costs tokens on every request and delivers a
        // delegate with no instructions.
        let (catalog, _dir) = catalog_with(&[]);
        let mut p = proposal();
        p.prompt = "Review the code.".into();
        let error = validate_proposal(&p, &catalog, &tools()).unwrap_err();
        assert!(error.to_string().contains("too short"), "{error}");
    }

    #[test]
    fn a_tool_this_session_does_not_have_is_refused() {
        // Saved and degraded is right for a file written on another machine,
        // and wrong for one being written now — here the mistake can just be
        // corrected before it is kept.
        let (catalog, _dir) = catalog_with(&[]);
        let mut p = proposal();
        p.tools = Some(vec!["read_file".into(), "send_email".into()]);
        let error = validate_proposal(&p, &catalog, &tools()).unwrap_err();
        assert!(error.to_string().contains("send_email"), "{error}");
    }

    #[test]
    fn a_scope_of_tools_that_all_exist_is_allowed() {
        let (catalog, _dir) = catalog_with(&[]);
        let mut p = proposal();
        p.tools = Some(vec!["read_file".into(), "grep".into()]);
        assert!(validate_proposal(&p, &catalog, &tools()).is_ok());
    }

    #[test]
    fn an_agent_that_already_covers_this_is_refused() {
        let (catalog, _dir) = catalog_with(&[(
            "code-reviewer",
            "Reviews a diff for correctness bugs and reports what it found",
        )]);
        let error = validate_proposal(&proposal(), &catalog, &tools()).unwrap_err();
        assert!(error.to_string().contains("code-reviewer"), "{error}");
    }

    #[test]
    fn replacing_an_agent_of_the_same_name_is_not_a_duplicate() {
        // Updating an agent is the one case where an identical description is
        // exactly right, and the card says `replaces existing` instead.
        let (catalog, _dir) = catalog_with(&[(
            "diff-reviewer",
            "Reviews a diff for correctness bugs and reports what it found",
        )]);
        assert!(validate_proposal(&proposal(), &catalog, &tools()).is_ok());
    }

    #[test]
    fn an_approved_proposal_is_written_where_discovery_finds_it() {
        // The round trip that matters: what `save` writes has to be what
        // `discover` reads, or an approved agent lands on disk and never
        // appears on the roster.
        let dir = TempDir::new().unwrap();
        let mut p = proposal();
        p.tools = Some(vec!["read_file".into(), "grep".into()]);

        let path = save(&p, dir.path()).unwrap();
        assert_eq!(path.file_name().unwrap(), "diff-reviewer.md");

        let (catalog, problems) = AgentCatalog::discover(&[AgentSource {
            tier: AgentTier::User,
            dir: dir.path().to_path_buf(),
        }]);
        assert!(problems.is_empty(), "{problems:?}");
        let saved = catalog.get("diff-reviewer").expect("not on the roster");
        assert_eq!(saved.frontmatter.description, p.description);
        assert_eq!(saved.frontmatter.tools, p.tools);
        assert_eq!(saved.system_prompt, PROMPT);
    }

    #[test]
    fn a_saved_agent_names_no_model_of_its_own() {
        // The field is absent from the proposal by design; this pins that it
        // stays absent from the file, so an approved agent runs on whatever the
        // session runs on.
        let dir = TempDir::new().unwrap();
        save(&proposal(), dir.path()).unwrap();
        let written = std::fs::read_to_string(dir.path().join("diff-reviewer.md")).unwrap();
        assert!(!written.contains("model:"), "{written}");
        assert!(!written.contains("provider:"), "{written}");
    }
}
