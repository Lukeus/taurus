//! The `propose_agent` tool.
//!
//! Sits here rather than in `taurus-agents` because it needs both a tool
//! registry and an agent catalog, and that crate is deliberately a leaf that
//! knows about neither. The rules it enforces live there; this is the surface
//! the model reaches them through.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::info;

use taurus_agents::catalog::SharedAgentCatalog;
use taurus_agents::proposal::{validate_proposal, AgentProposal, AgentProposalSink};
use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, Tool, ToolContext, ToolError, ToolRegistry, ToolResult};

/// Named as a constant because the host registers and unregisters this tool as
/// the synthesis setting changes, and a name typed twice is a name that can be
/// typed differently.
pub const PROPOSE_AGENT_TOOL: &str = "propose_agent";

#[derive(Deserialize, JsonSchema)]
pub struct ProposeAgentInput {
    /// Kebab-case identifier, e.g. `migration-checker`.
    pub name: String,
    /// One sentence on what this agent does and when to hand it work. This is
    /// the only text you will see later when choosing whether to delegate to
    /// it, so describe the job, not the implementation. Under 200 characters.
    pub description: String,
    /// The agent's system prompt. It shares none of your context and cannot ask
    /// questions, so write what it should do, what it should not, and what to
    /// report back. Write it for someone who has never seen this project.
    pub prompt: String,
    /// Tools it may call, e.g. `["read_file", "grep"]`. Omit to let it inherit
    /// yours. Naming tools you do not have is refused, so scope it to a subset.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Round trips it may take before being stopped. 1 to 50; 20 is a sane
    /// default for reading work and 25 for work that changes files.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// Why this is worth keeping. Shown to the user, not stored.
    #[serde(default)]
    pub rationale: String,
}

fn default_max_iterations() -> u32 {
    20
}

pub struct ProposeAgent {
    catalog: SharedAgentCatalog,
    /// Read for the tool names a proposal may scope itself to. The live
    /// registry, so a skill or MCP tool approved earlier in this session counts
    /// as available.
    registry: Arc<RwLock<ToolRegistry>>,
    sink: Arc<dyn AgentProposalSink>,
}

impl ProposeAgent {
    pub fn new(
        catalog: SharedAgentCatalog,
        registry: Arc<RwLock<ToolRegistry>>,
        sink: Arc<dyn AgentProposalSink>,
    ) -> Self {
        Self {
            catalog,
            registry,
            sink,
        }
    }
}

#[async_trait]
impl Tool for ProposeAgent {
    fn name(&self) -> &str {
        PROPOSE_AGENT_TOOL
    }

    fn description(&self) -> &str {
        "Write down a delegate worth keeping: a job you had to explain from scratch and will have \
         to explain again. Propose an agent when a task has a shape that recurs and a scope worth \
         narrowing — reviewing a diff, auditing one kind of file, working through a migration \
         site by site. Do not propose one for something a skill would cover better: a skill is a \
         procedure you follow, an agent is a worker you hand a task to and get an answer back \
         from. Do not propose one that duplicates an agent already on the roster, and do not \
         propose one for this task alone. Every agent costs a line of every request, so a roster \
         of near-duplicates is worse than a short one. The user reviews every proposal before it \
         is saved, so you can keep working immediately after calling this."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ProposeAgentInput>()
    }

    /// Read, not Write: this creates no file. The review card the user sees is
    /// itself the permission prompt, so gating it here would ask twice.
    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Propose agent '{}'",
            input.get("name").and_then(|n| n.as_str()).unwrap_or("?")
        )
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: ProposeAgentInput = parse_input(input)?;

        let mut proposal = AgentProposal::new(input.name, input.description, input.prompt);
        proposal.tools = input.tools;
        proposal.max_iterations = input.max_iterations;
        proposal.rationale = input.rationale;

        let available: Vec<String> = self
            .registry
            .read()
            .await
            .names()
            .map(str::to_string)
            .collect();

        {
            let catalog = self.catalog.read().await;
            proposal.replaces_existing = catalog.contains(&proposal.name);
            // Rejections come back as tool errors so the model can fix the
            // proposal rather than assume it succeeded.
            validate_proposal(&proposal, &catalog, &available)
                .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        }

        let name = proposal.name.clone();
        info!(agent = %name, "agent proposed");
        self.sink.submit(proposal).await;

        Ok(format!(
            "Proposed agent '{name}'. It is queued for the user to review, and becomes available \
             to delegate to once they approve it — not in this turn, since the roster is fixed \
             when a turn starts. Carry on with the current task."
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taurus_agents::catalog::AgentCatalog;
    use taurus_agents::proposal::CollectingSink;
    use taurus_tools::{AllowAll, PermissionEngine};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    const PROMPT: &str = "You review a diff for correctness bugs only. Report each finding with \
                          the file and line it is on, and say so plainly when you find nothing.";

    /// The tool over an empty roster, with `read_file` and `grep` available.
    fn tool() -> (ProposeAgent, Arc<CollectingSink>) {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(taurus_tools::builtin::fs::ReadFile));
        registry.register(Arc::new(taurus_tools::builtin::search::Grep));

        let sink = Arc::new(CollectingSink::default());
        let tool = ProposeAgent::new(
            Arc::new(RwLock::new(AgentCatalog::default())),
            Arc::new(RwLock::new(registry)),
            sink.clone(),
        );
        (tool, sink)
    }

    fn ctx() -> (ToolContext, TempDir) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let engine = Arc::new(PermissionEngine::new(
            &root,
            root.join(".taurus"),
            Box::new(AllowAll),
        ));
        (
            ToolContext::new(root, engine, CancellationToken::new()),
            dir,
        )
    }

    fn input(tools: Option<Vec<&str>>) -> serde_json::Value {
        serde_json::json!({
            "name": "diff-reviewer",
            "description": "Reviews a diff for correctness bugs and reports what it found",
            "prompt": PROMPT,
            "tools": tools.map(|t| t.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        })
    }

    #[tokio::test]
    async fn a_sound_proposal_is_queued_rather_than_saved() {
        let (tool, sink) = tool();
        let (ctx, _dir) = ctx();

        let result = tool.execute(input(Some(vec!["read_file"])), &ctx).await;

        assert!(result.is_ok(), "{result:?}");
        let queued = sink.proposals.lock().await;
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].name, "diff-reviewer");
        assert_eq!(queued[0].tools, Some(vec!["read_file".to_string()]));
    }

    #[tokio::test]
    async fn the_model_is_told_the_agent_is_not_usable_yet() {
        // A turn's roster is frozen when it starts, so a model that proposed an
        // agent and then tried to delegate to it would spend a round trip
        // finding that out. Say it in the result instead.
        let (tool, _sink) = tool();
        let (ctx, _dir) = ctx();

        let message = tool.execute(input(None), &ctx).await.unwrap();

        assert!(message.to_text().contains("review"), "{message}");
        assert!(message.to_text().contains("not in this turn"), "{message}");
    }

    #[tokio::test]
    async fn a_tool_the_session_lacks_comes_back_as_a_fixable_error() {
        // The model should be able to correct this and call again, so it has to
        // arrive as a tool error naming the tool — not as a silent rejection.
        let (tool, sink) = tool();
        let (ctx, _dir) = ctx();

        let error = tool
            .execute(input(Some(vec!["read_file", "send_email"])), &ctx)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("send_email"), "{error}");
        assert!(
            sink.proposals.lock().await.is_empty(),
            "a rejected proposal must not reach the user"
        );
    }

    #[tokio::test]
    async fn proposing_writes_nothing_and_asks_no_permission() {
        // `Effect::Read` is the claim; this is what makes it true. If this tool
        // ever writes, the review card stops being the only gate.
        let (tool, _sink) = tool();
        let (ctx, dir) = ctx();

        tool.execute(input(None), &ctx).await.unwrap();

        assert_eq!(tool.effect(), Effect::Read);
        let left_behind: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert!(left_behind.is_empty(), "{left_behind:?}");
    }
}
