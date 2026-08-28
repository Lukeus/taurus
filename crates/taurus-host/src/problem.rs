//! Things that went wrong at load time, and where they came from.
//!
//! Everything a reload can fail at — a provider file that will not parse, a
//! search backend with no key, an MCP server that would not start, a malformed
//! skill — is non-fatal by design: one broken layer must not cost the others.
//! They collect here instead, and are shown rather than swallowed.
//!
//! The source is carried with the message because the alternative was tried.
//! A single untagged list has to be rendered somewhere, that somewhere ends up
//! being one screen, and every problem that does not belong to it is then
//! reported in a place nobody would look for it — a malformed `providers.json`
//! announcing itself under a list of skills. A tag is what lets each problem
//! reach the screen that can act on it.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Which part of the configuration a problem came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProblemSource {
    /// `providers.json` — the model backends.
    Providers,
    /// `search.json` — the web search backend.
    Search,
    /// `mcp.json`, or a server that would not connect.
    Mcp,
    /// A skill directory that would not load.
    Skills,
    /// An agent file that would not load, or one that cannot run as written.
    Agents,
    /// `settings.json` — the tools the agent is allowed to have.
    Tools,
    /// An `AGENTS.md`, `CLAUDE.md`, or `TAURUS.md` that could not be read whole.
    Instructions,
    /// `hooks.json` — an entry that will not load, or a toggle with nothing to
    /// toggle.
    Hooks,
    /// A file under `themes/` that will not load, or names a colour, a size or
    /// a logo the app cannot use.
    Themes,
}

impl ProblemSource {
    /// Where a person goes to fix this, in the words the UI uses for it.
    pub fn where_to_fix(self) -> &'static str {
        match self {
            Self::Providers => "Settings › Models",
            Self::Search => "Settings › Search",
            // The drawer that lists the roster, which is where a broken agent
            // file is both visible and explicable.
            Self::Agents => "Agents",
            // The screen that lists what the agent can reach, which is exactly
            // what a disabled tool changes.
            // The drawer that lists what the agent knows and can reach, which
            // is where a brief that is not arriving belongs.
            Self::Themes => "Settings › Appearance",
            Self::Mcp | Self::Skills | Self::Tools | Self::Instructions | Self::Hooks => "Skills",
        }
    }

    /// What to call this on a terminal, where there are no screens to point at.
    pub fn label(self) -> &'static str {
        match self {
            Self::Providers => "providers",
            Self::Search => "search",
            Self::Mcp => "mcp",
            Self::Skills => "skills",
            Self::Agents => "agents",
            Self::Tools => "tools",
            Self::Instructions => "instructions",
            Self::Hooks => "hooks",
            Self::Themes => "themes",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Problem {
    pub source: ProblemSource,
    pub message: String,
}

/// `providers: providers.json is not valid JSON` — the source is kept in the
/// rendering, because on a terminal it is the only thing saying which file to
/// go and open.
impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.source.label(), self.message)
    }
}

impl Problem {
    pub fn new(source: ProblemSource, message: impl Into<String>) -> Self {
        Self {
            source,
            message: message.into(),
        }
    }

    /// Tags a batch from one loader in a single step, which is how every call
    /// site uses this — a loader knows its own source and nothing else's.
    pub fn tag<I>(source: ProblemSource, messages: I) -> Vec<Self>
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        messages
            .into_iter()
            .map(|message| Self::new(source, message))
            .collect()
    }
}

/// Filters a problem list to the sources one screen is responsible for.
pub fn of(problems: &[Problem], sources: &[ProblemSource]) -> Vec<Problem> {
    problems
        .iter()
        .filter(|problem| sources.contains(&problem.source))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagging_a_batch_keeps_every_message() {
        let tagged = Problem::tag(
            ProblemSource::Skills,
            vec!["first is broken".to_string(), "second is too".to_string()],
        );
        assert_eq!(tagged.len(), 2);
        assert!(tagged.iter().all(|p| p.source == ProblemSource::Skills));
        assert_eq!(tagged[1].message, "second is too");
    }

    #[test]
    fn filtering_keeps_only_the_sources_asked_for() {
        let problems = vec![
            Problem::new(ProblemSource::Providers, "providers.json is not json"),
            Problem::new(ProblemSource::Skills, "skill has no frontmatter"),
            Problem::new(ProblemSource::Mcp, "server would not start"),
        ];

        let library = of(&problems, &[ProblemSource::Skills, ProblemSource::Mcp]);
        assert_eq!(library.len(), 2);
        assert!(library.iter().all(|p| p.source != ProblemSource::Providers));

        let settings = of(&problems, &[ProblemSource::Providers]);
        assert_eq!(settings.len(), 1);
        assert!(settings[0].message.contains("providers.json"));
    }

    #[test]
    fn every_source_names_somewhere_to_go() {
        // A problem that reaches the UI without a screen behind it is one the
        // user can read and not act on.
        for source in [
            ProblemSource::Providers,
            ProblemSource::Search,
            ProblemSource::Mcp,
            ProblemSource::Skills,
            ProblemSource::Agents,
            ProblemSource::Tools,
            ProblemSource::Instructions,
        ] {
            assert!(!source.where_to_fix().is_empty());
        }
    }
}
