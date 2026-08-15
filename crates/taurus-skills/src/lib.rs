//! Skills: written-down procedures the agent can look up, run, and author.
//!
//! The design constraint throughout is the context window. A useful library
//! has dozens of skills; a local model has 8k tokens. So the system prompt
//! carries one line per skill and nothing more, and the model pulls the full
//! procedure only for the skill it actually picked.

pub mod catalog;
pub mod interpreter;
pub mod proposal;
pub mod skill;
pub mod tools;

pub use catalog::{CommandError, Invocation, SkillCatalog, SkillSource};
pub use proposal::{ProposalSink, SaveTarget, SkillProposal};
pub use skill::{Skill, SkillError, SkillOrigin, SkillSummary, SkillTier};
pub use tools::{LoadSkill, ProposeSkill, RunSkillScript, SharedCatalog, PROPOSE_TOOL};
