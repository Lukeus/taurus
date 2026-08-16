//! Everything both frontends need, and nothing either one owns alone.
//!
//! `src-tauri` and `taurus-cli` differ in how they talk to a person. They do
//! not differ in what a Taurus session *is* — same config files, same system
//! prompt, same tool registry, same skill library. That shared part lives here
//! so the two cannot drift apart.

pub mod config;
pub mod git;
pub mod host;
pub mod instructions;
pub mod problem;
pub mod prompt;
pub mod secrets;
pub mod sessions;
#[cfg(test)]
mod testing;

pub use config::{ProviderConfig, ProviderKind, Scope, Settings, Theme};
pub use git::{Commit, Repo, RepoStatus};
pub use host::{Host, PermissionPromptFactory, TurnRef, MAX_CONCURRENT_SUBAGENTS};
pub use instructions::{Instructions, InstructionsOrigin, InstructionsSource, InstructionsTier};
pub use problem::{Problem, ProblemSource};
pub use secrets::KeyStatus;
pub use sessions::{SessionLog, SessionMeta};
pub use taurus_tools::{Checkpoint, Restored, TurnChange};
// Re-exported so a frontend can edit search config without depending on the
// crate that runs the searches — the same reason `Checkpoint` comes through
// here rather than from `taurus-tools`.
pub use taurus_web::{BackendEntry, BackendKind, SearchFile};
