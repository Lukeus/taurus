//! Everything both frontends need, and nothing either one owns alone.
//!
//! `src-tauri` and `taurus-cli` differ in how they talk to a person. They do
//! not differ in what a Taurus session *is* — same config files, same system
//! prompt, same tool registry, same skill library. That shared part lives here
//! so the two cannot drift apart.

pub mod config;
pub mod host;
pub mod prompt;

pub use config::{ProviderConfig, ProviderKind, Settings};
pub use host::{Host, PermissionPromptFactory, MAX_CONCURRENT_SUBAGENTS};
