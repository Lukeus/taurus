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
use taurus_tools::{
    now_ms, Arrival, CheckpointStore, DelegateReport, DelegateStep, Disposition, Effect, Isolation,
    Opened, Requester, Tool, ToolContext, ToolError, ToolRegistry, ToolResult, Touched,
};

use crate::agent::{Agent, AgentConfig, AgentError, TurnOutcome, TurnRecorder};
use crate::event::UiEvent;
use crate::finish::{Finish, Reported, Slot, FINISH_TOOL};
use crate::session::Session;

/// Appended to every delegate's system prompt, whoever wrote the agent.
///
/// Here rather than in each built-in's prompt because an agent file on disk
/// was written before `finish` existed and can't be relied on to mention it.
const REPORT_BACK: &str = "\
When you're done, call `finish`. Report `done` with what you found or \
changed; `blocked` with who has to act (`waiting_on`) and the one thing they \
have to do (`needs`); or `failed` with why it can't be done. Its `summary` is \
all the agent that called you will read.";

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
    /// Start it and keep working; its report arrives on its own when it
    /// finishes. An agent that can write gets its own copy of the workspace
    /// for this, as with `isolate`.
    #[serde(default)]
    pub background: bool,
    /// Give an agent that can write its own copy of the workspace (a git
    /// worktree), so it can run beside other agents. What it changes is kept
    /// for you to apply with `apply_delegate`, rather than landing in the
    /// workspace.
    #[serde(default)]
    pub isolate: bool,
}

/// The one tool an isolated delegate's copy can't hold it to among the
/// read-only ones: it refreshes the index kept for the parent's folder, from
/// whichever folder it's called in.
const PARENT_INDEX_TOOL: &str = "search_code";

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
    /// `call` is the parent's tool call that started this child, so a
    /// conversation reopened later can put each transcript back on its card.
    async fn open(
        &self,
        agent_type: &str,
        child: &Session,
        call: Option<&str>,
    ) -> Option<Arc<dyn TurnRecorder>>;
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
    /// What a child runs with, apart from the three things its definition
    /// decides: its prompt, its iteration ceiling, and its tools. The parent's
    /// config, so content capture, output caps and the retry settings reach a
    /// delegate the way they reach the turn that spawned it.
    defaults: AgentConfig,
    /// Where a delegate that writes gets a copy of the workspace, when the host
    /// can give it one. See [`taurus_tools::isolation`].
    isolation: Option<Arc<dyn Isolation>>,
    /// Held by a delegate that writes in the workspace itself, for its whole
    /// run. Two writers in one folder would each edit what the other is
    /// halfway through, so one waits. A writer the round let run beside others,
    /// because it asked for a copy it then couldn't have, is the one this
    /// catches: it falls back to the workspace, and waits its turn there.
    alone: Arc<tokio::sync::Mutex<()>>,
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
            defaults: AgentConfig::default(),
            isolation: None,
            alone: Arc::default(),
        }
    }

    /// Lets a delegate that writes have a copy of the workspace, so it can run
    /// beside other agents and in the background.
    pub fn with_isolation(mut self, isolation: Arc<dyn Isolation>) -> Self {
        self.isolation = Some(isolation);
        self
    }

    /// Keeps every child's conversation, wherever the host puts transcripts.
    pub fn with_recorder(mut self, recorder: Arc<dyn SubagentRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// Runs every child with the parent's config, apart from the prompt, the
    /// iteration ceiling and the tools, which each child's definition sets.
    pub fn with_defaults(mut self, defaults: AgentConfig) -> Self {
        self.defaults = defaults;
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

    /// Only a delegate that can do nothing but read runs beside the rest of
    /// its round. One that can write runs alone, because two in the same
    /// working tree would each edit files the other is halfway through.
    ///
    /// Anything that can't be read cleanly here answers "alone": an agent
    /// with no `tools:` key inherits the parent's writers, an unknown agent is
    /// about to be refused anyway, and a registry being written to right now
    /// is one this can't see into. Serializing a reader costs time; letting a
    /// writer through costs files.
    ///
    /// A writer that asked for a copy runs beside the rest too, when there's
    /// somewhere to make one. If the copy then can't be made, it works in the
    /// workspace after all, and [`SpawnSubagent::alone`] makes it wait for any
    /// other writer there.
    fn runs_concurrently(&self, input: &serde_json::Value) -> bool {
        let Some(definition) = input
            .get("agent_type")
            .and_then(|v| v.as_str())
            .and_then(|name| self.definition(name))
        else {
            return false;
        };
        // Tried, not waited for: this is sync, and a busy registry answering
        // "alone" only costs time.
        if self
            .registry
            .try_read()
            .is_ok_and(|registry| reads_only_in(&registry, definition))
        {
            return true;
        }
        let asked = |key: &str| input.get(key).and_then(|v| v.as_bool()) == Some(true);
        self.isolation.is_some() && (asked("isolate") || asked("background"))
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

        let writes = !self.reads_only(definition).await;
        let mut notes = Vec::new();
        let opened = if writes && (input.isolate || input.background) {
            let tried = match &self.isolation {
                Some(isolation) => {
                    let who = Requester {
                        agent: definition.name().to_string(),
                        session: ctx.session_id.clone(),
                        call: ctx.call_id.clone(),
                    };
                    isolation
                        .open(&ctx.workspace, &who)
                        .await
                        .map(|opened| (isolation.clone(), opened))
                }
                None => Err("Taurus can't make a copy of the workspace here.".to_string()),
            };
            match tried {
                Ok(opened) => Some(opened),
                // In the background it would be writing beside the parent,
                // with nothing to stop the two meeting in one file.
                Err(why) if input.background => {
                    return Err(ToolError::InvalidInput(format!(
                        "'{}' can write, so in the background it needs a copy of the workspace \
                         of its own, and it can't have one: {why} Run it without `background` \
                         to have it work in the workspace while you wait, or delegate the \
                         reading to `{}` in the background.",
                        definition.name(),
                        builtin::EXPLORER
                    )));
                }
                Err(why) => {
                    notes.push(format!(
                        "It asked for a copy of the workspace and couldn't have one, so it worked \
                         in the workspace itself, with no other agent writing there. {why}"
                    ));
                    None
                }
            }
        } else {
            None
        };

        // A refusal here comes after the copy was made, and nothing else will
        // close it: the launch that would have is never built.
        let made = opened
            .as_ref()
            .map(|(isolation, opened)| (isolation.clone(), opened.run.clone()));
        let mut launch = match self.prepare(definition, ctx, opened).await {
            Ok(launch) => launch,
            Err(e) => {
                if let Some((isolation, run)) = made {
                    if let Err(why) = isolation.close(&run).await {
                        tracing::warn!(%run, error = %why, "couldn't close a copy nobody used");
                    }
                }
                return Err(e);
            }
        };
        launch.notes.extend(notes);
        if writes && launch.isolated.is_none() {
            launch.alone = Some(self.alone.clone());
        }

        if !input.background {
            return Ok(launch.run(input.prompt, ctx.clone()).await.into());
        }

        let Some(pending) = ctx.pending.clone() else {
            // Refused after its copy was made: close it, or the copy and its
            // lock stay for the life of the process with nobody to close them.
            if let Some((isolation, opened)) = launch.isolated.take() {
                if let Err(why) = isolation.close(&opened.run).await {
                    tracing::warn!(run = %opened.run, error = %why, "couldn't close a copy nobody used");
                }
            }
            return Err(no_background());
        };
        // Counted before this call returns, so the turn never sees the work
        // out but uncounted.
        let ticket = pending.start();
        let call = ctx.call_id.clone().unwrap_or_default();
        let agent = definition.name().to_string();
        // Its own stop: the turn's Stop still reaches it, and the turn can stop
        // it on any other way out without cancelling itself.
        let mut child = ctx.clone();
        child.cancel = pending.stop_token();
        ctx.report_detached().await;
        tokio::spawn(async move {
            let text = launch.run(input.prompt, child).await;
            ticket.deliver(Arrival { call, agent, text });
        });

        Ok(format!(
            "Started {} in the background. Keep working: its report arrives on its own when it \
             finishes, tagged with this call's id. There's no way to read it sooner, so don't \
             wait for it or ask about it.",
            definition.name()
        )
        .into())
    }
}

/// Whether every tool `definition` names only reads, in `registry`.
fn reads_only_in(registry: &ToolRegistry, definition: &AgentDefinition) -> bool {
    let Some(tools) = &definition.frontmatter.tools else {
        return false;
    };
    tools.iter().all(|name| {
        registry
            .get(name)
            .is_none_or(|tool| tool.effect().is_concurrent_safe())
    })
}

fn no_background() -> ToolError {
    ToolError::InvalidInput(
        "Background delegation isn't available here: nothing would be waiting for the report. \
         Run it without `background`."
            .into(),
    )
}

/// Everything one delegation needs, owned, so it can run where the call that
/// started it can't follow: in a task, after that call has returned.
struct Launch {
    name: String,
    provider: Arc<dyn Provider>,
    model: String,
    registry: ToolRegistry,
    slot: Slot,
    config: AgentConfig,
    recorder: Option<Arc<dyn SubagentRecorder>>,
    permits: Arc<Semaphore>,
    /// The copy it works in, and what made it, which closes it.
    isolated: Option<(Arc<dyn Isolation>, Opened)>,
    /// Taken for the whole run by a writer working in the workspace itself.
    alone: Option<Arc<tokio::sync::Mutex<()>>>,
    /// What the harness says on its report. See [`DelegateReport::notes`].
    notes: Vec<String>,
}

impl SpawnSubagent {
    /// Whether every tool this agent names only reads. See
    /// [`Tool::runs_concurrently`] for why the doubtful cases answer no.
    ///
    /// Waits for the registry rather than trying it: a reload holding it would
    /// otherwise turn a read-only agent into a writer for this one call.
    async fn reads_only(&self, definition: &AgentDefinition) -> bool {
        reads_only_in(&*self.registry.read().await, definition)
    }

    async fn prepare(
        &self,
        definition: &AgentDefinition,
        ctx: &ToolContext,
        isolated: Option<(Arc<dyn Isolation>, Opened)>,
    ) -> Result<Launch, ToolError> {
        // Depth cap, enforced structurally: the child's registry has no spawn
        // tool, so it cannot delegate no matter what it decides to do.
        let mut registry = self.registry.read().await.without(SPAWN_TOOL);
        // A copy holds a delegate only through tools that work where they're
        // told to: the built-ins, which resolve every path against the
        // context's folder. Anything else that can change something (an MCP
        // server's write, a skill's script) works wherever it was set up to,
        // which is the person's folder, so it's left out rather than trusted to
        // stay in the copy. Isolation the prompt has to keep isn't isolation.
        let mut withheld = Vec::new();
        if isolated.is_some() {
            let builtins = ToolRegistry::with_builtins();
            let names: Vec<String> = registry.names().map(str::to_owned).collect();
            for name in names {
                let keep = builtins.get(&name).is_some()
                    || (name != PARENT_INDEX_TOOL
                        && registry
                            .get(&name)
                            .is_some_and(|tool| tool.effect() == Effect::Read));
                if !keep {
                    withheld.push(name);
                }
            }
            for name in &withheld {
                registry = registry.without(name);
            }
        }
        // Its own, per delegation: the report it holds is this child's.
        let slot = Slot::default();
        registry.register(Arc::new(Finish::new(slot.clone())));

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
                let mut available: Vec<String> = wanted
                    .iter()
                    .filter(|name| registry.get(name).is_some())
                    .cloned()
                    .collect();
                // Counted before `finish` joins it: a scope that named nothing
                // available is still one that would otherwise run wide open.
                if available.is_empty() {
                    return Err(ToolError::Failed(format!(
                        "The sub-agent '{}' is scoped to tools that are not available here ({}), \
                         so it would run unrestricted. It has been refused instead. Enable those \
                         tools, or widen its `tools:` list.",
                        definition.name(),
                        wanted.join(", ")
                    )));
                }
                available.push(FINISH_TOOL.to_string());
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

        let mut notes = Vec::new();
        let place = match &isolated {
            Some((_, opened)) => {
                if let Some(note) = &opened.note {
                    notes.push(note.clone());
                }
                if !withheld.is_empty() {
                    withheld.sort();
                    notes.push(format!(
                        "It had no {} in its copy, because they can't be held to it.",
                        withheld.join(", ")
                    ));
                }
                format!(
                    "You are working in `{}`. It is a copy of the workspace made for you, as it \
                     stood when you started, so nothing you change reaches the original until \
                     someone applies it. Work only here: never `cd` out of it or write by \
                     absolute path to anywhere else.",
                    opened.root.display()
                )
            }
            None => format!("You are working in `{}`.", ctx.workspace.display()),
        };

        Ok(Launch {
            name: definition.name().to_string(),
            provider,
            model,
            registry,
            slot,
            config: AgentConfig {
                system_prompt: format!("{}\n\n{REPORT_BACK}\n\n{place}", definition.system_prompt),
                max_iterations: definition.frontmatter.max_iterations,
                allowed_tools: allowed,
                turn_hooks: false,
                ..self.defaults.clone()
            },
            recorder: self.recorder.clone(),
            permits: self.permits.clone(),
            isolated,
            alone: None,
            notes,
        })
    }
}

impl Launch {
    /// Runs the delegate to its end and returns its report as the parent
    /// reads it.
    ///
    /// `ctx` is the context of the call that started it, whose progress and
    /// report land on that call's card, and whose cancel stops it.
    async fn run(self, prompt: String, ctx: ToolContext) -> String {
        // Held for the whole run, in whichever task that is. A background
        // delegate waiting for a permit hasn't started, which is still right:
        // the cap is on children running, not children asked for. Said when
        // it has to wait, so a card waiting on a slot isn't mistaken for one
        // that has stalled.
        let _permit = match self.permits.clone().try_acquire_owned() {
            Ok(permit) => Ok(permit),
            Err(_) => {
                ctx.report_activity(DelegateStep::Queued { at: now_ms() })
                    .await;
                self.permits.clone().acquire_owned().await
            }
        };
        // After the permit, so a writer queued for a slot isn't also holding
        // the workspace against the others.
        let _alone = match &self.alone {
            Some(alone) => Some(alone.clone().lock_owned().await),
            None => None,
        };
        ctx.report_activity(DelegateStep::Started {
            agent: self.name.clone(),
            model: self.model.clone(),
            max_rounds: self.config.max_iterations,
            at: now_ms(),
        })
        .await;

        // Before the agent, because opening a recorder needs the id of the
        // conversation it is recording.
        let mut session = Session::new(&self.model);
        let touched = Arc::new(Touched::default());
        // A delegate in a copy works there for everything: its paths, its
        // commands, its own undo record, its own background commands. See
        // `ToolContext::rooted_at` for why each of those can't be shared.
        let works_in = match &self.isolated {
            Some((_, opened)) => {
                let records = CheckpointStore::new(&opened.scratch);
                let recorder = records.begin_turn(&opened.run, &opened.root, &prompt);
                ctx.clone().rooted_at(opened.root.clone(), Some(recorder))
            }
            None => ctx.clone(),
        };
        let own_jobs = self.isolated.as_ref().and_then(|_| works_in.jobs.clone());
        let agent = Agent::new(
            self.provider,
            self.registry,
            works_in.with_touched(touched.clone()),
            self.config,
        );

        let agent = match &self.recorder {
            Some(recorder) => match recorder
                .open(&self.name, &session, ctx.call_id.as_deref())
                .await
            {
                // Announced only once there is somewhere to read: a card that
                // offered to open a transcript nobody was writing would be a
                // worse answer than one that offers nothing.
                Some(sink) => {
                    ctx.report_transcript(session.id.clone(), &self.name).await;
                    agent.with_recorder(sink)
                }
                None => agent,
            },
            None => agent,
        };

        info!(kind = %self.name, model = %self.model, "spawning sub-agent");

        // The child's own text and results stay out of the parent's
        // transcript — it shows one delegation, not a second conversation.
        // But the child's run is forwarded as it happens, as typed steps under
        // the parent's call: a delegation can run for a minute, and a card
        // that says "running" for that long is indistinguishable from one that
        // has hung. The agents panel draws all of it; the card draws a line.
        let (tx, mut rx) = mpsc::channel::<UiEvent>(256);
        let progress = ctx.clone();
        let collector = tokio::spawn(async move {
            let mut tools: Vec<String> = Vec::new();
            while let Some(event) = rx.recv().await {
                if let UiEvent::ToolCallStarted { name, .. } = &event {
                    tools.push(name.clone());
                }
                if let Some(step) = step_of(event) {
                    progress.report_activity(step).await;
                }
            }
            tools
        });

        let outcome = agent
            .run_turn(&mut session, Message::user(&prompt), tx)
            .await;
        let tools_used = collector.await.unwrap_or_default();
        // The child's whole conversation, its own compaction included, onto
        // the parent's bill. Its transcript is kept apart from the parent's
        // listing, so this is the only place it is counted.
        ctx.add_spent(&session.spend);
        ctx.report_activity(DelegateStep::Finished {
            input_tokens: session.usage.input_tokens,
            output_tokens: session.usage.output_tokens,
            at: now_ms(),
        })
        .await;

        let reported = self.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        let mut report = report_for(reported, &outcome, &session, touched.paths());
        report.notes = self.notes;
        if let Some((isolation, opened)) = &self.isolated {
            // A dev server it left running would hold the folder open, and
            // there would be nobody left to read it or stop it.
            if let Some(jobs) = own_jobs {
                jobs.stop_all();
            }
            // What's in the copy is the record of what it changed: a command's
            // writes the sweep missed are in it too.
            match isolation.close(&opened.run).await {
                Ok(closed) => {
                    report.files = closed
                        .kept
                        .as_ref()
                        .map(|kept| kept.files.clone())
                        .unwrap_or_default();
                    if closed.kept.is_none() {
                        report.notes.push(
                            "It worked in a copy of the workspace and changed nothing.".into(),
                        );
                    }
                    report.kept = closed.kept;
                    report.notes.extend(closed.left_behind);
                }
                Err(e) => report.notes.push(format!(
                    "Its copy of the workspace (run {}) couldn't be closed: {e} What it changed \
                     is still there, and Taurus closes it when it next starts.",
                    opened.run
                )),
            }
        }
        ctx.report_delegate(report.clone()).await;

        let mut text = report.render();
        if !tools_used.is_empty() {
            text.push_str(&format!("\n\n[sub-agent used: {}]", summarize(&tools_used)));
        }
        text
    }
}

/// What a child's event says to someone watching the delegation, if anything.
///
/// Only what an agent visibly does: its rounds, its words, its tool calls and
/// how they came out, how full it is, and trouble. Its own bookkeeping (turn
/// boundaries, files changed, compaction) is either in the report or not
/// something a person watching needs.
fn step_of(event: UiEvent) -> Option<DelegateStep> {
    Some(match event {
        UiEvent::IterationStarted { iteration } => DelegateStep::Round { n: iteration },
        UiEvent::TextDelta { text } => DelegateStep::Text { text },
        UiEvent::ThinkingDelta { text } => DelegateStep::Thinking { text },
        UiEvent::ToolCallStarted {
            id, name, preview, ..
        } => DelegateStep::ToolStarted {
            call: id,
            name,
            preview,
            at: now_ms(),
        },
        UiEvent::ToolCallFinished { id, ok, .. } => DelegateStep::ToolFinished {
            call: id,
            ok,
            at: now_ms(),
        },
        UiEvent::ContextUsed { used, window, .. } => DelegateStep::Context { used, window },
        UiEvent::Retrying {
            attempt,
            of,
            reason,
        } => DelegateStep::Retrying {
            attempt,
            of,
            reason,
        },
        UiEvent::Error { message } => DelegateStep::Error { message },
        _ => return None,
    })
}

/// The report a delegation hands back, from what the child said and how its
/// turn ended.
///
/// What the child reported wins whenever there is one, even if the turn went
/// on to fail or be stopped: a report is a claim about the task, and a nudge
/// that ran out of iterations afterwards doesn't unmake it. Without one, the
/// outcome decides, and the child's last message goes along as the summary,
/// labeled as what it is.
fn report_for(
    reported: Option<Reported>,
    outcome: &Result<TurnOutcome, AgentError>,
    session: &Session,
    files: Vec<String>,
) -> DelegateReport {
    if let Some(reported) = reported {
        return DelegateReport {
            disposition: reported.disposition,
            owner: reported.owner,
            needs: reported.needs,
            summary: reported.summary,
            files,
            notes: Vec::new(),
            kept: None,
        };
    }

    let last = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == taurus_provider::Role::Assistant && !m.text().trim().is_empty())
        .map(|m| m.text())
        .unwrap_or_default();
    let (disposition, summary) = match outcome {
        Ok(done) if done.stop_reason == taurus_provider::StopReason::Canceled => (
            Disposition::Cancelled,
            if last.trim().is_empty() {
                "Stopped before it said anything.".to_string()
            } else {
                format!("Stopped before it finished. Its last message:\n\n{last}")
            },
        ),
        Ok(_) => (Disposition::Unreported, last),
        Err(e) => (
            Disposition::Failed,
            if last.trim().is_empty() {
                format!("It stopped early ({e}) without saying anything.")
            } else {
                format!("It stopped early ({e}). Its last message:\n\n{last}")
            },
        ),
    };
    DelegateReport {
        disposition,
        owner: None,
        needs: None,
        summary,
        files,
        notes: Vec::new(),
        kept: None,
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
            plugin: None,
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
        async fn open(
            &self,
            agent_type: &str,
            child: &Session,
            _call: Option<&str>,
        ) -> Option<Arc<dyn TurnRecorder>> {
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
        reports: tokio::sync::Mutex<Vec<DelegateReport>>,
        steps: tokio::sync::Mutex<Vec<DelegateStep>>,
    }

    #[async_trait]
    impl taurus_tools::ToolProgress for SpyProgress {
        async fn step(&self, _label: String) {}

        async fn transcript(&self, session: String, agent: String) {
            self.transcripts.lock().await.push((session, agent));
        }

        async fn delegate_report(&self, report: DelegateReport) {
            self.reports.lock().await.push(report);
        }

        async fn activity(&self, step: DelegateStep) {
            self.steps.lock().await.push(step);
        }
    }

    fn finish(id: &str, input: serde_json::Value) -> ScriptedTurn {
        ScriptedTurn::tool_call(id, FINISH_TOOL, input)
    }

    fn write(id: &str, path: &str) -> ScriptedTurn {
        ScriptedTurn::tool_call(
            id,
            "write_file",
            serde_json::json!({ "path": path, "content": "new\n" }),
        )
    }

    /// Opens a checkpointed turn on the context, as the host does, and returns
    /// the recorder so a test can play the parent's part in it.
    fn checkpointed(
        ctx: ToolContext,
        logs: &TempDir,
    ) -> (ToolContext, Arc<taurus_tools::TurnRecorder>) {
        let store = taurus_tools::CheckpointStore::new(logs.path());
        let recorder = store.begin_turn("parent", &ctx.workspace, "the parent's prompt");
        (ctx.with_checkpoints(recorder.clone()), recorder)
    }

    #[tokio::test]
    async fn a_delegate_that_answers_in_prose_is_asked_once_to_report() {
        let (tool, provider, ctx, _dir) = fixture_with(
            vec![
                ScriptedTurn::text("It's in src/lib.rs."),
                finish(
                    "f1",
                    serde_json::json!({ "disposition": "done", "summary": "It's in src/lib.rs." }),
                ),
            ],
            None,
        );
        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "explorer",
                    "prompt": "Find where the parser lives and report the file."
                }),
                &ctx,
            )
            .await
            .unwrap()
            .to_text()
            .into_owned();
        assert!(out.starts_with("Status: done."), "{out}");
        assert_eq!(provider.request_count().await, 2);
        let asked = provider.last_request().await.unwrap();
        let nudge = asked.messages.last().unwrap().text();
        assert!(nudge.contains("without calling `finish`"), "{nudge}");
    }

    #[tokio::test]
    async fn what_a_delegate_spent_is_handed_to_the_parent() {
        // Two requests: the prose answer and the nudged report. Both are the
        // parent's bill, and its transcript is the only place a total is read.
        let (tool, _provider, ctx, _dir) = fixture_with(
            vec![
                ScriptedTurn::text("It's in src/lib.rs."),
                finish(
                    "f1",
                    serde_json::json!({ "disposition": "done", "summary": "It's in src/lib.rs." }),
                ),
            ],
            None,
        );
        let spent = Arc::new(std::sync::Mutex::new(taurus_provider::Spend::default()));
        let ctx = ctx.with_spent(spent.clone());
        tool.execute(
            serde_json::json!({ "agent_type": "explorer", "prompt": "Find the parser." }),
            &ctx,
        )
        .await
        .unwrap();
        let spent = spent.lock().unwrap().clone();
        let total = spent.total();
        assert_eq!(total.output_tokens, 2, "{spent:?}");
        assert!(total.input_tokens > 0, "{spent:?}");
        // Against the model the child ran on, so a delegate routed elsewhere
        // shows up as that model's spending rather than the parent's.
        assert_eq!(spent.0.len(), 1, "{spent:?}");
    }

    #[tokio::test]
    async fn a_report_ends_the_delegates_turn() {
        let (tool, provider, ctx, _dir) = fixture_with(
            vec![finish(
                "f1",
                serde_json::json!({ "disposition": "done", "summary": "It's in src/lib.rs." }),
            )],
            None,
        );
        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "explorer",
                    "prompt": "Find where the parser lives and report the file."
                }),
                &ctx,
            )
            .await
            .unwrap()
            .to_text()
            .into_owned();
        assert!(
            out.starts_with("Status: done.\n\nIt's in src/lib.rs."),
            "{out}"
        );
        // No second request to write a closing paragraph nobody reads.
        assert_eq!(provider.request_count().await, 1);
    }

    #[tokio::test]
    async fn a_delegation_reports_its_run_as_it_happens() {
        // The agents panel draws a delegate from these: it started, which
        // round it's on, each tool call and how it came out, and that it
        // stopped. A card that only said "running" for a minute looked hung.
        let (tool, ctx, dir) = fixture(vec![
            ScriptedTurn::tool_call("r1", "list_dir", serde_json::json!({ "path": "." })),
            finish(
                "f1",
                serde_json::json!({ "disposition": "done", "summary": "Listed it." }),
            ),
        ]);
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        let progress = Arc::new(SpyProgress::default());
        let ctx = ctx.with_progress(progress.clone());
        tool.execute(
            serde_json::json!({
                "agent_type": "explorer",
                "prompt": "List what is at the top of the workspace."
            }),
            &ctx,
        )
        .await
        .unwrap();

        let steps = progress.steps.lock().await;
        let kinds: Vec<&str> = steps
            .iter()
            .map(|step| match step {
                DelegateStep::Queued { .. } => "queued",
                DelegateStep::Started { .. } => "started",
                DelegateStep::Round { .. } => "round",
                DelegateStep::Text { .. } => "text",
                DelegateStep::Thinking { .. } => "thinking",
                DelegateStep::ToolStarted { .. } => "tool_started",
                DelegateStep::ToolFinished { .. } => "tool_finished",
                DelegateStep::Context { .. } => "context",
                DelegateStep::Retrying { .. } => "retrying",
                DelegateStep::Error { .. } => "error",
                DelegateStep::Finished { .. } => "finished",
            })
            .filter(|kind| *kind != "context")
            .collect();
        assert_eq!(kinds.first(), Some(&"started"), "{kinds:?}");
        assert_eq!(kinds.last(), Some(&"finished"), "{kinds:?}");
        let listed = steps.iter().position(|step| {
            matches!(step, DelegateStep::ToolStarted { call, name, .. } if call == "r1" && name == "list_dir")
        });
        let back = steps.iter().position(|step| {
            matches!(step, DelegateStep::ToolFinished { call, ok: true, .. } if call == "r1")
        });
        assert!(listed.is_some() && back > listed, "{kinds:?}");
        assert!(kinds.contains(&"round"), "{kinds:?}");
        let DelegateStep::Started {
            agent, max_rounds, ..
        } = &steps[0]
        else {
            unreachable!()
        };
        assert_eq!(agent, "explorer");
        assert!(*max_rounds > 0);
    }

    #[tokio::test]
    async fn a_delegation_waiting_for_a_slot_says_so_before_it_starts() {
        let (tool, ctx, _dir) = fixture(vec![finish(
            "f1",
            serde_json::json!({ "disposition": "done", "summary": "Done." }),
        )]);
        // Both slots taken, as two delegates already running would hold them.
        let held = tool.permits.clone().acquire_many_owned(2).await.unwrap();
        let progress = Arc::new(SpyProgress::default());
        let ctx = ctx.with_progress(progress.clone());
        let run = tokio::spawn(async move {
            tool.execute(
                serde_json::json!({
                    "agent_type": "explorer",
                    "prompt": "Find where the server's port is configured."
                }),
                &ctx,
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(matches!(
            progress.steps.lock().await.as_slice(),
            [DelegateStep::Queued { .. }]
        ));
        drop(held);
        run.await.unwrap().unwrap();
        assert!(matches!(
            progress.steps.lock().await.get(1),
            Some(DelegateStep::Started { .. })
        ));
    }

    #[tokio::test]
    async fn a_blocked_report_reaches_the_card_with_who_and_what() {
        let (tool, ctx, _dir) = fixture(vec![finish(
            "f1",
            serde_json::json!({
                "disposition": "blocked",
                "summary": "Two config files disagree about the port.",
                "waiting_on": "user",
                "needs": "say which of config.toml and local.toml is canonical"
            }),
        )]);
        let progress = Arc::new(SpyProgress::default());
        let ctx = ctx.with_progress(progress.clone());
        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "explorer",
                    "prompt": "Work out which port the server listens on."
                }),
                &ctx,
            )
            .await
            .unwrap()
            .to_text()
            .into_owned();

        assert!(
            out.starts_with("Status: blocked. It needs the user to: say which"),
            "{out}"
        );
        let reports = progress.reports.lock().await;
        assert_eq!(reports.len(), 1, "one report, sent once");
        assert_eq!(reports[0].disposition, Disposition::Blocked);
        assert_eq!(reports[0].owner, Some(taurus_tools::Owner::User));
    }

    #[tokio::test]
    async fn the_files_listed_are_the_ones_the_delegate_changed() {
        let (tool, ctx, dir) = fixture(vec![
            ScriptedTurn::tool_calls(vec![
                (
                    "w1",
                    "write_file",
                    serde_json::json!({ "path": "old.txt", "content": "b\n" }),
                ),
                (
                    "w2",
                    "write_file",
                    serde_json::json!({ "path": "new.txt", "content": "c\n" }),
                ),
            ]),
            finish(
                "f1",
                serde_json::json!({ "disposition": "done", "summary": "Wrote both." }),
            ),
        ]);
        let tool = tool.with_defaults(AgentConfig {
            verify_changes: false,
            ..Default::default()
        });
        std::fs::write(dir.path().join("old.txt"), "a\n").unwrap();
        std::fs::write(dir.path().join("parents.txt"), "a\n").unwrap();
        let logs = TempDir::new().unwrap();
        let (ctx, recorder) = checkpointed(ctx, &logs);
        // The parent touched both earlier in this turn. The recorder keeps one
        // pre-image per file per turn, so it won't record `old.txt` again —
        // which is exactly why it can't be what a report is read from.
        recorder.capture(&ctx.workspace.join("old.txt")).await;
        recorder.capture(&ctx.workspace.join("parents.txt")).await;

        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "worker",
                    "prompt": "Write old.txt and new.txt with the contents given."
                }),
                &ctx,
            )
            .await
            .unwrap()
            .to_text()
            .into_owned();
        assert!(out.contains("Files changed: new.txt, old.txt"), "{out}");
        assert!(
            !out.contains("parents.txt"),
            "the parent's own edit is not the child's"
        );
    }

    #[tokio::test]
    async fn a_delegate_that_reports_done_on_unchecked_work_is_asked_to_check_it() {
        let (tool, provider, ctx, _dir) = fixture_with(
            vec![
                write("w1", "a.txt"),
                finish(
                    "f1",
                    serde_json::json!({ "disposition": "done", "summary": "Wrote it." }),
                ),
                finish(
                    "f2",
                    serde_json::json!({
                        "disposition": "done",
                        "summary": "Wrote it. There's nothing to run against a text file."
                    }),
                ),
            ],
            None,
        );
        let logs = TempDir::new().unwrap();
        let (ctx, _recorder) = checkpointed(ctx, &logs);
        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "worker",
                    "prompt": "Write a.txt with the word new in it."
                }),
                &ctx,
            )
            .await
            .unwrap()
            .to_text()
            .into_owned();
        assert_eq!(
            provider.request_count().await,
            3,
            "write, report, nudge, report"
        );
        assert!(
            out.contains("nothing to run against"),
            "the later report wins: {out}"
        );
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
    async fn a_delegate_runs_with_the_parents_config() {
        // Built on `Default::default()`, a child threw the parent's config
        // away: content capture, output caps and the retry settings never
        // reached a delegate. What it sends is what it was configured with.
        let (tool, provider, ctx, _dir) =
            fixture_with(vec![ScriptedTurn::text("Three files.")], None);
        let tool = tool.with_defaults(AgentConfig {
            temperature: Some(0.25),
            max_tokens: Some(1_234),
            ..Default::default()
        });

        tool.execute(
            serde_json::json!({
                "agent_type": "explorer",
                "prompt": "Count the files in this directory and report back."
            }),
            &ctx,
        )
        .await
        .unwrap();

        let sent = provider.last_request().await.expect("the child asked");
        assert_eq!(sent.temperature, Some(0.25));
        assert_eq!(sent.max_tokens, Some(1_234));
    }

    #[tokio::test]
    async fn a_delegates_conversation_is_kept_not_dropped_with_the_tool_result() {
        let (tool, ctx, _dir) = fixture(vec![
            ScriptedTurn::tool_call("t1", "list_dir", serde_json::json!({"path": "."})),
            finish(
                "f1",
                serde_json::json!({ "disposition": "done", "summary": "There are three files." }),
            ),
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
        assert!(out.to_text().contains("There are three files."));

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
        assert!(out.to_text().contains("stopped early"), "{out}");
        assert!(out.to_text().starts_with("Status: failed."), "{out}");

        let kept = spy.kept.lock().await.clone();
        assert!(!kept.is_empty(), "a turn that failed is still a transcript");
        assert!(kept.iter().any(|m| m.has_tool_use()));
    }

    #[tokio::test]
    async fn a_delegate_that_ignores_the_ask_to_report_is_passed_on_as_unreported() {
        // Asked once to call `finish`, and it answers in prose again: what it
        // said the second time goes along, labeled as what it is.
        let (tool, ctx, _dir) = fixture(vec![
            ScriptedTurn::text("I think it's 41."),
            ScriptedTurn::text("The answer is 42."),
        ]);
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
        assert!(out.to_text().contains("The answer is 42."));
        // It never called `finish`, and the parent is told so rather than
        // handed prose as though it were a report.
        assert!(out.to_text().starts_with("Status: unreported."));
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
        assert!(out.to_text().contains("[sub-agent used: list_dir]"));
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
            finish(
                "f1",
                serde_json::json!({ "disposition": "failed", "summary": "I could not delegate." }),
            ),
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
        assert!(out.to_text().contains("I could not delegate."));
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
        // What it named, and the one tool every delegate has to report with.
        assert_eq!(
            offered,
            vec![FINISH_TOOL.to_string(), "read_file".to_string()]
        );
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

        let spent = Arc::new(std::sync::Mutex::new(taurus_provider::Spend::default()));
        let ctx = ctx.with_spent(spent.clone());
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
        let spent = spent.lock().unwrap().clone();
        let models: Vec<&str> = spent.0.iter().map(|s| s.model.as_str()).collect();
        assert_eq!(models, ["qwen3:32b"], "{spent:?}");
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

    /// A hook for `on` that appends `word` to `log`.
    #[cfg(unix)]
    fn logging_hook(
        dir: &std::path::Path,
        log: &std::path::Path,
        word: &str,
        on: taurus_hooks::HookEvent,
    ) -> (String, taurus_hooks::Hook) {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join(format!("{word}.sh"));
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho {word} >> '{}'\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (
            word.to_string(),
            taurus_hooks::Hook {
                on,
                command: script.display().to_string(),
                args: vec![],
                matches: None,
                timeout_seconds: 5,
                foreign: None,
                disabled: false,
            },
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_delegate_meets_the_tool_hooks_but_not_the_turn_hooks() {
        use taurus_hooks::HookEvent;
        let (tool, ctx, _dir) = fixture(vec![
            ScriptedTurn::tool_call("t1", "list_dir", serde_json::json!({})),
            ScriptedTurn::text("Nothing in it."),
        ]);
        let scripts = TempDir::new().unwrap();
        let log = scripts.path().join("fired.log");
        let runner = taurus_hooks::HookRunner::new(vec![
            logging_hook(scripts.path(), &log, "pre", HookEvent::PreToolUse),
            logging_hook(scripts.path(), &log, "prompt", HookEvent::UserPromptSubmit),
            logging_hook(scripts.path(), &log, "stop", HookEvent::Stop),
        ]);
        let ctx = ctx.with_hooks(Arc::new(runner));

        tool.execute(
            serde_json::json!({
                "agent_type": "explorer",
                "prompt": "List the workspace and say what is in it."
            }),
            &ctx,
        )
        .await
        .unwrap();

        let fired = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            fired.contains("pre"),
            "a guard must still see the child's calls"
        );
        assert!(
            !fired.contains("prompt") && !fired.contains("stop"),
            "a delegation is one call inside the conversation's turn, not a turn: {fired:?}"
        );
    }

    /// A parent agent whose only way to delegate is a spawn tool on a provider
    /// of its own, so the parent's and the child's scripts can't interleave.
    fn parent(
        parent_turns: Vec<ScriptedTurn>,
        child_turns: Vec<ScriptedTurn>,
        config: AgentConfig,
    ) -> (Agent, TempDir) {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let permissions = Arc::new(PermissionEngine::new(
            &workspace,
            workspace.join(".taurus"),
            Box::new(AllowAll),
        ));
        let ctx = ToolContext::new(workspace, permissions, CancellationToken::new());
        let spawn = SpawnSubagent::new(
            FakeProvider::new(child_turns),
            Arc::new(RwLock::new(ToolRegistry::with_builtins())),
            "fake",
            2,
        );
        let mut registry = ToolRegistry::with_builtins();
        registry.register(Arc::new(spawn));
        (
            Agent::new(FakeProvider::new(parent_turns), registry, ctx, config),
            dir,
        )
    }

    fn in_background(agent_type: &str) -> ScriptedTurn {
        ScriptedTurn::tool_call(
            "bg1",
            SPAWN_TOOL,
            serde_json::json!({
                "agent_type": agent_type,
                "prompt": "Find which file defines the parser and report it.",
                "background": true
            }),
        )
    }

    async fn drive(agent: &Agent, session: &mut Session) -> Result<crate::TurnOutcome, AgentError> {
        let (tx, mut rx) = mpsc::channel(256);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let outcome = agent.run_turn(session, Message::user("Go."), tx).await;
        drain.await.unwrap();
        outcome
    }

    #[tokio::test]
    async fn a_turn_waits_for_its_background_delegate_and_reads_the_report() {
        let (agent, _dir) = parent(
            vec![
                in_background(builtin::EXPLORER),
                ScriptedTurn::text("Started it; waiting."),
                ScriptedTurn::text("It's in src/parse.rs."),
            ],
            vec![
                // Slow enough that the parent tries to finish first.
                ScriptedTurn::rate_limited(std::time::Duration::from_millis(300)),
                finish(
                    "f1",
                    serde_json::json!({ "disposition": "done", "summary": "src/parse.rs" }),
                ),
            ],
            AgentConfig::default(),
        );
        let mut session = Session::new("fake");
        drive(&agent, &mut session).await.unwrap();

        let texts: Vec<String> = session.messages.iter().map(|m| m.text()).collect();
        let at = texts
            .iter()
            .position(|t| t.contains("<background-report call=\"bg1\" agent=\"explorer\">"))
            .expect("the report reaches the parent");
        assert!(texts[at].contains("Status: done."), "{}", texts[at]);
        assert_eq!(
            texts
                .iter()
                .filter(|t| t.contains("<background-report"))
                .count(),
            1,
            "delivered exactly once"
        );
        assert_eq!(
            texts.last().unwrap(),
            "It's in src/parse.rs.",
            "the turn ends on an answer written after the report"
        );
        let started = session
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .find_map(|block| match block {
                taurus_provider::ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } if tool_use_id == "bg1" => Some(content.to_text().into_owned()),
                _ => None,
            })
            .expect("the call has a result");
        assert!(
            started.contains("Started explorer in the background"),
            "{started}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_report_that_lands_mid_turn_rides_with_the_next_results() {
        // Not a message of its own: that would be two user messages in a row.
        let (agent, _dir) = parent(
            vec![
                in_background(builtin::EXPLORER),
                ScriptedTurn::tool_call(
                    "r1",
                    "run_command",
                    serde_json::json!({ "command": "sleep 0.5" }),
                ),
                ScriptedTurn::text("It's in src/parse.rs."),
            ],
            vec![finish(
                "f1",
                serde_json::json!({ "disposition": "done", "summary": "src/parse.rs" }),
            )],
            AgentConfig {
                verify_changes: false,
                ..Default::default()
            },
        );
        let mut session = Session::new("fake");
        drive(&agent, &mut session).await.unwrap();

        let carrier = session
            .messages
            .iter()
            .find(|m| m.text().contains("<background-report"))
            .expect("the report reaches the parent");
        assert!(
            carrier.content.iter().any(|b| matches!(
                b,
                taurus_provider::ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "r1"
            )),
            "it shares the message that answers the next round's call"
        );
        for pair in session.messages.windows(2) {
            assert!(
                !(pair[0].role == taurus_provider::Role::User
                    && pair[1].role == taurus_provider::Role::User),
                "two user messages in a row"
            );
        }
    }

    #[tokio::test]
    async fn only_an_agent_that_cannot_write_runs_in_the_background() {
        let (tool, ctx, _dir) = fixture(vec![]);
        let err = tool
            .execute(
                serde_json::json!({
                    "agent_type": "coder",
                    "prompt": "Add a test for the retry path and run it.",
                    "background": true
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
        assert!(
            err.to_string().contains("needs a copy of the workspace"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn background_is_refused_where_nothing_waits_for_the_report() {
        let (tool, ctx, _dir) = fixture(vec![]);
        let err = tool
            .execute(
                serde_json::json!({
                    "agent_type": "explorer",
                    "prompt": "Find which file defines the parser.",
                    "background": true
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("nothing would be waiting"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_turn_that_fails_stops_its_background_work_rather_than_waiting_for_it() {
        let (agent, _dir) = parent(
            vec![in_background(builtin::EXPLORER)],
            // A child that would take a minute to get anywhere.
            vec![ScriptedTurn::rate_limited(std::time::Duration::from_secs(
                60,
            ))],
            AgentConfig {
                max_iterations: 1,
                ..Default::default()
            },
        );
        let mut session = Session::new("fake");
        let started = std::time::Instant::now();
        let outcome = drive(&agent, &mut session).await;
        assert!(
            matches!(outcome, Err(AgentError::IterationLimit(1))),
            "{outcome:?}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}: the turn waited for work it should have stopped",
            started.elapsed()
        );
    }

    /// Copies of the workspace, standing in for the host's git worktrees: a
    /// folder per run with the workspace's top-level files copied in, and on
    /// close, the files whose contents differ from the original's.
    #[derive(Default)]
    struct Copies {
        refuse: Option<String>,
        open: std::sync::Mutex<HashMap<String, (std::path::PathBuf, TempDir)>>,
        closed: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Isolation for Copies {
        async fn open(
            &self,
            workspace: &std::path::Path,
            _who: &Requester,
        ) -> Result<Opened, String> {
            if let Some(why) = &self.refuse {
                return Err(why.clone());
            }
            let dir = TempDir::new().unwrap();
            let root = dir.path().canonicalize().unwrap();
            for entry in std::fs::read_dir(workspace).unwrap().flatten() {
                if entry.file_type().unwrap().is_file() {
                    std::fs::copy(entry.path(), root.join(entry.file_name())).unwrap();
                }
            }
            let mut open = self.open.lock().unwrap();
            let run = format!("{:012x}", open.len() + 1);
            open.insert(run.clone(), (workspace.to_path_buf(), dir));
            Ok(Opened {
                run,
                scratch: root.join(".records"),
                root,
                base: "base000000".into(),
                note: None,
            })
        }

        async fn close(&self, run: &str) -> Result<taurus_tools::Closed, String> {
            self.closed.lock().unwrap().push(run.to_string());
            let open = self.open.lock().unwrap();
            let (workspace, dir) = &open[run];
            let mut files = Vec::new();
            for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !entry.file_type().unwrap().is_file() {
                    continue;
                }
                let now = std::fs::read(entry.path()).unwrap();
                if std::fs::read(workspace.join(&name)).ok() != Some(now) {
                    files.push(name);
                }
            }
            files.sort();
            Ok(taurus_tools::Closed {
                kept: (!files.is_empty()).then(|| taurus_tools::Kept {
                    run: run.to_string(),
                    base: "base000000".into(),
                    files,
                    outside: vec![],
                }),
                left_behind: None,
            })
        }

        async fn apply(
            &self,
            run: &str,
            _session: Option<&str>,
            _recorder: Option<&taurus_tools::TurnRecorder>,
            _dry_run: bool,
        ) -> Result<taurus_tools::ApplyPlan, String> {
            Err(format!("{run}: the copies in this test can't be applied"))
        }
    }

    fn isolated(asks: &str) -> serde_json::Value {
        serde_json::json!({
            "agent_type": "coder",
            "prompt": "Change a.txt to say what the test needs it to say.",
            asks: true,
        })
    }

    fn no_verify(tool: SpawnSubagent) -> SpawnSubagent {
        tool.with_defaults(AgentConfig {
            verify_changes: false,
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn a_writer_given_a_copy_works_in_it_and_leaves_the_workspace_alone() {
        let (tool, provider, ctx, dir) = fixture_with(
            vec![
                write("w1", "a.txt"),
                finish(
                    "f1",
                    serde_json::json!({ "disposition": "done", "summary": "Changed a.txt." }),
                ),
            ],
            None,
        );
        std::fs::write(dir.path().join("a.txt"), "old\n").unwrap();
        let copies = Arc::new(Copies::default());
        let tool = no_verify(tool).with_isolation(copies.clone());
        let progress = Arc::new(SpyProgress::default());
        let mut ctx = ctx;
        ctx.progress = Some(progress.clone());

        let out = tool
            .execute(isolated("isolate"), &ctx)
            .await
            .unwrap()
            .to_text()
            .into_owned();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "old\n"
        );
        assert!(
            out.contains("Kept as run 000000000001, started from base000000."),
            "{out}"
        );
        assert!(
            out.ends_with("Files changed: a.txt") || out.contains("Files changed: a.txt\n"),
            "{out}"
        );
        assert_eq!(*copies.closed.lock().unwrap(), ["000000000001"]);
        let report = progress.reports.lock().await[0].clone();
        assert_eq!(report.kept.unwrap().files, ["a.txt"]);
        // Told where it is, and that it's a copy.
        let system = provider.last_request().await.unwrap().system.unwrap();
        assert!(
            system.contains("It is a copy of the workspace made for you"),
            "{system}"
        );
        assert!(
            !system.contains(&format!("`{}`", ctx.workspace.display())),
            "{system}"
        );
    }

    #[test]
    fn a_writer_that_asks_for_a_copy_shares_its_round_only_where_one_can_be_made() {
        let (tool, _ctx, _dir) = fixture(vec![]);
        assert!(!tool.runs_concurrently(&isolated("isolate")));
        let tool = tool.with_isolation(Arc::new(Copies::default()));
        assert!(tool.runs_concurrently(&isolated("isolate")));
        assert!(tool.runs_concurrently(&isolated("background")));
        assert!(alone(&tool, builtin::CODER), "not without asking");
    }

    #[tokio::test]
    async fn a_copy_that_cannot_be_made_leaves_the_writer_in_the_workspace_saying_why() {
        let (tool, ctx, dir) = fixture(vec![
            write("w1", "a.txt"),
            finish(
                "f1",
                serde_json::json!({ "disposition": "done", "summary": "Changed a.txt." }),
            ),
        ]);
        let tool = no_verify(tool).with_isolation(Arc::new(Copies {
            refuse: Some("This repository uses Git LFS.".into()),
            ..Default::default()
        }));
        let out = tool
            .execute(isolated("isolate"), &ctx)
            .await
            .unwrap()
            .to_text()
            .into_owned();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "new\n"
        );
        assert!(out.contains("couldn't have one"), "{out}");
        assert!(out.contains("Git LFS"), "{out}");
        assert!(!out.contains("Kept as run"), "{out}");
    }

    #[tokio::test]
    async fn a_writer_in_the_workspace_waits_for_the_one_already_writing_there() {
        let (tool, ctx, _dir) = fixture(vec![
            write("w1", "a.txt"),
            finish(
                "f1",
                serde_json::json!({ "disposition": "done", "summary": "Changed a.txt." }),
            ),
        ]);
        let tool = Arc::new(no_verify(tool).with_isolation(Arc::new(Copies {
            refuse: Some("No copy today.".into()),
            ..Default::default()
        })));
        // Another writer is working in the workspace.
        let held = tool.alone.clone().lock_owned().await;
        let running = {
            let tool = tool.clone();
            tokio::spawn(async move { tool.execute(isolated("isolate"), &ctx).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!running.is_finished(), "it wrote beside the other writer");
        drop(held);
        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_copy_made_for_a_delegate_that_is_then_refused_is_closed() {
        // Scoped to a tool a copy can't hold it to, so nothing it names is
        // left once the copy is made.
        let (tool, _provider, ctx, _dir) = fixture_with(
            vec![],
            Some(vec![custom(
                "coder",
                "You write code for the parent agent.",
                Some(vec!["remote_write"]),
            )]),
        );
        tool.registry
            .write()
            .await
            .register(Arc::new(Elsewhere("remote_write", Effect::Write)));
        let copies = Arc::new(Copies::default());
        let tool = tool.with_isolation(copies.clone());
        let err = tool.execute(isolated("isolate"), &ctx).await.unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");
        assert_eq!(
            copies.closed.lock().unwrap().len(),
            copies.open.lock().unwrap().len(),
            "a copy was opened and left open"
        );
    }

    #[tokio::test]
    async fn background_work_with_nobody_to_wait_for_it_closes_its_copy() {
        let (tool, ctx, _dir) = fixture(vec![]);
        assert!(ctx.pending.is_none());
        let copies = Arc::new(Copies::default());
        let tool = tool.with_isolation(copies.clone());
        let err = tool
            .execute(isolated("background"), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("without `background`"), "{err}");
        assert_eq!(
            copies.closed.lock().unwrap().len(),
            copies.open.lock().unwrap().len(),
            "a copy was opened and left open"
        );
    }

    #[tokio::test]
    async fn in_the_background_a_writer_with_no_copy_is_refused_with_the_reason() {
        let (tool, ctx, _dir) = fixture(vec![]);
        let tool = tool.with_isolation(Arc::new(Copies {
            refuse: Some("This folder isn't in a git repository.".into()),
            ..Default::default()
        }));
        let err = tool
            .execute(isolated("background"), &ctx)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("isn't in a git repository"),
            "{err}"
        );
        assert!(err.to_string().contains("without `background`"), "{err}");
    }

    /// A tool that changes something wherever it was set up to, as an MCP
    /// server's would.
    struct Elsewhere(&'static str, Effect);

    #[async_trait]
    impl Tool for Elsewhere {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "does something outside the context's folder"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        fn effect(&self) -> Effect {
            self.1
        }
        async fn execute(&self, _: serde_json::Value, _: &ToolContext) -> ToolResult {
            Ok("done".into())
        }
    }

    #[tokio::test]
    async fn a_copy_holds_its_delegate_only_through_tools_that_work_where_they_are_told() {
        let (_, provider, ctx, _dir) = fixture_with(
            vec![finish(
                "f1",
                serde_json::json!({ "disposition": "done", "summary": "Nothing to do." }),
            )],
            None,
        );
        let mut shared = ToolRegistry::with_builtins();
        shared.register(Arc::new(Elsewhere("mcp_write", Effect::Write)));
        shared.register(Arc::new(Elsewhere("mcp_lookup", Effect::Read)));
        shared.register(Arc::new(Elsewhere(PARENT_INDEX_TOOL, Effect::Read)));
        let tool = no_verify(SpawnSubagent::new(
            provider.clone(),
            Arc::new(RwLock::new(shared)),
            "fake",
            2,
        ))
        .with_isolation(Arc::new(Copies::default()));

        let out = tool
            .execute(
                serde_json::json!({
                    "agent_type": "worker",
                    "prompt": "Look around the workspace and report what is there.",
                    "isolate": true
                }),
                &ctx,
            )
            .await
            .unwrap()
            .to_text()
            .into_owned();

        let offered: Vec<String> = provider
            .last_request()
            .await
            .unwrap()
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect();
        assert!(offered.contains(&"write_file".to_string()), "{offered:?}");
        assert!(offered.contains(&"mcp_lookup".to_string()), "{offered:?}");
        assert!(!offered.contains(&"mcp_write".to_string()), "{offered:?}");
        assert!(
            !offered.contains(&PARENT_INDEX_TOOL.to_string()),
            "{offered:?}"
        );
        assert!(
            out.contains("It had no mcp_write, search_code in its copy"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn a_background_writer_in_a_copy_reports_without_touching_the_workspace() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::write(workspace.join("a.txt"), "old\n").unwrap();
        let permissions = Arc::new(PermissionEngine::new(
            &workspace,
            workspace.join(".taurus"),
            Box::new(AllowAll),
        ));
        let ctx = ToolContext::new(workspace.clone(), permissions, CancellationToken::new());
        let spawn = no_verify(SpawnSubagent::new(
            FakeProvider::new(vec![
                write("w1", "a.txt"),
                finish(
                    "f1",
                    serde_json::json!({ "disposition": "done", "summary": "Changed a.txt." }),
                ),
            ]),
            Arc::new(RwLock::new(ToolRegistry::with_builtins())),
            "fake",
            2,
        ))
        .with_isolation(Arc::new(Copies::default()));
        let mut registry = ToolRegistry::with_builtins();
        registry.register(Arc::new(spawn));
        let agent = Agent::new(
            FakeProvider::new(vec![
                ScriptedTurn::tool_call(
                    "bg1",
                    SPAWN_TOOL,
                    serde_json::json!({
                        "agent_type": "coder",
                        "prompt": "Change a.txt to say what the test needs it to say.",
                        "background": true
                    }),
                ),
                ScriptedTurn::text("Started it; waiting."),
                ScriptedTurn::text("It's kept for you to apply."),
            ]),
            registry,
            ctx,
            AgentConfig::default(),
        );
        let mut session = Session::new("fake");
        drive(&agent, &mut session).await.unwrap();

        let texts: Vec<String> = session.messages.iter().map(|m| m.text()).collect();
        let report = texts
            .iter()
            .find(|t| t.contains("<background-report call=\"bg1\" agent=\"coder\">"))
            .expect("the report reaches the parent");
        assert!(report.contains("Kept as run"), "{report}");
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "old\n"
        );
    }

    fn alone(tool: &SpawnSubagent, agent_type: &str) -> bool {
        !tool.runs_concurrently(&serde_json::json!({
            "agent_type": agent_type,
            "prompt": "Whatever the task is, it is long enough."
        }))
    }

    #[test]
    fn only_a_delegate_that_cannot_write_shares_its_round() {
        let (tool, _ctx, _dir) = fixture(vec![]);
        assert!(!alone(&tool, builtin::EXPLORER), "explorer only reads");
        assert!(alone(&tool, builtin::CODER), "coder writes");
        // No `tools:` key: it inherits the parent's writers.
        assert!(alone(&tool, builtin::WORKER), "worker inherits writers");
        assert!(alone(&tool, "wizard"), "an unknown agent runs alone");
        assert!(alone(&tool, ""), "so does a call with no agent named");
    }

    #[test]
    fn a_custom_agent_is_judged_by_the_tools_it_names() {
        let (tool, _, _ctx, _dir) = fixture_with(
            vec![],
            Some(vec![
                custom("reader", "Read.", Some(vec!["read_file", "grep"])),
                custom("editor", "Edit.", Some(vec!["read_file", "edit_file"])),
                custom("everything", "Do it.", None),
            ]),
        );
        assert!(!alone(&tool, "reader"));
        assert!(alone(&tool, "editor"));
        assert!(alone(&tool, "everything"));
    }
}
