//! What the UI sees while a turn runs.
//!
//! Deliberately narrower than [`taurus_provider::StreamEvent`]: the UI needs
//! rendered, already-correlated facts (this tool call finished, here is its
//! output), not the raw token protocol.

use serde::{Deserialize, Serialize};
use taurus_provider::{StopReason, TokenUsage};
use taurus_tools::view::TranscriptView;
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
        /// Drawn in place of this call's row, for the handful of tools whose
        /// output is something to look at rather than something they did. See
        /// [`taurus_tools::view`].
        ///
        /// Carried on the announcement rather than the result because that is
        /// when the payload is known — it is the call's own input — and because
        /// a question card that only appeared once its call finished would be
        /// waiting on the answer it is asking for.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        view: Option<TranscriptView>,
    },
    /// Something a still-running tool has done. Only tools slow enough for
    /// silence to be ambiguous emit these — in practice, delegation.
    ToolProgress {
        /// The call this belongs under, matching its `ToolCallStarted`.
        id: String,
        label: String,
    },
    /// A still-running tool call has a conversation of its own, and here is
    /// where it is kept.
    ///
    /// Only delegation emits this. Carried separately from the call's result
    /// because it is known when the child *starts*: a delegation that is still
    /// going, or that was canceled half way, has a transcript worth opening,
    /// and one that only arrived with the result could not offer either.
    ToolTranscript {
        /// The call this belongs under, matching its `ToolCallStarted`.
        id: String,
        /// The child's own session id, under this conversation.
        session: String,
        /// Which kind of sub-agent it was.
        agent: String,
    },
    ToolCallFinished {
        id: String,
        ok: bool,
        /// Tool output, truncated for display.
        output: String,
        /// Pictures the tool handed back, for the card to draw.
        ///
        /// Separate from `output` rather than encoded into it because they are
        /// different things: `output` is shortened to keep the IPC channel
        /// small, and an image cannot be shortened — half a PNG is not half a
        /// picture, it is a broken file. What bounds these is the size cap
        /// applied where the result is produced.
        ///
        /// Empty for every tool that returns prose, which is nearly all of
        /// them, and always empty for a failed call: an error is something to
        /// read.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ResultImage>,
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
    /// Files this turn has changed, workspace-relative, as it changes them.
    ///
    /// Sent whenever a round captured a pre-image for a path it had not seen
    /// before, which is the moment the change becomes undoable. The whole set
    /// each time rather than the delta: a listener that missed one event would
    /// otherwise be short a file for the rest of the turn, and the set is
    /// bounded by what one turn touched.
    ///
    /// This exists because the alternative was asking. The count in the header
    /// was read from the checkpoint log after the turn finished, so a turn that
    /// spent two minutes rewriting the project said "No file changes" for all
    /// of it, and the truth arrived with a round trip once there was nothing
    /// left to watch.
    FilesChanged {
        paths: Vec<String>,
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

/// One picture a tool handed back, on its way to the card that draws it.
///
/// Its own type rather than [`taurus_provider::ToolResultBlock`] because the UI
/// has no use for the other two kinds — text is already in `output`, and JSON
/// is text as far as a transcript is concerned.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResultImage {
    pub mime_type: String,
    /// Base64, no data-URI prefix, exactly as it will be sent to the model.
    pub data: String,
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
