//! Sub-agent definitions and frontmatter parsing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

/// Longest a description may be. It is paid on every request — the roster sits
/// in the spawn tool's description — so it is held to the same 200 characters
/// a skill's `when_to_use` is, and for the same reason.
pub const DESCRIPTION_LIMIT: usize = 200;

/// Ceiling on `max_iterations`. A file saying `100000` against a concurrency of
/// two is a runaway, and the ceiling is cheaper than the incident.
pub const MAX_ITERATIONS_LIMIT: u32 = 50;

/// Where an agent came from. Later tiers shadow earlier ones by name, so a
/// project can override a personal agent, and either can override a built-in.
///
/// Unlike [`taurus_skills::SkillTier`], there *is* a `Builtin`: `explorer`,
/// `worker`, and `coder` ship with the harness and always have, so a variant
/// for them is describing what exists rather than reserving a label for
/// something that might. A user file named `explorer.md` replaces the built-in,
/// which is the natural way to say "your explorer, but with the shell".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentTier {
    /// Compiled in. Never read from disk, so no filesystem state can remove it.
    Builtin,
    /// `~/.taurus/agents`
    User,
    /// `<workspace>/.taurus/agents`
    Project,
}

impl AgentTier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Builtin => "built-in",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct AgentFrontmatter {
    pub name: String,
    /// What the parent reads when choosing a sub-agent. Capped at
    /// [`DESCRIPTION_LIMIT`]; see the constant for why.
    pub description: String,
    /// Tools this agent may call.
    ///
    /// Enforcing, unlike a skill's `allowed_tools`, which is advisory and grants
    /// nothing. This one genuinely narrows the child's registry, which is why it
    /// is spelled `tools` — two identically named keys with opposite force
    /// across two sibling file formats is a trap that costs nothing to avoid
    /// now and is unfixable later.
    ///
    /// `None` — the key absent — means inherit the parent's set minus the spawn
    /// tool, which is what `worker` has always done. That is deliberately *not*
    /// the same as an empty list: an agent whose named tools all turned out to
    /// be unavailable must not silently widen into "everything". Keeping the
    /// distinction in the type is what makes that checkable downstream.
    ///
    /// Skipped when absent on the way *out*, so a generated agent file reads
    /// like a hand-written one. A person writing this file omits the key; a
    /// serializer left alone writes `tools: null`, which is the same thing to
    /// serde and an eyesore to the user who opens it next.
    #[serde(
        default,
        deserialize_with = "deserialize_tools",
        skip_serializing_if = "Option::is_none"
    )]
    pub tools: Option<Vec<String>>,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// Run this agent on a different model than the session's. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Which configured provider `model` belongs to. Optional; defaults to the
    /// session's provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

fn default_max_iterations() -> u32 {
    20
}

/// Accepts both spellings of a tool list.
///
/// This plan writes `tools: [read_file, grep]`; Claude Code writes
/// `tools: Read, Glob, Grep` as one comma-separated string. Taking either costs
/// a match arm and means a subagent file written for another harness parses
/// here rather than failing on its punctuation.
fn deserialize_tools<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ToolList {
        Csv(String),
        List(Vec<String>),
    }

    let raw = Option::<ToolList>::deserialize(deserializer)?;
    Ok(raw.map(|list| {
        let names = match list {
            ToolList::Csv(text) => text.split(',').map(str::to_string).collect(),
            ToolList::List(names) => names,
        };
        names
            .into_iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect()
    }))
}

#[derive(Clone, Debug)]
pub struct AgentDefinition {
    pub frontmatter: AgentFrontmatter,
    /// The markdown body: this agent's system prompt.
    pub system_prompt: String,
    pub tier: AgentTier,
    /// `None` for built-ins, which have no file.
    pub path: Option<PathBuf>,
    /// The tier this definition displaced, when it shadowed one.
    pub shadows: Option<AgentTier>,
    /// Set when the agent cannot fully run as written — an unresolvable model,
    /// or a tool it names that this session does not have. Filled in by the
    /// host, which is the only layer that can see either.
    pub degraded: Option<String>,
}

impl AgentDefinition {
    pub fn name(&self) -> &str {
        &self.frontmatter.name
    }

    /// The one line this agent contributes to the spawn tool's description.
    pub fn roster_line(&self) -> String {
        format!(
            "- {}: {}",
            self.frontmatter.name, self.frontmatter.description
        )
    }
}

/// An agent summary for the UI and the CLI. Excludes the system prompt, which
/// can be long.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSummary {
    pub name: String,
    pub description: String,
    pub tier: AgentTier,
    /// Enforced. `None` means the agent inherits the parent's tools.
    pub tools: Option<Vec<String>>,
    pub max_iterations: u32,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub shadows: Option<AgentTier>,
    pub degraded: Option<String>,
    /// The file it was read from. `None` for built-ins.
    pub path: Option<String>,
}

impl From<&AgentDefinition> for AgentSummary {
    fn from(agent: &AgentDefinition) -> Self {
        Self {
            name: agent.frontmatter.name.clone(),
            description: agent.frontmatter.description.clone(),
            tier: agent.tier,
            tools: agent.frontmatter.tools.clone(),
            max_iterations: agent.frontmatter.max_iterations,
            model: agent.frontmatter.model.clone(),
            provider: agent.frontmatter.provider.clone(),
            shadows: agent.shadows,
            degraded: agent.degraded.clone(),
            path: agent.path.as_ref().map(|p| p.display().to_string()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("{path}: missing YAML frontmatter (the file must start with a --- line)")]
    NoFrontmatter { path: String },

    #[error("{path}: frontmatter is not valid YAML: {source}")]
    BadYaml {
        path: String,
        #[source]
        source: serde_yaml_ng::Error,
    },

    #[error("{path}: {message}")]
    Invalid { path: String, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Splits `---\nyaml\n---\nbody` into its two halves.
pub fn parse_agent_md(text: &str, path: &Path) -> Result<(AgentFrontmatter, String), AgentError> {
    let display = path.display().to_string();
    // Tolerate a leading BOM and Windows line endings, for the same reason
    // skills do: these files get authored by models and edited by humans on
    // every platform.
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| AgentError::NoFrontmatter {
            path: display.clone(),
        })?;
    let (yaml, body) = rest
        .split_once("\n---\n")
        .or_else(|| rest.split_once("\n---"))
        .ok_or_else(|| AgentError::NoFrontmatter {
            path: display.clone(),
        })?;

    let frontmatter: AgentFrontmatter =
        serde_yaml_ng::from_str(yaml).map_err(|source| AgentError::BadYaml {
            path: display.clone(),
            source,
        })?;

    Ok((frontmatter, body.trim().to_string()))
}

/// Rules an agent must satisfy to enter the catalog.
///
/// Tool names are deliberately not checked here: the registry is not visible
/// from this crate, and a list that looks wrong from here may be a perfectly
/// good MCP tool. That check belongs to the host, which sees the finished
/// registry, and it happens there.
pub fn validate(frontmatter: &AgentFrontmatter, body: &str, path: &str) -> Result<(), AgentError> {
    let invalid = |message: String| AgentError::Invalid {
        path: path.to_string(),
        message,
    };

    if !is_kebab_case(&frontmatter.name) {
        return Err(invalid(format!(
            "name '{}' must be kebab-case (lowercase letters, digits, and hyphens), \
             like 'code-reviewer'",
            frontmatter.name
        )));
    }
    if frontmatter.description.trim().is_empty() {
        return Err(invalid(
            "description must not be empty; it is the only text the parent agent sees when \
             choosing which sub-agent to delegate to"
                .into(),
        ));
    }
    if frontmatter.description.chars().count() > DESCRIPTION_LIMIT {
        return Err(invalid(format!(
            "description is {} characters; keep it under {DESCRIPTION_LIMIT} so the roster stays \
             cheap — it is sent on every request",
            frontmatter.description.chars().count()
        )));
    }
    if body.trim().is_empty() {
        return Err(invalid(
            "the body below the frontmatter is empty; it is this agent's system prompt, and an \
             agent without one is a name with no behaviour behind it"
                .into(),
        ));
    }
    if frontmatter.max_iterations == 0 || frontmatter.max_iterations > MAX_ITERATIONS_LIMIT {
        return Err(invalid(format!(
            "max_iterations is {}; it must be between 1 and {MAX_ITERATIONS_LIMIT}",
            frontmatter.max_iterations
        )));
    }
    // An empty list is not the same as an absent one, and neither is what the
    // author meant: `tools: []` reads as "no tools", which would be an agent
    // that can only talk. Say which of the two they probably wanted.
    if frontmatter.tools.as_ref().is_some_and(Vec::is_empty) {
        return Err(invalid(
            "tools is empty; remove the key entirely to inherit the parent's tools, or list the \
             tools this agent may call"
                .into(),
        ));
    }
    if frontmatter.provider.is_some() && frontmatter.model.is_none() {
        return Err(invalid(
            "provider is set but model is not; a provider on its own selects nothing, so either \
             name a model or drop the provider"
                .into(),
        ));
    }
    Ok(())
}

fn is_kebab_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"---
name: reviewer
description: Reviews a diff for correctness bugs. Use after a change is written.
tools: [read_file, grep, glob]
max_iterations: 20
---

You are a review sub-agent. Report only defects you can point at a line for.
"#;

    fn fm(name: &str, description: &str) -> AgentFrontmatter {
        AgentFrontmatter {
            name: name.into(),
            description: description.into(),
            tools: None,
            max_iterations: 20,
            model: None,
            provider: None,
        }
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let (front, body) = parse_agent_md(GOOD, Path::new("reviewer.md")).unwrap();
        assert_eq!(front.name, "reviewer");
        assert_eq!(
            front.tools.as_deref(),
            Some(["read_file", "grep", "glob"].map(String::from).as_slice())
        );
        assert_eq!(front.max_iterations, 20);
        assert!(body.starts_with("You are a review sub-agent."));
    }

    #[test]
    fn parses_crlf_and_bom() {
        let text = format!("\u{feff}{}", GOOD.replace('\n', "\r\n"));
        let (front, body) = parse_agent_md(&text, Path::new("reviewer.md")).unwrap();
        assert_eq!(front.name, "reviewer");
        assert!(body.contains("review sub-agent"));
    }

    #[test]
    fn max_iterations_defaults_when_absent() {
        let text = "---\nname: a\ndescription: d\n---\nbody";
        let (front, _) = parse_agent_md(text, Path::new("a.md")).unwrap();
        assert_eq!(front.max_iterations, default_max_iterations());
    }

    #[test]
    fn an_absent_tools_key_is_none_not_empty() {
        // The whole of R4 rests on these being different values.
        let text = "---\nname: a\ndescription: d\n---\nbody";
        let (front, _) = parse_agent_md(text, Path::new("a.md")).unwrap();
        assert!(front.tools.is_none());
    }

    #[test]
    fn parses_a_comma_separated_tool_list() {
        // The spelling Claude Code writes.
        let text = "---\nname: a\ndescription: d\ntools: Read, Glob, Grep\n---\nbody";
        let (front, _) = parse_agent_md(text, Path::new("a.md")).unwrap();
        assert_eq!(
            front.tools.as_deref(),
            Some(["Read", "Glob", "Grep"].map(String::from).as_slice())
        );
    }

    #[test]
    fn rejects_a_file_without_frontmatter() {
        let err = parse_agent_md("# Just markdown\n", Path::new("a.md")).unwrap_err();
        assert!(matches!(err, AgentError::NoFrontmatter { .. }));
    }

    #[test]
    fn rejects_invalid_yaml() {
        let err = parse_agent_md("---\nname: [unclosed\n---\nbody", Path::new("a.md")).unwrap_err();
        assert!(matches!(err, AgentError::BadYaml { .. }));
    }

    #[test]
    fn accepts_a_valid_frontmatter() {
        assert!(validate(&fm("reviewer", "reviews diffs"), "body", "p").is_ok());
    }

    #[test]
    fn rejects_a_non_kebab_case_name() {
        for bad in ["My_Agent", "my agent", "-lead", "trail-", "MyAgent"] {
            assert!(
                validate(&fm(bad, "d"), "body", "p").is_err(),
                "'{bad}' should be rejected"
            );
        }
    }

    #[test]
    fn rejects_an_empty_description() {
        assert!(validate(&fm("ok-name", "   "), "body", "p").is_err());
    }

    #[test]
    fn rejects_an_overlong_description() {
        let err = validate(&fm("ok-name", &"x".repeat(201)), "body", "p").unwrap_err();
        assert!(err.to_string().contains("201 characters"));
    }

    #[test]
    fn rejects_an_empty_body() {
        let err = validate(&fm("ok-name", "d"), "  \n ", "p").unwrap_err();
        assert!(err.to_string().contains("system prompt"));
    }

    #[test]
    fn rejects_max_iterations_outside_the_range() {
        for bad in [0, MAX_ITERATIONS_LIMIT + 1, 100_000] {
            let mut front = fm("ok-name", "d");
            front.max_iterations = bad;
            assert!(
                validate(&front, "body", "p").is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_an_explicitly_empty_tool_list() {
        let mut front = fm("ok-name", "d");
        front.tools = Some(vec![]);
        let err = validate(&front, "body", "p").unwrap_err();
        assert!(err.to_string().contains("remove the key"));
    }

    #[test]
    fn rejects_a_provider_without_a_model() {
        let mut front = fm("ok-name", "d");
        front.provider = Some("ollama".into());
        let err = validate(&front, "body", "p").unwrap_err();
        assert!(err.to_string().contains("selects nothing"));
    }

    #[test]
    fn accepts_a_model_without_a_provider() {
        let mut front = fm("ok-name", "d");
        front.model = Some("qwen3:32b".into());
        assert!(validate(&front, "body", "p").is_ok());
    }
}
