//! `finish`: how a delegate hands its result back.
//!
//! Registered into a delegate's own registry, never the shared one. It costs
//! a delegate's requests its schema, and every other request nothing. See
//! [`taurus_tools::delegate`] for why a report has a shape.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Disposition, Effect, Owner, Tool, ToolContext, ToolError, ToolResult};

pub const FINISH_TOOL: &str = "finish";

/// The shortest `needs` taken as a real action. Shorter is "wait" or "input".
const MIN_NEEDS_CHARS: usize = 10;

/// What a delegate may say about itself. `cancelled` and `unreported` are
/// the harness's to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Stated {
    Done,
    Blocked,
    Failed,
}

#[derive(Deserialize, JsonSchema)]
struct FinishInput {
    /// done: the task is complete. blocked: you can't go on until someone
    /// does something. failed: it can't be done, and you know why.
    disposition: Stated,
    /// What you found or did. This is all the agent that called you reads.
    summary: String,
    /// blocked only: who has to act, "parent" (the agent that called you) or
    /// "user".
    #[serde(default)]
    waiting_on: Option<Owner>,
    /// blocked only: the one concrete thing they have to do.
    #[serde(default)]
    needs: Option<String>,
}

/// A report as the delegate made it. Files are added by the delegation,
/// from what the harness saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reported {
    pub disposition: Disposition,
    pub summary: String,
    pub owner: Option<Owner>,
    pub needs: Option<String>,
}

/// Where the last accepted report is kept until the delegation reads it.
pub type Slot = Arc<Mutex<Option<Reported>>>;

pub struct Finish {
    slot: Slot,
    schema: serde_json::Value,
}

impl Finish {
    pub fn new(slot: Slot) -> Self {
        Self {
            slot,
            schema: schema_for::<FinishInput>(),
        }
    }
}

#[async_trait]
impl Tool for Finish {
    fn name(&self) -> &str {
        FINISH_TOOL
    }

    fn description(&self) -> &str {
        "Hand your result back to the agent that gave you this task, and stop. Call it once, \
         when you're done, blocked, or sure it can't be done."
    }

    fn input_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn ends_turn(&self) -> bool {
        true
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let disposition = input
            .get("disposition")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        format!("Report back: {disposition}")
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: FinishInput = parse_input(input)?;
        let summary = input.summary.trim().to_string();
        if summary.is_empty() {
            return Err(ToolError::InvalidInput(
                "`summary` is empty. It's the only part of your work the agent that called you \
                 will read, so say what you found or did."
                    .into(),
            ));
        }

        let (disposition, owner, needs) = match input.disposition {
            Stated::Done => (Disposition::Done, None, None),
            Stated::Failed => (Disposition::Failed, None, None),
            Stated::Blocked => {
                let needs = input
                    .needs
                    .map(|n| n.trim().to_string())
                    .filter(|n| n.chars().count() >= MIN_NEEDS_CHARS);
                let (Some(owner), Some(needs)) = (input.waiting_on, needs) else {
                    // Invalid input, not a refusal: the call is right to be
                    // made, and the same call with the two fields filled in
                    // succeeds.
                    return Err(ToolError::InvalidInput(
                        "A blocked report has to say who must act (`waiting_on`: \"parent\" or \
                         \"user\") and what they must do (`needs`, one concrete step). Without \
                         both it reaches nobody. If nobody can unblock it, report `failed` \
                         and say why."
                            .into(),
                    ));
                };
                (Disposition::Blocked, Some(owner), Some(needs))
            }
        };

        // The last one wins. A delegate asked to check its work after
        // reporting done reports again, and the second report is the one
        // that knows what the check said.
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(Reported {
            disposition,
            summary,
            owner,
            needs,
        });
        Ok("Reported.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taurus_tools::{AllowAll, PermissionEngine};
    use tokio_util::sync::CancellationToken;

    async fn call(input: serde_json::Value) -> (Result<String, ToolError>, Option<Reported>) {
        let dir = tempfile::TempDir::new().unwrap();
        let permissions = Arc::new(PermissionEngine::new(
            dir.path(),
            dir.path().join(".taurus"),
            Box::new(AllowAll),
        ));
        let ctx = ToolContext::new(dir.path(), permissions, CancellationToken::new());
        let slot = Slot::default();
        let out = Finish::new(slot.clone()).execute(input, &ctx).await;
        let stated = slot.lock().unwrap().clone();
        (out.map(|o| o.to_text().into_owned()), stated)
    }

    #[tokio::test]
    async fn a_done_report_is_kept() {
        let (out, stated) = call(serde_json::json!({
            "disposition": "done",
            "summary": "  The parser is in src/parse.rs.  "
        }))
        .await;
        assert_eq!(out.unwrap(), "Reported.");
        let stated = stated.unwrap();
        assert_eq!(stated.disposition, Disposition::Done);
        assert_eq!(stated.summary, "The parser is in src/parse.rs.");
        assert_eq!(stated.owner, None);
    }

    #[tokio::test]
    async fn a_blocked_report_without_an_owner_and_an_action_is_refused() {
        for input in [
            serde_json::json!({ "disposition": "blocked", "summary": "Stuck." }),
            serde_json::json!({ "disposition": "blocked", "summary": "Stuck.", "waiting_on": "user" }),
            serde_json::json!({ "disposition": "blocked", "summary": "Stuck.", "needs": "decide which file is canonical" }),
            serde_json::json!({ "disposition": "blocked", "summary": "Stuck.", "waiting_on": "user", "needs": "input" }),
        ] {
            let (out, stated) = call(input.clone()).await;
            let err = out.unwrap_err();
            assert!(matches!(err, ToolError::InvalidInput(_)), "{input}");
            assert!(err.to_string().contains("reaches nobody"), "{err}");
            assert!(stated.is_none(), "a refused report must not be kept");
        }
    }

    #[tokio::test]
    async fn a_blocked_report_names_who_and_what() {
        let (out, stated) = call(serde_json::json!({
            "disposition": "blocked",
            "summary": "Two config files disagree.",
            "waiting_on": "user",
            "needs": "say which of config.toml and config.local.toml is canonical"
        }))
        .await;
        out.unwrap();
        let stated = stated.unwrap();
        assert_eq!(stated.disposition, Disposition::Blocked);
        assert_eq!(stated.owner, Some(Owner::User));
        assert!(stated.needs.unwrap().starts_with("say which"));
    }

    #[tokio::test]
    async fn an_empty_summary_is_refused() {
        let (out, _) = call(serde_json::json!({ "disposition": "done", "summary": " " })).await;
        assert!(out.unwrap_err().to_string().contains("`summary` is empty"));
    }

    #[test]
    fn the_schema_offers_only_what_a_delegate_may_say() {
        let schema = Finish::new(Slot::default()).input_schema();
        let values = schema["properties"]["disposition"]["enum"].clone();
        assert_eq!(values, serde_json::json!(["done", "blocked", "failed"]));
    }
}
