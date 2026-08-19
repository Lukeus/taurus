//! Delegation to scoped child agents.
//!
//! A sub-agent gets its own conversation, its own context window, and a
//! narrower tool set. The parent sees only the child's conclusion, which is the
//! point: a search that reads thirty files should cost the parent one paragraph
//! rather than thirty file dumps.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::{mpsc, RwLock, Semaphore};
use tracing::info;

use taurus_agents::{builtin, AgentDefinition};
use taurus_provider::{Message, Provider};
use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, Tool, ToolContext, ToolError, ToolRegistry, ToolResult};

use crate::agent::{Agent, AgentConfig, TurnRecorder};
use crate::event::UiEvent;
use crate::session::Session;

pub const SPAWN_TOOL: &str = "spawn_subagent";

/// What one agent's `model:` and `provider:` resolved to.
///
/// Resolved by the host at reload rather than here, because building a provider
/// reads the OS credential store and that is not something to put on a per-turn
/// path.
#[derive(Clone)]
pub struct AgentModel {
    /// `None` when the file named a model but no provider: the model is then a
    /// different model *on the session's provider*, which is the only provider
    /// the host does not know at reload time.
    pub provider: Option<Arc<dyn Provider>>,
    pub model: String,
}

/// Per-agent model overrides, keyed by agent name.
pub type ModelOverrides = HashMap<String, AgentModel>;

#[derive(Deserialize, JsonSchema)]
pub struct SpawnInput {
    /// Which kind of sub-agent to run. Must be one of the types this tool's
    /// description lists.
    pub agent_type: String,
    /// The complete task. The sub-agent shares none of your context and cannot
    /// ask follow-up questions, so include every detail it needs.
    pub prompt: String,
}

/// Where a delegate's conversation is kept.
///
/// The parent's transcript records one delegation — a call, and the paragraph
/// it came back with. That is the whole point of delegating, and it is also
/// how a child's work used to vanish: thirty files read, a plan formed, a
/// dead end backed out of, and nothing left on disk to say so once the tool
/// returned. So the child gets a transcript of its own, beside its parent's
/// and out of the way of it.
///
/// Opened per delegation rather than written at the end, so a child that is
/// still running, or was canceled, or crashed the process, has left as much of
/// itself behind as it got through. Returning `None` declines to record this
/// one, which is what a host with nowhere to write says.
#[async_trait]
pub trait SubagentRecorder: Send + Sync {
    async fn open(&self, agent_type: &str, child: &Session) -> Option<Arc<dyn TurnRecorder>>;
}

pub struct SpawnSubagent {
    /// The session's provider and model, used by any agent that does not name
    /// its own.
    default_provider: Arc<dyn Provider>,
    default_model: String,
    /// Per-agent overrides. See [`ModelOverrides`].
    overrides: ModelOverrides,
    /// This turn's roster, frozen at construction. A turn that saw one set of
    /// agents when it started should not find a different set halfway through
    /// because a file was saved.
    agents: Arc<Vec<AgentDefinition>>,
    /// The live registry, shared with the parent so skills and MCP tools
    /// approved mid-session are visible to children too.
    registry: Arc<RwLock<ToolRegistry>>,
    /// Caps how many children run at once. A confused model will otherwise
    /// spawn until the machine falls over.
    permits: Arc<Semaphore>,
    /// Prebuilt from `agents`, so `description` can still return a `&str`.
    description: String,
    /// Prebuilt from `agents`: `agent_type` carries an `enum` of the live names.
    schema: serde_json::Value,
    /// Where children's transcripts go. `None` keeps them in memory and drops
    /// them with the turn, which is what every test and example wants.
    recorder: Option<Arc<dyn SubagentRecorder>>,
}

impl SpawnSubagent {
    /// Built with the compiled-in roster. Hosts that have scanned for custom
    /// agents replace it with [`SpawnSubagent::with_roster`].
    pub fn new(
        provider: Arc<dyn Provider>,
        registry: Arc<RwLock<ToolRegistry>>,
        model: impl Into<String>,
        max_concurrent: usize,
    ) -> Self {
        let agents = Arc::new(builtin::definitions());
        Self {
            default_provider: provider,
            default_model: model.into(),
            overrides: ModelOverrides::new(),
            description: describe(&agents),
            schema: schema_with_agent_types(&agents),
            agents,
            registry,
            permits: Arc::new(Semaphore::new(max_concurrent.max(1))),
            recorder: None,
        }
    }

    /// Keeps every child's conversation, wherever the host puts transcripts.
    pub fn with_recorder(mut self, recorder: Arc<dyn SubagentRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// Swaps in the host's roster and its resolved model overrides.
    pub fn with_roster(
        mut self,
        agents: Arc<Vec<AgentDefinition>>,
        overrides: ModelOverrides,
    ) -> Self {
        self.description = describe(&agents);
        self.schema = schema_with_agent_types(&agents);
        self.agents = agents;
        self.overrides = overrides;
        self
    }

    fn definition(&self, name: &str) -> Option<&AgentDefinition> {
        self.agents.iter().find(|a| a.name() == name)
    }
}

/// The tool description, with the roster in it.
///
/// The roster reaches the model here rather than through the system prompt so
/// that it sits next to the `agent_type` parameter it constrains — and, more
/// usefully, so it can be mirrored into that parameter's schema below.
fn describe(agents: &[AgentDefinition]) -> String {
    let mut out = String::from(
        "Hand a self-contained task to a sub-agent with its own context, and get back its result. \
         Use this when the work would fill your context with detail you do not need to keep — a \
         broad search, or an independent change. The sub-agent cannot ask you questions and \
         cannot delegate further, so give it complete instructions.\n\nTypes:\n",
    );
    for agent in agents {
        out.push_str(&agent.roster_line());
        out.push('\n');
    }
    out
}

/// Patches the statically derived schema so `agent_type` enumerates the live
/// names.
///
/// An enum is materially stronger than prose for the 7B-class local models this
/// harness targets, which otherwise invent plausible agent names. It is patched
/// rather than derived because `schema_for` runs against a type, and the roster
/// is not known until the host has scanned the disk.
fn schema_with_agent_types(agents: &[AgentDefinition]) -> serde_json::Value {
    let mut schema = schema_for::<SpawnInput>();
    if let Some(property) = schema
        .pointer_mut("/properties/agent_type")
        .and_then(|v| v.as_object_mut())
    {
        property.insert(
            "enum".into(),
            agents
                .iter()
                .map(|a| serde_json::Value::String(a.name().to_string()))
                .collect(),
        );
    }
    schema
}

#[async_trait]
impl Tool for SpawnSubagent {
    fn name(&self) -> &str {
        SPAWN_TOOL
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> serde_json::Value {
        self.schema.clone()
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

        let Some(definition) = self.definition(&input.agent_type) else {
            let known: Vec<&str> = self.agents.iter().map(AgentDefinition::name).collect();
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

        // `allowed_tools` empty means "everything the parent has", which is what
        // an agent with no `tools:` key wants. An agent that *did* name its
        // tools must never reach that state by attrition: a list that filters
        // down to nothing is a scope the user wrote to narrow this agent, and
        // honouring it as "everything" would hand it the shell instead. The
        // host refuses such an agent at load; this is the same check standing
        // between the two, because the live registry is not the one the host
        // checked against.
        let allowed: Vec<String> = match &definition.frontmatter.tools {
            None => Vec::new(),
            Some(wanted) => {
                let available: Vec<String> = wanted
                    .iter()
                    .filter(|name| child_registry.get(name).is_some())
                    .cloned()
                    .collect();
                if available.is_empty() {
                    return Err(ToolError::Failed(format!(
                        "The sub-agent '{}' is scoped to tools that are not available here ({}), \
                         so it would run unrestricted. It has been refused instead. Enable those \
                         tools, or widen its `tools:` list.",
                        definition.name(),
                        wanted.join(", ")
                    )));
                }
                available
            }
        };

        let (provider, model) = match self.overrides.get(definition.name()) {
            Some(over) => (
                over.provider
                    .clone()
                    .unwrap_or_else(|| self.default_provider.clone()),
                over.model.clone(),
            ),
            None => (self.default_provider.clone(), self.default_model.clone()),
        };

        // Before the agent, because opening a recorder needs the id of the
        // conversation it is recording.
        let mut session = Session::new(&model);
        let agent = Agent::new(
            provider,
            child_registry,
            ctx.clone(),
            AgentConfig {
                system_prompt: format!(
                    "{}\n\nYou are working in `{}`.",
                    definition.system_prompt,
                    ctx.workspace.display()
                ),
                max_iterations: definition.frontmatter.max_iterations,
                allowed_tools: allowed,
                ..Default::default()
            },
        );

        let agent = match &self.recorder {
            Some(recorder) => match recorder.open(definition.name(), &session).await {
                // Announced only once there is somewhere to read: a card that
                // offered to open a transcript nobody was writing would be a
                // worse answer than one that offers nothing.
                Some(sink) => {
                    ctx.report_transcript(session.id.clone(), definition.name())
                        .await;
                    agent.with_recorder(sink)
                }
                None => agent,
            },
            None => agent,
        };

        info!(kind = definition.name(), %model, "spawning sub-agent");

        // The child's own text and results stay inside the child — the parent's
        // transcript should show one delegation, not a second conversation. But
        // *what it is doing* is forwarded as it happens: a delegation can run
        // for a minute, and a card that says "running" for that long is
        // indistinguishable from one that has hung.
        let (tx, mut rx) = mpsc::channel::<UiEvent>(256);
        let progress = ctx.clone();
        let collector = tokio::spawn(async move {
            let mut tools: Vec<String> = Vec::new();
            while let Some(event) = rx.recv().await {
                if let UiEvent::ToolCallStarted { name, preview, .. } = event {
                    progress.report(preview).await;
                    tools.push(name);
                }
            }
            tools
        });

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
        let (tool, _, ctx, dir) = fixture_with(turns, None);
        (tool, ctx, dir)
    }

    /// The same fixture, plus the provider (to read back what the child was
    /// actually sent) and an optional custom roster.
    fn fixture_with(
        turns: Vec<ScriptedTurn>,
        agents: Option<Vec<AgentDefinition>>,
    ) -> (SpawnSubagent, Arc<FakeProvider>, ToolContext, TempDir) {
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

        let mut tool = SpawnSubagent::new(provider.clone(), registry, "fake", 2);
        if let Some(agents) = agents {
            tool = tool.with_roster(Arc::new(agents), ModelOverrides::new());
        }
        (tool, provider, ctx, dir)
    }

    /// A definition as a discovered file would produce one.
    fn custom(name: &str, prompt: &str, tools: Option<Vec<&str>>) -> AgentDefinition {
        AgentDefinition {
            frontmatter: taurus_agents::AgentFrontmatter {
                name: name.into(),
                description: format!("does {name}"),
                tools: tools.map(|t| t.into_iter().map(String::from).collect()),
                max_iterations: 5,
                model: None,
                provider: None,
            },
            system_prompt: prompt.into(),
            tier: taurus_agents::AgentTier::User,
            path: Some(std::path::PathBuf::from(format!("/agents/{name}.md"))),
            borrowed: false,
            shadows: None,
            degraded: None,
        }
    }

    /// A recorder that keeps what it was given, standing in for the host's
    /// transcript files.
    #[derive(Default)]
    struct SpyRecorder {
        opened: tokio::sync::Mutex<Vec<(String, String)>>,
        kept: Arc<tokio::sync::Mutex<Vec<Message>>>,
    }

    #[async_trait]
    impl SubagentRecorder for SpyRecorder {
        async fn open(&self, agent_type: &str, child: &Session) -> Option<Arc<dyn TurnRecorder>> {
            self.opened
                .lock()
                .await
                .push((agent_type.to_string(), child.id.clone()));
            Some(Arc::new(SpySink {
                kept: self.kept.clone(),
            }))
        }
    }

    struct SpySink {
        kept: Arc<tokio::sync::Mutex<Vec<Message>>>,
    }

    #[async_trait]
    impl TurnRecorder for SpySink {
        async fn record(&self, session: &Session) {
            *self.kept.lock().await = session.messages.clone();
        }
    }

    /// Catches what the tool told the card about this call.
    #[derive(Default)]
    struct SpyProgress {
        transcripts: tokio::sync::Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl taurus_tools::ToolProgress for SpyProgress {
        async fn step(&self, _label: String) {}

        async fn transcript(&self, session: String, agent: String) {
            self.transcripts.lock().await.push((session, agent));
        }
    }

    #[tokio::test]
    async fn a_kept_conversation_is_announced_to_the_card_that_could_open_it() {
        let (tool, ctx, _dir) = fixture(vec![ScriptedTurn::text("Three files.")]);
        let spy = Arc::new(SpyRecorder::default());
        let progress = Arc::new(SpyProgress::default());
        let tool = tool.with_recorder(spy.clone());
        let ctx = ctx.with_progress(progress.clone());

        tool.execute(
            serde_json::json!({
                "agent_type": "explorer",
                "prompt": "Count the files in this directory and report back."
            }),
            &ctx,
        )
        .await
        .unwrap();

        let announced = progress.transcripts.lock().await.clone();
        assert_eq!(announced.len(), 1);
        assert_eq!(announced[0].1, "explorer");
        // The id the card would ask for is the one the recorder opened.
        let opened = spy.opened.lock().await.clone();
        assert_eq!(announced[0].0, opened[0].1);
    }

    #[tokio::test]
    async fn nothing_is_announced_when_nothing_is_being_recorded() {
        // No recorder, no transcript, nothing offered. A card that invited
        // somebody to open a conversation that was never written is worse than
        // one that offers nothing at all.
        let (tool, ctx, _dir) = fixture(vec![ScriptedTurn::text("Three files.")]);
        let progress = Arc::new(SpyProgress::default());
        let ctx = ctx.with_progress(progress.clone());

        tool.execute(
            serde_json::json!({
                "agent_type": "explorer",
                "prompt": "Count the files in this directory and report back."
            }),
            &ctx,
        )
        .await
        .unwrap();

        assert!(progress.transcripts.lock().await.is_empty());
    }

    #[tokio::test]
    async fn a_delegates_conversation_is_kept_not_dropped_with_the_tool_result() {
        let (tool, ctx, _dir) = fixture(vec![
            ScriptedTurn::tool_call("t1", "list_dir", serde_json::json!({"path": "."})),
            ScriptedTurn::text("There are three files."),
        ]);
        let spy = Arc::new(SpyRecorder::default());
        let tool = tool.with_recorder(spy.clone());

        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "explorer",
                    "prompt": "Count the files in this directory and report back."
                }),
                &ctx,
            )
            .await
            .unwrap();

        // What the parent gets back is still one paragraph.
        assert!(out.contains("There are three files."));

        let opened = spy.opened.lock().await.clone();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].0, "explorer", "recorded under the kind it was");
        assert!(!opened[0].1.is_empty(), "and under its own id");

        // What was kept is the part the parent never sees: the task it was
        // given, the tool call it made, and the result that came back.
        let kept = spy.kept.lock().await.clone();
        assert!(kept.len() > 2, "{kept:#?}");
        assert!(kept[0].text().contains("Count the files"));
        assert!(kept.iter().any(|m| m.has_tool_use()));
    }

    #[tokio::test]
    async fn a_delegate_that_never_finished_still_left_a_transcript() {
        // The child asks for a tool that does not exist, over and over, until
        // the loop stops it. Nothing useful comes back to the parent — which is
        // exactly when somebody wants to read what the child was doing.
        let (tool, ctx, _dir) = fixture(vec![
            ScriptedTurn::tool_call("t1", "no_such_tool", serde_json::json!({})),
            ScriptedTurn::tool_call("t2", "no_such_tool", serde_json::json!({})),
            ScriptedTurn::tool_call("t3", "no_such_tool", serde_json::json!({})),
            ScriptedTurn::tool_call("t4", "no_such_tool", serde_json::json!({})),
        ]);
        let spy = Arc::new(SpyRecorder::default());
        let tool = tool.with_recorder(spy.clone());

        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "explorer",
                    "prompt": "Use the tool that does not exist, repeatedly."
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("stopped early"), "{out}");

        let kept = spy.kept.lock().await.clone();
        assert!(!kept.is_empty(), "a turn that failed is still a transcript");
        assert!(kept.iter().any(|m| m.has_tool_use()));
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

    #[tokio::test]
    async fn a_user_agent_shadows_a_builtin_of_the_same_name() {
        let (tool, provider, ctx, _dir) = fixture_with(
            vec![ScriptedTurn::text("Done.")],
            Some(vec![custom(
                "explorer",
                "You are the explorer this user wrote.",
                Some(vec!["read_file"]),
            )]),
        );
        tool.execute(
            serde_json::json!({
                "agent_type": "explorer",
                "prompt": "Look at the workspace and describe what you find."
            }),
            &ctx,
        )
        .await
        .unwrap();

        let system = provider.last_request().await.unwrap().system.unwrap();
        assert!(system.contains("the explorer this user wrote"));
    }

    #[tokio::test]
    async fn a_tools_list_is_actually_enforced() {
        // Unlike a skill's `allowed_tools`, which grants and withholds nothing.
        let (tool, provider, ctx, _dir) = fixture_with(
            vec![ScriptedTurn::text("Done.")],
            Some(vec![custom(
                "reader",
                "Read only.",
                Some(vec!["read_file"]),
            )]),
        );
        tool.execute(
            serde_json::json!({
                "agent_type": "reader",
                "prompt": "Read something and report back what it said."
            }),
            &ctx,
        )
        .await
        .unwrap();

        let offered: Vec<String> = provider
            .last_request()
            .await
            .unwrap()
            .tools
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(offered, vec!["read_file".to_string()]);
    }

    #[tokio::test]
    async fn an_absent_tools_key_inherits_the_parent_set() {
        // `worker` has always had everything the parent has minus the spawn
        // tool, and that must not change under it.
        let (tool, provider, ctx, _dir) = fixture_with(vec![ScriptedTurn::text("Done.")], None);
        tool.execute(
            serde_json::json!({
                "agent_type": "worker",
                "prompt": "Make the change described in the instructions above."
            }),
            &ctx,
        )
        .await
        .unwrap();

        let offered: Vec<String> = provider
            .last_request()
            .await
            .unwrap()
            .tools
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(offered.len() > 1);
        assert!(offered.contains(&"read_file".to_string()));
        assert!(
            !offered.contains(&SPAWN_TOOL.to_string()),
            "inheriting must still stop at the depth cap"
        );
    }

    #[tokio::test]
    async fn an_agent_whose_tools_all_vanished_is_refused_not_widened() {
        // The one failure in this feature that is a permission *widening* rather
        // than a broken feature: an empty allow-list means "everything", so a
        // scope that filtered down to nothing must refuse instead of inherit.
        let (tool, ctx, _dir) = {
            let (tool, _, ctx, dir) = fixture_with(
                vec![ScriptedTurn::text("Done.")],
                Some(vec![custom(
                    "scoped",
                    "Narrow by design.",
                    Some(vec!["tool_that_is_not_registered"]),
                )]),
            );
            (tool, ctx, dir)
        };
        let err = tool
            .execute(
                serde_json::json!({
                    "agent_type": "scoped",
                    "prompt": "Do the narrow thing you were scoped to do."
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("would run unrestricted"));
    }

    #[tokio::test]
    async fn a_model_override_sends_the_child_to_a_different_model() {
        let (tool, provider, ctx, _dir) = fixture_with(
            vec![ScriptedTurn::text("Done.")],
            Some(vec![custom(
                "big-thinker",
                "Think hard.",
                Some(vec!["read_file"]),
            )]),
        );
        let overrides: ModelOverrides = [(
            "big-thinker".to_string(),
            AgentModel {
                provider: Some(provider.clone() as Arc<dyn Provider>),
                model: "qwen3:32b".to_string(),
            },
        )]
        .into_iter()
        .collect();
        let tool = tool.with_roster(
            Arc::new(vec![custom(
                "big-thinker",
                "Think hard.",
                Some(vec!["read_file"]),
            )]),
            overrides,
        );

        tool.execute(
            serde_json::json!({
                "agent_type": "big-thinker",
                "prompt": "Think about this problem and report your conclusion."
            }),
            &ctx,
        )
        .await
        .unwrap();

        assert_eq!(provider.last_request().await.unwrap().model, "qwen3:32b");
    }

    #[test]
    fn the_schema_enumerates_the_live_roster() {
        // Derived statically from `SpawnInput` and patched, so it stops working
        // silently if schemars changes its output shape. Hence the test.
        let agents = vec![custom("alpha", "a", None), custom("beta", "b", None)];
        let schema = schema_with_agent_types(&agents);
        let names = schema
            .pointer("/properties/agent_type/enum")
            .expect("agent_type must carry an enum of the live names");
        assert_eq!(names, &serde_json::json!(["alpha", "beta"]));
    }

    #[test]
    fn the_description_lists_the_live_roster() {
        let text = describe(&[custom("reviewer", "r", None)]);
        assert!(text.contains("- reviewer: does reviewer"));
        assert!(text.contains("cannot delegate further"));
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
