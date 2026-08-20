//! Programs the user has asked to run at fixed points in a turn.
//!
//! A hook is configuration, not a plugin: a command line in `hooks.json`, run
//! with no shell, told what is about to happen on stdin, and answering with an
//! exit code. There is no API to build against and nothing to compile.
//!
//! **A hook can refuse and cannot permit.** It runs after the permission engine
//! has already allowed a call, so the set of things a machine will do only ever
//! shrinks as hooks are added to it. That is what keeps this from being a second
//! permission system sitting beside the first, disagreeing with it — and it is
//! why a project's `hooks.json` is safe to honour once the project is trusted at
//! all. See [`config::HookEvent::PreToolUse`] for the argument in full, and
//! [`runner`] for what a hook is told and what its exit code means.

pub mod config;
pub mod runner;

pub use config::{config_file, load, merge, Hook, HookConfig, HookEntry, HookEvent, Match};
pub use runner::{HookPayload, HookRunner, HookSummary, Outcome, DENY};
