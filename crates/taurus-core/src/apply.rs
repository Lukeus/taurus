//! `apply_delegate`: bringing a delegate's kept work into the workspace.
//!
//! A delegate that worked in a copy leaves its changes there, kept as a
//! commit. This is how the agent that sent it takes them. It's a write like
//! any other, so it's asked about like one, and what it writes goes into this
//! turn's undo record, so a rewind of the turn takes it back out.
//!
//! The rules are the provider's: see [`taurus_tools::isolation`]. One file
//! that can't be applied refuses the lot, and a file that decides what runs is
//! never applied for anyone.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, Isolation, Tool, ToolContext, ToolError, ToolResult};

pub const APPLY_TOOL: &str = "apply_delegate";

#[derive(Deserialize, JsonSchema)]
pub struct ApplyInput {
    /// The run named in the delegate's report: "Kept as run <run>".
    pub run: String,
    /// Say what applying would do to each file, and write nothing.
    #[serde(default)]
    pub dry_run: bool,
}

pub struct ApplyDelegate {
    isolation: Arc<dyn Isolation>,
}

impl ApplyDelegate {
    pub fn new(isolation: Arc<dyn Isolation>) -> Self {
        Self { isolation }
    }
}

#[async_trait]
impl Tool for ApplyDelegate {
    fn name(&self) -> &str {
        APPLY_TOOL
    }

    fn description(&self) -> &str {
        "Apply the changes a delegate kept in its own copy of the workspace (its report says \
         \"Kept as run <run>\"). Each file is checked against the workspace as it is now: one \
         that changed on both sides is merged if the changes don't overlap, and if any file can't \
         be applied, nothing is. Use dry_run to see what it would do first."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ApplyInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let run = input.get("run").and_then(|v| v.as_str()).unwrap_or("?");
        if input.get("dry_run").and_then(|v| v.as_bool()) == Some(true) {
            format!("Check what applying delegate run {run} would do")
        } else {
            format!("Apply the changes delegate run {run} kept to the workspace")
        }
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: ApplyInput = parse_input(input)?;
        let plan = self
            .isolation
            .apply(
                input.run.trim(),
                ctx.session_id.as_deref(),
                ctx.checkpoints.as_deref(),
                input.dry_run,
            )
            .await
            .map_err(ToolError::Failed)?;
        if !input.dry_run && !plan.applied {
            return Err(ToolError::Failed(plan.describe()));
        }
        Ok(plan.describe().into())
    }
}
