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
pub use session::Session;
pub use subagent::{SpawnSubagent, SPAWN_TOOL};
