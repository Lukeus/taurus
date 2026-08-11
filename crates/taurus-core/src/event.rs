//! What the UI sees while a turn runs.
//!
//! Deliberately narrower than [`taurus_provider::StreamEvent`]: the UI needs
//! rendered, already-correlated facts (this tool call finished, here is its
//! output), not the raw token protocol.

use serde::{Deserialize, Serialize};
use taurus_provider::{StopReason, TokenUsage};
use ts_rs::TS;

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum UiEvent {
    /// A new model request has started. `iteration` counts tool round-trips
    /// within one user turn, so the UI can show "still working" honestly.
    IterationStarted {
        iteration: u32,
    },
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolCallStarted {
        id: String,
        name: String,
        /// Human-readable summary of the specific call.
        preview: String,
    },
    /// Something a still-running tool has done. Only tools slow enough for
    /// silence to be ambiguous emit these — in practice, delegation.
    ToolProgress {
        /// The call this belongs under, matching its `ToolCallStarted`.
        id: String,
        label: String,
    },
    ToolCallFinished {
        id: String,
        ok: bool,
        /// Tool output, truncated for display.
        output: String,
    },
    /// History was summarized to stay inside the context window.
    Compacted {
        messages_removed: usize,
    },
    TurnFinished {
        stop_reason: StopReason,
        usage: TokenUsage,
    },
    /// The turn could not continue. Distinct from a failed tool call, which
    /// the model recovers from on its own.
    Error {
        message: String,
    },
}

/// Longest tool output echoed to the UI. The model still receives the full
/// text; this only bounds what is pushed through the IPC channel per call.
const MAX_UI_OUTPUT: usize = 4000;

pub fn truncate_for_ui(text: &str) -> String {
    if text.chars().count() <= MAX_UI_OUTPUT {
        return text.to_string();
    }
    let head: String = text.chars().take(MAX_UI_OUTPUT).collect();
    format!("{head}\n… (truncated for display)")
}
