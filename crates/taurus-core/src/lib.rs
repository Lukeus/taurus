//! Session state and the agent loop.
//!
//! Nothing here depends on Tauri. The whole harness is drivable headlessly,
//! which is what makes the loop testable against a scripted provider.

pub mod agent;
pub mod event;
pub mod finish;
pub mod propose;
pub mod relay;
pub mod session;
pub mod subagent;
pub mod telemetry;
pub mod testing;

pub use agent::{Agent, AgentConfig, AgentError, TurnOutcome, TurnRecorder};
pub use event::UiEvent;
pub use finish::FINISH_TOOL;
pub use propose::{ProposeAgent, PROPOSE_AGENT_TOOL};
pub use relay::ContextStrategy;
pub use session::{
    estimate_block, estimate_message, estimate_tokens, owed_results, Cause, Interrupted, Session,
    Trimmed, CEILING_CONTINUE_PROMPT, CONTINUE_PROMPT, MAX_ATTEMPTS,
};
pub use subagent::{AgentModel, ModelOverrides, SpawnSubagent, SubagentRecorder, SPAWN_TOOL};
pub use telemetry::Capture;
