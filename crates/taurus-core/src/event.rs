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
    /// A request failed in a way a retry could plausibly fix, and is being
    /// retried after a backoff.
    ///
    /// Reported for the same reason [`UiEvent::ToolProgress`] is: a wait of
    /// several seconds with nothing on screen is indistinguishable from a hang,
    /// and a rate limit the harness is already handling should not look like
    /// one.
    Retrying {
        /// 1-based, so it reads as "attempt 2 of 4" rather than counting
        /// retries the user never saw.
        attempt: u32,
        of: u32,
        /// Why the previous attempt failed, in the provider's own words.
        reason: String,
    },
    /// Older tool output was shortened to stay inside the context window.
    /// Cheaper than [`UiEvent::Compacted`] and tried first: nothing was sent to
    /// the model to produce it, and no message was removed.
    ContextTrimmed {
        /// Tool results collapsed or shortened.
        results: usize,
        /// Estimated tokens recovered.
        tokens_saved: u32,
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
