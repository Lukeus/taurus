//! Delegation to scoped child agents.
//!
//! A sub-agent gets its own conversation, its own context window, and a
//! narrower tool set. The parent sees only the child's conclusion, which is the
//! point: a search that reads thirty files should cost the parent one paragraph
//! rather than thirty file dumps.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::{mpsc, RwLock, Semaphore};
use tracing::info;

use taurus_provider::{Message, Provider};
use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, Tool, ToolContext, ToolError, ToolRegistry, ToolResult};

use crate::agent::{Agent, AgentConfig};
use crate::event::UiEvent;
use crate::session::Session;

pub const SPAWN_TOOL: &str = "spawn_subagent";

/// A kind of sub-agent the parent can ask for.
#[derive(Clone, Debug)]
pub struct AgentDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub system_prompt: &'static str,
    /// Tools this kind may use. Empty means everything the parent has.
    pub allowed_tools: &'static [&'static str],
    pub max_iterations: u32,
}

/// The kinds available out of the box.
pub const DEFINITIONS: &[AgentDefinition] = &[
    AgentDefinition {
        name: "explorer",
        description:
            "Searches and reads the codebase to answer a question. Cannot modify anything. Use \
             this when finding the answer would mean reading many files.",
        system_prompt:
            "You are a research sub-agent. Search and read to answer the question you were given, \
             then reply with the answer and the paths that support it. You cannot modify files. \
             Be specific and brief; the agent that called you sees only your reply, not your \
             tool calls.",
        allowed_tools: &["read_file", "list_dir", "glob", "grep", "load_skill"],
        max_iterations: 20,
    },
    AgentDefinition {
        name: "worker",
        description:
            "Carries out a well-specified, self-contained change. Give it complete instructions; \
             it cannot ask you questions.",
        system_prompt:
            "You are a sub-agent carrying out one specific task. You cannot ask questions, so work \
             from the instructions you were given. When done, reply with what you changed. Be \
             brief; the agent that called you sees only your reply.",
        allowed_tools: &[],
        max_iterations: 25,
    },
];

fn definition(name: &str) -> Option<&'static AgentDefinition> {
    DEFINITIONS.iter().find(|d| d.name == name)
}

#[derive(Deserialize, JsonSchema)]
pub struct SpawnInput {
    /// Which kind of sub-agent to run: `explorer` or `worker`.
    pub agent_type: String,
    /// The complete task. The sub-agent shares none of your context and cannot
    /// ask follow-up questions, so include every detail it needs.
    pub prompt: String,
}

pub struct SpawnSubagent {
    provider: Arc<dyn Provider>,
    /// The live registry, shared with the parent so skills and MCP tools
    /// approved mid-session are visible to children too.
    registry: Arc<RwLock<ToolRegistry>>,
    model: String,
    /// Caps how many children run at once. A confused model will otherwise
    /// spawn until the machine falls over.
    permits: Arc<Semaphore>,
}

impl SpawnSubagent {
    pub fn new(
        provider: Arc<dyn Provider>,
        registry: Arc<RwLock<ToolRegistry>>,
        model: impl Into<String>,
        max_concurrent: usize,
    ) -> Self {
        Self {
            provider,
            registry,
            model: model.into(),
            permits: Arc::new(Semaphore::new(max_concurrent.max(1))),
        }
    }
}

#[async_trait]
impl Tool for SpawnSubagent {
    fn name(&self) -> &str {
        SPAWN_TOOL
    }

    fn description(&self) -> &str {
        "Hand a self-contained task to a sub-agent with its own context, and get back its result. \
         Use this when the work would fill your context with detail you do not need to keep — a \
         broad search, or an independent change. The sub-agent cannot ask you questions and \
         cannot delegate further, so give it complete instructions. Types: 'explorer' (read-only \
         research) and 'worker' (makes changes)."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<SpawnInput>()
    }

    /// The child's own tool calls are gated individually against the same
    /// permission engine, so spawning is not itself a privileged act.
    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let kind = input
            .get("agent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let task = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        let short: String = task.chars().take(80).collect();
        format!("Delegate to {kind}: {short}")
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: SpawnInput = parse_input(input)?;

        let Some(definition) = definition(&input.agent_type) else {
            let known: Vec<&str> = DEFINITIONS.iter().map(|d| d.name).collect();
            return Err(ToolError::InvalidInput(format!(
                "No sub-agent type '{}'. Available: {}.",
                input.agent_type,
                known.join(", ")
            )));
        };

        if input.prompt.trim().len() < 15 {
            return Err(ToolError::InvalidInput(
                "The task is too vague. The sub-agent shares none of your context, so spell out \
                 what it should do and what to report back."
                    .into(),
            ));
        }

        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| ToolError::Failed("sub-agent pool is shut down".into()))?;

        // Depth cap, enforced structurally: the child's registry has no spawn
        // tool, so it cannot delegate no matter what it decides to do.
        let child_registry = self.registry.read().await.without(SPAWN_TOOL);

        let allowed: Vec<String> = definition
            .allowed_tools
            .iter()
            .map(|s| s.to_string())
            .filter(|name| child_registry.get(name).is_some())
            .collect();

        let agent = Agent::new(
            self.provider.clone(),
            child_registry,
            ctx.clone(),
            AgentConfig {
                system_prompt: format!(
                    "{}\n\nYou are working in `{}`.",
                    definition.system_prompt,
                    ctx.workspace.display()
                ),
                max_iterations: definition.max_iterations,
                allowed_tools: allowed,
                ..Default::default()
            },
        );

        info!(kind = definition.name, "spawning sub-agent");

        // The child's events are collected rather than forwarded: the parent's
        // transcript should show one delegation, not the child's whole run.
        let (tx, mut rx) = mpsc::channel::<UiEvent>(256);
        let collector = tokio::spawn(async move {
            let mut tools: Vec<String> = Vec::new();
            while let Some(event) = rx.recv().await {
                if let UiEvent::ToolCallStarted { name, .. } = event {
                    tools.push(name);
                }
            }
            tools
        });

        let mut session = Session::new(&self.model);
        let outcome = agent
            .run_turn(&mut session, Message::user(&input.prompt), tx)
            .await;
        let tools_used = collector.await.unwrap_or_default();

        let answer = session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == taurus_provider::Role::Assistant && !m.text().trim().is_empty())
            .map(|m| m.text())
            .unwrap_or_default();

        let mut report = match outcome {
            Ok(_) if !answer.trim().is_empty() => answer,
            Ok(_) => "The sub-agent finished without reporting anything.".into(),
            Err(e) => format!(
                "The sub-agent stopped early ({e}). Partial result:\n\n{}",
                if answer.trim().is_empty() {
                    "(none)"
                } else {
                    &answer
                }
            ),
        };

        if !tools_used.is_empty() {
            report.push_str(&format!("\n\n[sub-agent used: {}]", summarize(&tools_used)));
        }
        Ok(report)
    }
}

/// `read_file ×3, grep ×1` — enough for the user to see what the child did
/// without reproducing its transcript.
fn summarize(tools: &[String]) -> String {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for tool in tools {
        *counts.entry(tool.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(name, n)| {
            if n == 1 {
                name.to_string()
            } else {
                format!("{name} ×{n}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeProvider, ScriptedTurn};
    use taurus_tools::{AllowAll, PermissionEngine};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn fixture(turns: Vec<ScriptedTurn>) -> (SpawnSubagent, ToolContext, TempDir) {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let permissions = Arc::new(PermissionEngine::new(
            &workspace,
            workspace.join(".taurus"),
            Box::new(AllowAll),
        ));
        let ctx = ToolContext::new(workspace, permissions, CancellationToken::new());

        let provider = FakeProvider::new(turns);
        let mut registry = ToolRegistry::with_builtins();
        // The parent's registry contains the spawn tool, as it would in the app.
        registry.register(Arc::new(SpawnSubagent::new(
            provider.clone(),
            Arc::new(RwLock::new(ToolRegistry::with_builtins())),
            "fake",
            2,
        )));
        let registry = Arc::new(RwLock::new(registry));

        let tool = SpawnSubagent::new(provider, registry, "fake", 2);
        (tool, ctx, dir)
    }

    #[tokio::test]
    async fn returns_the_childs_final_answer() {
        let (tool, ctx, _dir) = fixture(vec![ScriptedTurn::text("The answer is 42.")]);
        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "explorer",
                    "prompt": "Find out what the answer is and report it."
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("The answer is 42."));
    }

    #[tokio::test]
    async fn reports_which_tools_the_child_used() {
        let (tool, ctx, _dir) = fixture(vec![
            ScriptedTurn::tool_call("t1", "list_dir", serde_json::json!({})),
            ScriptedTurn::text("Found nothing of note."),
        ]);
        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "explorer",
                    "prompt": "Look around the workspace and describe it."
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("[sub-agent used: list_dir]"));
    }

    #[tokio::test]
    async fn a_child_cannot_delegate_further() {
        // Depth is capped by construction, not by a counter the model could
        // talk its way past.
        let (tool, ctx, _dir) = fixture(vec![
            ScriptedTurn::tool_call(
                "t1",
                SPAWN_TOOL,
                serde_json::json!({"agent_type": "worker", "prompt": "recurse forever please"}),
            ),
            ScriptedTurn::text("I could not delegate."),
        ]);
        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "worker",
                    "prompt": "Try to spawn another sub-agent and tell me what happens."
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("I could not delegate."));
    }

    #[tokio::test]
    async fn rejects_an_unknown_agent_type() {
        let (tool, ctx, _dir) = fixture(vec![]);
        let err = tool
            .execute(
                serde_json::json!({
                    "agent_type": "wizard",
                    "prompt": "Do something clever with the codebase."
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("explorer"));
        assert!(err.to_string().contains("worker"));
    }

    #[tokio::test]
    async fn rejects_a_task_too_vague_to_act_on() {
        let (tool, ctx, _dir) = fixture(vec![]);
        let err = tool
            .execute(
                serde_json::json!({"agent_type": "worker", "prompt": "fix it"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("too vague"));
    }

    #[test]
    fn tool_usage_summary_counts_repeats() {
        let tools = vec![
            "read_file".to_string(),
            "grep".to_string(),
            "read_file".to_string(),
        ];
        assert_eq!(summarize(&tools), "grep, read_file ×2");
    }
}
