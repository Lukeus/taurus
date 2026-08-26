//! Everything both frontends need, and nothing either one owns alone.
//!
//! `src-tauri` and `taurus-cli` differ in how they talk to a person. They do
//! not differ in what a Taurus session *is* — same config files, same system
//! prompt, same tool registry, same skill library. That shared part lives here
//! so the two cannot drift apart.

pub mod attach;
pub mod command;
pub mod config;
pub mod freshness;
pub mod git;
pub mod host;
pub mod instructions;
pub mod mcp_view;
pub mod memory;
pub mod onscreen;
pub mod problem;
pub mod prompt;
pub mod search;
pub mod secrets;
pub mod sessions;
#[cfg(test)]
mod testing;
pub mod trust;
pub mod usage;

pub use attach::Attachment;
pub use command::{CommandError, CommandKind, CommandSummary, Invocation};
pub use config::{ProviderConfig, ProviderKind, Scope, Settings, Theme};
pub use git::{Commit, Repo, RepoStatus};
pub use host::{Host, PermissionPromptFactory, TurnRef, MAX_CONCURRENT_SUBAGENTS};
pub use instructions::{Instructions, InstructionsOrigin, InstructionsSource, InstructionsTier};
pub use mcp_view::{McpServerDraft, McpServerRef, McpServerView, McpTransport, McpValue};
pub use memory::Note;
pub use onscreen::OnScreen;
pub use problem::{Problem, ProblemSource};
pub use search::{Match, SearchResults, SessionHit};
pub use secrets::KeyStatus;
pub use sessions::{SessionLog, SessionMeta, SubagentLogs, Switch};
pub use taurus_tools::{Checkpoint, Restored, Rewind, TurnChange};
pub use trust::PendingConfig;
pub use usage::{SchemaCost, ToolUsage, UsageReport};
// Re-exported so a frontend can edit search config without depending on the
// crate that runs the searches — the same reason `Checkpoint` comes through
// here rather than from `taurus-tools`. `IndexProgress` is here for the same
// reason once removed: the desktop app implements it to draw a progress bar,
// and has no other business with the indexer.
pub use taurus_index::IndexProgress;
pub use taurus_web::{BackendEntry, BackendKind, SearchFile};
