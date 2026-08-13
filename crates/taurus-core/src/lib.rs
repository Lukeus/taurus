//! Session state and the agent loop.
//!
//! Nothing here depends on Tauri. The whole harness is drivable headlessly,
//! which is what makes the loop testable against a scripted provider.

pub mod agent;
pub mod event;
pub mod session;
pub mod subagent;
pub mod testing;

pub use agent::{Agent, AgentConfig, AgentError, TurnOutcome};
pub use event::UiEvent;
pub use session::{estimate_block, estimate_message, estimate_tokens, Session, Trimmed};
pub use subagent::{AgentModel, ModelOverrides, SpawnSubagent, SPAWN_TOOL};
