//! Sub-agents: the kinds of delegate a turn can hand work to.
//!
//! An agent is a markdown file with YAML frontmatter — the body is its system
//! prompt — discovered from `~/.taurus/agents` and `<workspace>/.taurus/agents`
//! on the same shadowing rule skills use. `explorer` and `worker` are compiled
//! in and can be replaced by a file of the same name.
//!
//! This crate is pure data: it parses, validates, and discovers, and it knows
//! nothing about providers, registries, or the tool that spawns anything. That
//! is what keeps it a leaf with no workspace dependencies, and what lets the
//! rules be tested without a running harness.

pub mod agent;
pub mod builtin;
pub mod catalog;
pub mod proposal;

pub use agent::{
    parse_agent_md, validate, AgentDefinition, AgentError, AgentFrontmatter, AgentSummary,
    AgentTier, DESCRIPTION_LIMIT, MAX_ITERATIONS_LIMIT,
};
pub use catalog::{AgentCatalog, AgentSource};
pub use proposal::{AgentProposal, AgentProposalSink, SaveTarget};
