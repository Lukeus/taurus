//! Assembly of a running harness, independent of how it is driven.
//!
//! Both frontends — the desktop app and the CLI — need the same things: a
//! workspace, a permission engine bound to it, a tool registry carrying
//! built-ins plus skills plus MCP tools, and an [`Agent`] configured with the
//! system prompt those imply. Building that twice is how the two would drift,
//! so it is built once here and the frontends supply only what genuinely
//! differs: how to ask the user for permission, and where skill proposals go.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;

use taurus_agents::catalog::{AgentCatalog, AgentSource, SharedAgentCatalog};
use taurus_agents::proposal::AgentProposalSink;
use taurus_agents::{AgentDefinition, AgentSummary, AgentTier};
use taurus_core::{Agent, AgentConfig, AgentModel, ModelOverrides, ProposeAgent, SpawnSubagent};
use taurus_mcp::{McpManager, ServerStatus};
use taurus_provider::Provider;
use taurus_provider_anthropic::{
    AnthropicCapabilities, AnthropicProvider, Thinking as AnthropicThinking,
};
use taurus_provider_gemini::{GeminiCapabilities, GeminiProvider};
use taurus_provider_ollama::OllamaProvider;
use taurus_provider_openai::{ModelSpec, OpenAiCapabilities, OpenAiProvider};
use taurus_skills::catalog::SkillCatalog;
use taurus_skills::proposal::ProposalSink;
use taurus_skills::skill::SkillSummary;
use taurus_skills::SharedCatalog;
use taurus_tools::builtin::plan::UpdatePlan;
use taurus_tools::builtin::present::{AskUser, ShowChart, ShowSequence, ShowTable};
use taurus_tools::PlanBoard;
use taurus_tools::{
    Asker, CheckpointStore, PermissionEngine, PermissionPrompt, ToolContext, ToolRegistry,
};

use crate::command;
use crate::config::{self, ProviderConfig, ProviderKind, Scope, Settings, Theme};
use crate::instructions::{self, Instructions};
use crate::mcp_view::{LayerOf, McpServerView};
use crate::problem::{self, Problem, ProblemSource};
use crate::prompt;
use crate::secrets;

/// How many sub-agents may run at once. Low on purpose: each is a full model
/// stream, and local hardware serves them all from the same GPU.
pub const MAX_CONCURRENT_SUBAGENTS: usize = 2;

/// How many characters of agent roster are worth carrying before it is worth
/// saying so.
///
/// The roster sits in the spawn tool's description, so every line is paid on
/// every request of every turn — the same argument `disabled_tools` makes about
/// tool schemas. Silent expense is the failure mode worth engineering against;
/// a visible one the user chose is fine, so passing this reports a problem
/// rather than dropping anything. Roughly a dozen agents at the 200-character
/// description cap, which is well past what a person curates by hand.
pub const ROSTER_BUDGET_CHARS: usize = 2_400;

/// Tools a parent turn registers for itself, rather than into the shared
/// registry every reload rebuilds.
///
/// All four are here for the same reason: a sub-agent must not have them.
/// `spawn_subagent` is the depth cap, and the other three speak to the person
/// watching this conversation, which a delegate does not have.
///
/// Named as a set because `disabled_tools` has to know about them twice over —
/// once to take one away in [`Host::build_agent`], and once so
/// [`Host::reload`], which is looking at a registry that does not contain them,
/// does not report a working name as a typo.
pub const PER_TURN_TOOLS: &[&str] = &[
    taurus_core::SPAWN_TOOL,
    taurus_tools::builtin::present::SHOW_TABLE_TOOL,
    taurus_tools::builtin::present::SHOW_CHART_TOOL,
    taurus_tools::builtin::present::SHOW_SEQUENCE_TOOL,
    taurus_tools::builtin::present::ASK_USER_TOOL,
    taurus_tools::builtin::plan::UPDATE_PLAN_TOOL,
];

/// Makes a permission prompt on demand.
///
/// A factory rather than a single instance because the permission engine is
/// rebuilt whenever the workspace changes, and each engine owns its prompt.
pub trait PermissionPromptFactory: Send + Sync {
    fn create(&self) -> Box<dyn PermissionPrompt>;
}

/// Names the turn about to run, so what it changes can be undone.
///
/// Passed to [`Host::build_agent`] rather than read out of the session inside
/// it, because the desktop app builds its agent before it takes the session
/// lock. Both frontends hold these two strings at that point either way.
pub struct TurnRef<'a> {
    pub session_id: &'a str,
    /// What the user asked for. Labels the checkpoint in a listing.
    pub prompt: &'a str,
}

pub struct Host {
    workspace: RwLock<PathBuf>,
    providers: RwLock<Vec<ProviderConfig>>,
    settings: RwLock<Settings>,
    catalog: SharedCatalog,
    /// The standing brief for this machine and this workspace, re-read on every
    /// reload. Held rather than read per turn for the reason the skill catalog
    /// is: this is six `stat`s and a handful of file reads, and a turn is not
    /// the place to pay for them again.
    instructions: RwLock<Vec<Instructions>>,
    /// The sub-agent roster. Seeded with the built-ins so `explorer` and
    /// `worker` work before anything has been scanned.
    ///
    /// Shared rather than owned so `propose_agent` can check a proposed name
    /// against the roster as it stands now. A turn delegates against a frozen
    /// snapshot; a duplicate check has to see the live set.
    agents: SharedAgentCatalog,
    /// Each agent's `(provider, model)`, resolved once per reload. Resolving it
    /// here rather than per turn keeps a keychain read off the hot path.
    agent_models: RwLock<ModelOverrides>,
    /// Shared rather than owned so sub-agents can be handed the same registry:
    /// it has no spawn tool, which is what caps delegation depth.
    registry: Arc<RwLock<ToolRegistry>>,
    permissions: RwLock<Arc<PermissionEngine>>,
    mcp: McpManager,
    problems: RwLock<Vec<Problem>>,
    prompts: Arc<dyn PermissionPromptFactory>,
    /// Where `ask_user` puts its questions. Not a factory like `prompts`: it is
    /// bound to nothing that a workspace change rebuilds.
    asker: Arc<dyn Asker>,
    proposals: Arc<dyn ProposalSink>,
    /// Where a proposed *agent* goes. Separate from `proposals` because the two
    /// carry different payloads and land on different review cards, not because
    /// a frontend would ever want one and not the other.
    agent_proposals: Arc<dyn AgentProposalSink>,
    /// One checklist per conversation, so an unfinished plan survives the
    /// message that interrupted it. Keyed by session id and dropped with the
    /// session — see [`Host::forget_plan`].
    ///
    /// Held here rather than in the session log because a plan is working state,
    /// not a record: it is rebuilt by the model from the transcript if this
    /// process restarts, and writing it to disk would create a second copy that
    /// could disagree with the tool calls that made it.
    plans: RwLock<std::collections::HashMap<String, PlanBoard>>,
}

impl Host {
    pub fn new(
        workspace: PathBuf,
        prompts: Arc<dyn PermissionPromptFactory>,
        asker: Arc<dyn Asker>,
        proposals: Arc<dyn ProposalSink>,
        agent_proposals: Arc<dyn AgentProposalSink>,
    ) -> Self {
        let permissions = Arc::new(PermissionEngine::new(
            &workspace,
            config::home_dir(),
            prompts.create(),
        ));
        // Both layers are read here and again on every `reload`, because the
        // workspace layer changes underneath a running host.
        let (providers, _) = config::load_providers(Some(&workspace));
        let settings = config::load_settings(Some(&workspace));
        Self {
            providers: RwLock::new(providers),
            settings: RwLock::new(settings),
            workspace: RwLock::new(workspace),
            catalog: Arc::new(RwLock::new(SkillCatalog::default())),
            instructions: RwLock::new(Vec::new()),
            agents: Arc::new(RwLock::new(AgentCatalog::default())),
            agent_models: RwLock::new(ModelOverrides::new()),
            registry: Arc::new(RwLock::new(ToolRegistry::with_builtins())),
            permissions: RwLock::new(permissions),
            mcp: McpManager::new(),
            problems: RwLock::new(Vec::new()),
            prompts,
            asker,
            proposals,
            agent_proposals,
            plans: RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// The workspace remembered from last time, else the current directory.
    ///
    /// Global layer only: there is no workspace yet to read a second layer
    /// from, which is exactly why `last_workspace` is written globally.
    pub fn default_workspace() -> PathBuf {
        let candidate = config::read_settings(Scope::Global, None)
            .last_workspace
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        candidate.canonicalize().unwrap_or(candidate)
    }

    /// Re-resolves both config layers, rescans skills, reconnects MCP servers,
    /// and rebuilds the registry.
    ///
    /// Every layered file is re-read here rather than only at startup: the
    /// workspace layer belongs to a directory the user can change at any time,
    /// so "which providers exist" is not a fact that survives a workspace
    /// switch.
    pub async fn reload(&self) {
        let workspace = self.workspace.read().await.clone();

        let (providers, provider_problems) = config::load_providers(Some(&workspace));
        let mut problems = Problem::tag(ProblemSource::Providers, provider_problems);
        *self.providers.write().await = providers;
        *self.settings.write().await = config::load_settings(Some(&workspace));

        let sources = config::skill_sources(Some(&workspace));
        let (catalog, skill_problems) = SkillCatalog::discover(&sources);
        info!(
            skills = catalog.len(),
            problems = skill_problems.len(),
            "skills loaded"
        );
        problems.extend(Problem::tag(
            ProblemSource::Skills,
            skill_problems.iter().map(|p| p.to_string()),
        ));
        *self.catalog.write().await = catalog;

        // Re-read on every reload for the reason providers are: these files
        // belong to the workspace, and a switch changes which of them exist.
        let (loaded, instruction_problems) =
            instructions::load(instructions::sources(Some(&workspace)));
        info!(files = loaded.len(), "instructions loaded");
        problems.extend(Problem::tag(
            ProblemSource::Instructions,
            instruction_problems,
        ));
        *self.instructions.write().await = loaded;

        let mut registry = ToolRegistry::with_builtins();
        registry.register(Arc::new(taurus_skills::LoadSkill::new(
            self.catalog.clone(),
        )));
        registry.register(Arc::new(taurus_skills::RunSkillScript::new(
            self.catalog.clone(),
        )));
        // Only when the setting is on. The prompt's authoring guidance already
        // follows this setting, and advertising the tool without it left the
        // model holding a schema — one of the largest here — that nothing told
        // it when to use. Same rule the web tools follow: never offer a tool the
        // prompt cannot explain.
        if self.settings.read().await.skill_synthesis_enabled {
            registry.register(Arc::new(taurus_skills::ProposeSkill::new(
                self.catalog.clone(),
                self.proposals.clone(),
            )));
        }
        if self.settings.read().await.agent_synthesis_enabled {
            registry.register(Arc::new(ProposeAgent::new(
                self.agents.clone(),
                self.registry.clone(),
                self.agent_proposals.clone(),
            )));
        }

        // Both web tools stand or fall together: a `fetch_url` with no way to
        // find a URL is a tool the model can only use on links the user pastes,
        // and registering search without fetch leaves it holding snippets it
        // cannot follow. Neither appears until a backend resolves, so the model
        // is never offered a search it has no key for.
        let (search_backend, search_problems) = config::load_search(Some(&workspace));
        problems.extend(Problem::tag(ProblemSource::Search, search_problems));
        if let Some(backend) = search_backend {
            info!(
                backend = %backend.id,
                allow_private_hosts = backend.allow_private_hosts,
                "web search enabled"
            );
            registry.register(Arc::new(taurus_web::FetchUrl::new(
                backend.allow_private_hosts,
            )));
            registry.register(Arc::new(taurus_web::WebSearch::new(backend)));
        }

        // Semantic search, when an embedding model is named. Off by default and
        // on the same rule the web tools follow: a tool the model can see is a
        // tool it will try, and one with no embedding model pulled costs it a
        // turn to find that out.
        //
        // Registered into the shared registry rather than per turn, unlike the
        // tools that address the person watching. A delegate sent to explore an
        // unfamiliar codebase is exactly who needs this most, and the per-turn
        // set is the one sub-agents do not get.
        let embedding_model = self
            .settings
            .read()
            .await
            .embedding_model
            .trim()
            .to_string();
        if !embedding_model.is_empty() {
            // The provider the conversation is on. An embedding model lives on
            // the same server as the chat model in every local setup, and a
            // second provider entry naming the same machine would be one more
            // thing to keep in step.
            let id = self.embedding_provider_id().await;
            match id {
                Some(id) => match self.provider(&id).await {
                    Ok(provider) => {
                        info!(model = %embedding_model, provider = %id, "semantic search enabled");
                        registry.register(Arc::new(taurus_index::SearchCode::new(
                            provider,
                            &embedding_model,
                            taurus_index::index_dir(
                                &config::home_dir(),
                                &crate::sessions::workspace_key(&workspace),
                            ),
                        )));
                    }
                    Err(e) => problems.push(Problem {
                        source: ProblemSource::Providers,
                        message: format!("semantic search is configured but {e}"),
                    }),
                },
                None => problems.push(Problem {
                    source: ProblemSource::Providers,
                    message: "semantic search is configured but no provider is".into(),
                }),
            }
        }

        // Unconditional, unlike everything else registered here. It is how a
        // user with no MCP servers gets their first one, so gating it on having
        // some would take it away from exactly the person who needs it. Safe to
        // offer unconditionally because it does the opposite of what its name
        // suggests to a reader in a hurry: it writes nothing and starts
        // nothing. See `taurus_mcp::draft`.
        registry.register(Arc::new(
            taurus_mcp::DraftMcpServer::new(config::home_dir()),
        ));

        // Reconnecting drops the previous connections, stopping the old child
        // processes; leaving them would leak one per workspace change.
        self.mcp.shutdown().await;
        let mut layers = Vec::new();
        for dir in config::config_dirs(Some(&workspace)) {
            match taurus_mcp::load(&dir) {
                Ok(layer) => layers.push(layer),
                // A layer that will not parse is skipped, not fatal: the other
                // one is still a working set of servers.
                Err(e) => problems.push(Problem::new(ProblemSource::Mcp, e)),
            }
        }
        let (mcp_config, mcp_problems) = config::merge_mcp(layers);
        problems.extend(Problem::tag(ProblemSource::Mcp, mcp_problems));
        for tool in self.mcp.connect_all(&mcp_config).await {
            registry.register(tool);
        }

        // Last, so it applies to everything the harness assembled — built-ins,
        // skill tools, web, MCP — rather than to whichever of them happened to
        // register before the setting was read.
        let disabled = self.settings.read().await.disabled_tools.clone();
        // The per-turn tools are not in this registry to be removed from — a
        // turn adds them to its own copy, and takes them away there. Held back
        // so that naming one is not reported as naming a tool that does not
        // exist, which is the one message that would send someone looking for a
        // typo in a line that works.
        let here: Vec<String> = disabled
            .iter()
            .filter(|name| !PER_TURN_TOOLS.contains(&name.as_str()))
            .cloned()
            .collect();
        problems.extend(Problem::tag(
            ProblemSource::Tools,
            disable(&mut registry, &here),
        ));

        // After the registry is finished, and deliberately so: an agent scoped
        // to tools the user has since disabled is exactly the case this catches.
        let available: Vec<String> = registry.names().map(str::to_string).collect();
        problems.extend(self.load_agents(&workspace, &available).await);

        *self.registry.write().await = registry;
        *self.problems.write().await = problems;
    }

    /// Rescans the agent directories without touching anything else.
    ///
    /// The whole authoring surface for an agent is a text editor, so a drawer
    /// that shows the catalog as it was at startup is not showing the feature
    /// working. This is what opening it calls. It is deliberately narrower than
    /// [`Host::reload`]: rescanning a directory should not restart every MCP
    /// server, which a full reload does.
    pub async fn rescan_agents(&self) {
        let workspace = self.workspace.read().await.clone();
        let available: Vec<String> = self
            .registry
            .read()
            .await
            .names()
            .map(str::to_string)
            .collect();
        let found = self.load_agents(&workspace, &available).await;

        let mut problems = self.problems.write().await;
        problems.retain(|p| p.source != ProblemSource::Agents);
        problems.extend(found);
    }

    /// Discovers the roster, checks it against `available`, resolves its
    /// models, and installs it. Returns what to report.
    async fn load_agents(&self, workspace: &Path, available: &[String]) -> Vec<Problem> {
        let (mut agents, errors) = AgentCatalog::discover(&[
            AgentSource {
                tier: AgentTier::User,
                dir: config::user_agents_dir(),
            },
            AgentSource {
                tier: AgentTier::Project,
                dir: config::workspace_agents_dir(workspace),
            },
        ]);
        info!(
            agents = agents.len(),
            problems = errors.len(),
            "agents loaded"
        );

        let mut problems = Problem::tag(
            ProblemSource::Agents,
            errors.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
        );
        problems.extend(Problem::tag(
            ProblemSource::Agents,
            cross_check_tools(&mut agents, available),
        ));
        self.resolve_agent_models(&mut agents).await;

        if agents.roster_cost() > ROSTER_BUDGET_CHARS {
            problems.push(Problem::new(
                ProblemSource::Agents,
                format!(
                    "the {} sub-agents cost {} characters of every request, over the \
                     {ROSTER_BUDGET_CHARS} this budgets for; shorten their descriptions or remove \
                     the ones you do not use",
                    agents.len(),
                    agents.roster_cost()
                ),
            ));
        }

        *self.agents.write().await = agents;
        problems
    }

    /// Resolves each agent's `model:` and `provider:` into something the spawn
    /// tool can use.
    ///
    /// An unresolvable model degrades rather than failing the load: a repo can
    /// then ship an agent that names a cloud model without breaking for the
    /// contributor who runs Ollama only. Like a skill with a missing
    /// interpreter, that is recorded on the agent rather than raised as a
    /// problem — the drawer row and `taurus agents check` both show the reason,
    /// so the fallback is visible without the status strip claiming something
    /// is broken when nothing is.
    async fn resolve_agent_models(&self, agents: &mut AgentCatalog) {
        let mut resolved = ModelOverrides::new();

        for agent in agents.iter_mut() {
            let Some(model) = agent.frontmatter.model.clone() else {
                continue;
            };
            let Some(provider_id) = agent.frontmatter.provider.clone() else {
                // A model with no provider is a different model on whichever
                // provider the session is using, which is not known until a turn
                // starts. Nothing to resolve, and nothing that can fail here.
                resolved.insert(
                    agent.name().to_string(),
                    AgentModel {
                        provider: None,
                        model,
                    },
                );
                continue;
            };

            match self.provider(&provider_id).await {
                Ok(provider) => {
                    resolved.insert(
                        agent.name().to_string(),
                        AgentModel {
                            provider: Some(provider),
                            model,
                        },
                    );
                }
                Err(e) => degrade(
                    agent,
                    format!(
                        "wants {model} on provider '{provider_id}', which is not usable here \
                         ({e}); it will run on the session's model instead"
                    ),
                ),
            }
        }

        *self.agent_models.write().await = resolved;
    }

    pub async fn set_workspace(&self, path: &Path) -> Result<PathBuf, String> {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !canonical.is_dir() {
            return Err(format!("{} is not a directory", canonical.display()));
        }

        *self.workspace.write().await = canonical.clone();
        *self.permissions.write().await = Arc::new(PermissionEngine::new(
            &canonical,
            config::home_dir(),
            self.prompts.create(),
        ));

        // Global, and only global: "the workspace I had open" is a fact about
        // the user, and writing it into the workspace it names would be a file
        // that can only ever point at its own directory.
        let remembered = canonical.display().to_string();
        config::edit_settings(Scope::Global, None, |s| s.last_workspace = Some(remembered));

        // Reload re-resolves both layers, so the in-memory settings pick up the
        // new workspace's file without a second write.
        self.reload().await;
        Ok(canonical)
    }

    /// Instantiates a provider from its config.
    ///
    /// Built per call rather than cached: they are cheap, hold no session
    /// state, and this way an edited base URL takes effect without a restart.
    pub async fn provider(&self, id: &str) -> Result<Arc<dyn Provider>, String> {
        let config = self
            .provider_config(id)
            .await
            .ok_or_else(|| format!("no provider configured with id '{id}'"))?;

        Ok(match config.kind {
            ProviderKind::Ollama => Arc::new(OllamaProvider::new(config.base_url)),
            ProviderKind::OpenAiCompatible => {
                let defaults = OpenAiCapabilities::default();
                Arc::new(
                    OpenAiProvider::new(
                        config.id.clone(),
                        config.base_url.clone(),
                        config.api_key(),
                        OpenAiCapabilities {
                            native_tools: config.native_tools.unwrap_or(defaults.native_tools),
                            vision: config.vision.unwrap_or(defaults.vision),
                            context_length: config
                                .context_length
                                .unwrap_or(defaults.context_length),
                        },
                    )
                    .with_api_prefix(config.api_prefix.clone())
                    .with_api_key_header(config.api_key_header.clone())
                    .with_models(
                        config
                            .models
                            .iter()
                            .map(|m| ModelSpec {
                                id: m.id.clone(),
                                display_name: m.display_name.clone(),
                                context_length: m.context_length,
                                native_tools: m.native_tools,
                                vision: m.vision,
                            })
                            .collect(),
                    ),
                )
            }

            // Neither of the next two takes `native_tools`: both back model
            // families that call tools natively, and a prompted fallback there
            // would be a worse implementation of something that works. Both
            // take `context_length` only as a fallback — each can ask its own
            // backend, and a configured value that disagrees with the model is
            // how a conversation compacts at the wrong moment.
            ProviderKind::Anthropic => Arc::new(
                AnthropicProvider::new(
                    config.id.clone(),
                    config.base_url.clone(),
                    config.api_key(),
                )
                .with_thinking(
                    config
                        .thinking
                        .as_deref()
                        .map(AnthropicThinking::parse)
                        .unwrap_or_default(),
                )
                .with_fallback_capabilities(AnthropicCapabilities {
                    vision: AnthropicCapabilities::default().vision,
                    context_length: config
                        .context_length
                        .unwrap_or(AnthropicCapabilities::default().context_length),
                })
                .with_models(config.models.iter().map(|m| m.id.clone()).collect()),
            ),

            ProviderKind::Gemini => Arc::new(
                GeminiProvider::new(config.id.clone(), config.base_url.clone(), config.api_key())
                    .with_fallback_capabilities(GeminiCapabilities {
                        vision: GeminiCapabilities::default().vision,
                        context_length: config
                            .context_length
                            .unwrap_or(GeminiCapabilities::default().context_length),
                    })
                    .with_models(config.models.iter().map(|m| m.id.clone()).collect()),
            ),
        })
    }

    /// Builds the agent for one turn.
    ///
    /// The single place system prompt, tool set, sub-agent wiring, and the
    /// turn's checkpoint come together, so the CLI and the desktop app cannot
    /// disagree about how an agent is configured — or about whether a turn is
    /// rewindable.
    pub async fn build_agent(
        &self,
        provider: Arc<dyn Provider>,
        model: &str,
        cancel: CancellationToken,
        turn: TurnRef<'_>,
    ) -> Agent {
        // Bound to this session's provider and model, so it is added per turn
        // rather than living in the shared registry. Children get the shared
        // registry, which has no spawn tool — that is the depth cap.
        let mut registry = self.registry.read().await.clone();
        // The roster is snapshotted here, so a turn sees the set of agents it
        // started with even if a file is saved while it runs.
        registry.register(Arc::new(
            SpawnSubagent::new(
                provider.clone(),
                self.registry.clone(),
                model,
                MAX_CONCURRENT_SUBAGENTS,
            )
            .with_roster(
                Arc::new(self.agents.read().await.to_vec()),
                self.agent_models.read().await.clone(),
            ),
        ));

        // Registered per turn, alongside the spawn tool and for the same
        // reason: these address the person watching this conversation, and a
        // sub-agent has no such person. It shares the registry above, which is
        // what keeps `ask_user` away from a worker that cannot ask anyone
        // anything, and keeps a delegate from drawing a chart into a transcript
        // it is not part of.
        registry.register(Arc::new(ShowTable));
        registry.register(Arc::new(ShowChart));
        registry.register(Arc::new(ShowSequence));
        registry.register(Arc::new(AskUser::new(self.asker.clone())));

        // The *tool* is per turn, like the three above: a delegate writing into
        // the parent's checklist would report progress against a task nobody
        // gave it. The *board* is per conversation, so an unfinished plan
        // survives the message that interrupted it — `start_turn` is what drops
        // a finished one, and is the whole of the staleness rule.
        let plan = self.plan_board(turn.session_id).await;
        plan.start_turn();
        registry.register(Arc::new(UpdatePlan::new(plan.clone())));

        // `reload` applies this to the shared registry, which is everything
        // registered *there* — so without a second pass here, the per-turn
        // tools were the one set `disabled_tools` could not reach, and the
        // guarantee that a disabled tool is not registered at all held for
        // every tool but the four the parent turn adds for itself. Silent, and
        // exactly the direction that matters: a name typed to take a tool away
        // that quietly leaves it on.
        //
        // Unmatched names are not reported from here. `reload` already reports
        // them once, against the full set including these, and a turn is not a
        // place to raise a configuration problem — it would arrive once per
        // message for as long as the typo lived.
        disable(
            &mut registry,
            &self.settings.read().await.disabled_tools.clone(),
        );

        let workspace = self.workspace.read().await.clone();
        let skill_section = self.catalog.read().await.prompt_section();
        let instructions_section = instructions::section(&self.instructions.read().await);
        let synthesis = self.settings.read().await.skill_synthesis_enabled;
        let agent_synthesis = self.settings.read().await.agent_synthesis_enabled;

        // Opened here rather than by the loop, because a sub-agent runs its own
        // loop and must record into the turn that spawned it, not one of its
        // own. Cloning the context is what carries it down.
        let recorder =
            self.checkpoints()
                .await
                .begin_turn(turn.session_id, &workspace, turn.prompt);

        Agent::new(
            provider,
            registry,
            self.tool_context(cancel).await.with_checkpoints(recorder),
            AgentConfig {
                system_prompt: prompt::build(
                    &workspace,
                    skill_section,
                    instructions_section,
                    synthesis,
                    agent_synthesis,
                ),
                // Read per turn rather than captured once, so raising it in
                // Settings applies to the next message instead of the next
                // launch.
                max_iterations: self.settings.read().await.max_iterations,
                ..Default::default()
            },
        )
        // The same board the tool writes to, so what the model wrote on the
        // last iteration is what it reads on the next one.
        .with_plan(plan)
    }

    /// The open workspace's checkpoint logs, for the turn about to record into
    /// them.
    pub async fn checkpoints(&self) -> CheckpointStore {
        CheckpointStore::new(crate::sessions::checkpoints_dir(
            &self.workspace.read().await,
        ))
    }

    /// One named workspace's checkpoint logs.
    ///
    /// What anything *reading* a conversation's history wants, because a
    /// checkpoint log is keyed by the workspace the conversation was held in
    /// rather than by the one open now. Asked against the wrong workspace the
    /// log is simply not there, and the answer is an empty history for a
    /// conversation that rewrote half the project — which is what listing and
    /// rewinding both used to do the moment somebody switched folders.
    pub fn checkpoints_for(&self, workspace: &Path) -> CheckpointStore {
        CheckpointStore::new(crate::sessions::checkpoints_dir(workspace))
    }

    /// Where this workspace stands with git.
    ///
    /// Read on demand rather than cached: a user switches branches in a
    /// terminal beside this window, and a cached answer would be wrong exactly
    /// when it matters — the moment before someone commits a turn.
    pub async fn repo_status(&self) -> crate::git::RepoStatus {
        crate::git::Repo::status(&self.workspace.read().await.clone()).await
    }

    /// The branch this workspace is on, for stamping onto a new conversation.
    pub async fn branch(&self) -> Option<String> {
        self.repo_status().await.branch
    }

    pub async fn tool_context(&self, cancel: CancellationToken) -> ToolContext {
        ToolContext::new(
            self.workspace.read().await.clone(),
            self.permissions.read().await.clone(),
            cancel,
        )
        // Read-only, and only the skills actually loaded. A skill's procedure
        // points at its own bundled files, and the ones under the home
        // directory are outside the workspace the guard confines everything
        // else to.
        .with_readable_roots(self.catalog.read().await.dirs())
    }

    pub async fn workspace(&self) -> PathBuf {
        self.workspace.read().await.clone()
    }

    pub async fn providers(&self) -> Vec<ProviderConfig> {
        self.providers.read().await.clone()
    }

    /// The global provider layer alone, as an editor must see it.
    ///
    /// [`Self::providers`] returns the effective list with this workspace's
    /// overrides already applied. Editing that and saving it would write every
    /// inherited and overridden value into the global file, so a setting made
    /// for one project would silently follow the user into all the others.
    pub async fn global_providers(&self) -> Vec<ProviderConfig> {
        config::load_providers(None).0
    }

    pub async fn provider_config(&self, id: &str) -> Option<ProviderConfig> {
        self.providers
            .read()
            .await
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    /// Persists an edited provider list to the global layer.
    ///
    /// The effective list is then re-resolved rather than assumed to equal what
    /// was passed in: if this workspace overrides one of these providers, the
    /// override still wins, and the UI must show what will actually be used.
    pub async fn set_providers(&self, providers: Vec<ProviderConfig>) {
        config::save_providers(&providers);
        let workspace = self.workspace.read().await.clone();
        let (effective, _) = config::load_providers(Some(&workspace));
        *self.providers.write().await = effective;
    }

    /// Stores a provider's API key in the OS credential store.
    ///
    /// Takes the id rather than a whole config because the secret is not part
    /// of the config: `providers.json` is written on every settings save, and a
    /// key that travelled with it would eventually be written into it.
    pub async fn set_provider_key(&self, provider_id: &str, key: &str) -> Result<(), String> {
        if !self
            .providers
            .read()
            .await
            .iter()
            .any(|p| p.id == provider_id)
        {
            return Err(format!("no provider configured with id '{provider_id}'"));
        }
        secrets::store(provider_id, key)
    }

    pub async fn clear_provider_key(&self, provider_id: &str) -> Result<(), String> {
        secrets::clear(provider_id)
    }

    /// Where each configured provider's key is coming from.
    ///
    /// Returned for the whole list at once because that is how the settings
    /// screen draws it, and asking per provider would mean one credential-store
    /// round trip per row.
    pub async fn key_statuses(&self) -> Vec<(String, secrets::KeyStatus)> {
        self.providers
            .read()
            .await
            .iter()
            .map(|p| (p.id.clone(), p.key_status()))
            .collect()
    }

    /// Whether this machine can store keys at all, so a frontend can offer the
    /// field or explain its absence instead of failing on save.
    pub fn keychain_available() -> bool {
        secrets::available()
    }

    pub async fn settings(&self) -> Settings {
        self.settings.read().await.clone()
    }

    /// The global `search.json`, for the settings editor.
    pub fn global_search(&self) -> taurus_web::SearchFile {
        config::load_global_search()
    }

    /// Where each configured search backend's key comes from.
    ///
    /// The twin of [`Self::key_statuses`], and for the same reason: both
    /// frontends draw a list of these at once, and asking per backend would be
    /// one credential-store round trip per row.
    pub fn search_key_statuses(&self) -> Vec<(String, secrets::KeyStatus)> {
        config::load_global_search()
            .backends
            .iter()
            .map(|(id, entry)| {
                (
                    id.clone(),
                    config::search_key_status(id, entry.api_key_env.as_deref()),
                )
            })
            .collect()
    }

    /// Whether web search resolved to something that can actually run.
    ///
    /// Distinct from "a backend is selected": a selection with no key resolves
    /// to nothing, and the difference is what the settings screen shows.
    pub async fn search_active(&self) -> bool {
        self.registry.read().await.get("web_search").is_some()
    }

    /// Saves the global search layer and rebuilds, so turning search on
    /// registers its tools without a restart.
    pub async fn set_search(&self, file: taurus_web::SearchFile) {
        config::save_search(&file);
        self.reload().await;
    }

    pub async fn set_search_key(&self, backend_id: &str, key: &str) -> Result<(), String> {
        secrets::store(&config::search_key_id(backend_id), key)?;
        // A saved key can be the thing that makes a selected backend resolve,
        // and the tools are only registered for one that does.
        self.reload().await;
        Ok(())
    }

    pub async fn clear_search_key(&self, backend_id: &str) -> Result<(), String> {
        secrets::clear(&config::search_key_id(backend_id))?;
        self.reload().await;
        Ok(())
    }

    /// Retunes one sub-agent's iteration limit, in place.
    ///
    /// Everything else about the agent is preserved, `model:` and `provider:`
    /// included — see [`taurus_agents::AgentDefinition::write_to`] for why that
    /// is not the same code path an approved proposal takes.
    ///
    /// A built-in has no file to edit, so changing one writes a user-tier
    /// override: the built-in stays as it shipped, and the copy shadows it
    /// everywhere. The path comes back so the caller can say which file now
    /// exists, because a control that silently creates one is a control that
    /// surprises whoever finds the file later.
    pub async fn set_agent_iterations(&self, name: &str, limit: u32) -> Result<String, String> {
        let limit = limit.clamp(1, taurus_agents::MAX_ITERATIONS_LIMIT);

        let (mut definition, path) = {
            let catalog = self.agents.read().await;
            let definition = catalog
                .get(name)
                .ok_or_else(|| format!("no agent named '{name}'"))?
                .clone();
            // A built-in is the only case with nothing to write back to.
            let path = definition.path.clone().unwrap_or_else(|| {
                config::user_agents_dir().join(format!("{}.md", definition.name()))
            });
            (definition, path)
        };

        if definition.frontmatter.max_iterations == limit && definition.path.is_some() {
            return Ok(path.display().to_string());
        }
        definition.frontmatter.max_iterations = limit;
        definition
            .write_to(&path)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;

        info!(agent = name, limit, path = %path.display(), "agent iteration limit changed");
        self.rescan_agents().await;
        Ok(path.display().to_string())
    }

    /// Sets how many model turns one message may take, for every workspace.
    ///
    /// Clamped on the way in as well as on the way out: `load_settings` brings a
    /// hand-edited number back into range, and doing it here too means the
    /// value written to the file is the value that will be used, rather than
    /// one that silently reads back as something else.
    ///
    /// No registry work, unlike the two toggles below — this changes a number
    /// the next turn reads, not which tools exist.
    pub async fn set_max_iterations(&self, limit: u32) {
        let limit = limit.clamp(1, taurus_agents::MAX_ITERATIONS_LIMIT);
        config::edit_settings(Scope::Global, None, |s| s.max_iterations = Some(limit));
        let workspace = self.workspace.read().await.clone();
        *self.settings.write().await = config::load_settings(Some(&workspace));
    }

    /// Toggles skill synthesis for every workspace.
    ///
    /// A project that wants it off regardless can say so in its own
    /// `.taurus/settings.json`, which this will not overwrite.
    pub async fn set_skill_synthesis(&self, enabled: bool) {
        config::edit_settings(Scope::Global, None, |s| {
            s.skill_synthesis_enabled = Some(enabled)
        });
        let workspace = self.workspace.read().await.clone();
        *self.settings.write().await = config::load_settings(Some(&workspace));

        // The tool follows the setting rather than waiting for a reload, so
        // turning synthesis off stops paying for its schema on the next request
        // instead of the next restart. Rebuilding the whole registry here would
        // also drop every MCP connection, which a checkbox has no business
        // doing.
        let resolved = self.settings.read().await.clone();
        let mut registry = self.registry.write().await;
        registry.remove(taurus_skills::PROPOSE_TOOL);
        if resolved.skill_synthesis_enabled
            && !resolved
                .disabled_tools
                .iter()
                .any(|d| d == taurus_skills::PROPOSE_TOOL)
        {
            registry.register(Arc::new(taurus_skills::ProposeSkill::new(
                self.catalog.clone(),
                self.proposals.clone(),
            )));
        }
    }

    /// Toggles sub-agent synthesis for every workspace.
    ///
    /// The twin of [`Host::set_skill_synthesis`], down to not rebuilding the
    /// registry: a checkbox has no business dropping every MCP connection.
    pub async fn set_agent_synthesis(&self, enabled: bool) {
        config::edit_settings(Scope::Global, None, |s| {
            s.agent_synthesis_enabled = Some(enabled)
        });
        let workspace = self.workspace.read().await.clone();
        *self.settings.write().await = config::load_settings(Some(&workspace));

        let resolved = self.settings.read().await.clone();
        let mut registry = self.registry.write().await;
        registry.remove(taurus_core::PROPOSE_AGENT_TOOL);
        if resolved.agent_synthesis_enabled
            && !resolved
                .disabled_tools
                .iter()
                .any(|d| d == taurus_core::PROPOSE_AGENT_TOOL)
        {
            registry.register(Arc::new(ProposeAgent::new(
                self.agents.clone(),
                self.registry.clone(),
                self.agent_proposals.clone(),
            )));
        }
    }

    /// Sets the palette for every workspace.
    ///
    /// Global only, like every other edit from the UI: a theme is a property of
    /// the person looking at the screen, not of the project on it, and writing
    /// it into a workspace file would hand one repo the power to decide how the
    /// app looks everywhere it is opened.
    pub async fn set_theme(&self, theme: Theme) {
        config::edit_settings(Scope::Global, None, |s| s.theme = Some(theme));
        let workspace = self.workspace.read().await.clone();
        *self.settings.write().await = config::load_settings(Some(&workspace));
    }

    /// Which provider serves the embedding model.
    ///
    /// The one the conversation is on, falling back to the first configured:
    /// an embedding model lives on the same server as the chat model in every
    /// local setup, and a second provider entry naming the same machine would
    /// be one more thing to keep in step.
    ///
    /// A method rather than an expression at each of its two call sites because
    /// the first version was written twice and the copy reached for
    /// `blocking_read` inside an async fn — which tokio answers by panicking,
    /// on the one path nobody exercises: a machine with no remembered provider.
    async fn embedding_provider_id(&self) -> Option<String> {
        if let Some(id) = self.settings.read().await.last_provider.clone() {
            return Some(id);
        }
        self.providers.read().await.first().map(|p| p.id.clone())
    }

    /// Which embedding model semantic search runs on. Empty means off.
    ///
    /// Global only. It names a model on the machine's own server, which is a
    /// property of the machine rather than of any one project.
    pub async fn set_embedding_model(&self, model: &str) {
        let model = model.trim().to_string();
        config::edit_settings(Scope::Global, None, |s| s.embedding_model = Some(model));
        let workspace = self.workspace.read().await.clone();
        *self.settings.write().await = config::load_settings(Some(&workspace));
    }

    /// Brings this workspace's semantic index up to date, outside any turn.
    ///
    /// The first index of a repository takes the better part of a minute, and
    /// until this existed the only way to pay that was to be halfway through a
    /// turn when the model first reached for `search_code` — a turn that then
    /// sat on an unreturned tool call for the whole of it. Run here, the cost
    /// is paid when someone chose to pay it, against a progress bar, with a
    /// Stop that stops indexing rather than a conversation.
    ///
    /// It is the same `refresh` the tool calls, deliberately: an index built
    /// here and an index brought up to date by a search have to be the same
    /// thing, or one of the two paths is quietly writing a second format.
    pub async fn build_index(
        &self,
        cancel: CancellationToken,
        progress: Option<&dyn taurus_index::IndexProgress>,
    ) -> Result<String, String> {
        let model = self
            .settings
            .read()
            .await
            .embedding_model
            .trim()
            .to_string();
        if model.is_empty() {
            return Err(
                "No embedding model is set, so there is no index to build. Name one under \
                 Settings → Search."
                    .into(),
            );
        }

        let id = self
            .embedding_provider_id()
            .await
            .ok_or("No provider is configured, so there is nothing to embed with.")?;
        let provider = self.provider(&id).await.map_err(|e| e.to_string())?;

        let workspace = self.workspace.read().await.clone();
        let index = taurus_index::Index::new(
            taurus_index::index_dir(
                &config::home_dir(),
                &crate::sessions::workspace_key(&workspace),
            ),
            &workspace,
        );

        let (_, report) =
            taurus_index::refresh(&index, &workspace, &provider, &model, &cancel, progress).await?;
        Ok(report.summary())
    }

    /// Records the provider and model just used, in both layers.
    ///
    /// The workspace copy is what makes a repo reopen on the model it was last
    /// worked in; the global copy is the starting point for a workspace that
    /// has no memory of its own yet.
    pub async fn remember_session(&self, provider_id: &str, model: &str) {
        let workspace = self.workspace.read().await.clone();
        for (scope, dir) in [
            (Scope::Global, None),
            (Scope::Workspace, Some(workspace.as_path())),
        ] {
            config::edit_settings(scope, dir, |s| {
                s.last_provider = Some(provider_id.to_string());
                s.last_model = Some(model.to_string());
            });
        }

        let mut settings = self.settings.write().await;
        settings.last_provider = Some(provider_id.to_string());
        settings.last_model = Some(model.to_string());
    }

    pub fn catalog(&self) -> &SharedCatalog {
        &self.catalog
    }

    /// The live roster, for a caller re-checking a proposal before it is saved.
    /// [`Host::agents`] returns the summaries a drawer renders; this is the
    /// catalog itself.
    pub fn agent_catalog(&self) -> &SharedAgentCatalog {
        &self.agents
    }

    /// The live registry, for the same reason: a proposal naming a tool has to
    /// be checked against what this session actually has.
    pub fn registry(&self) -> &Arc<RwLock<ToolRegistry>> {
        &self.registry
    }

    pub async fn skills(&self) -> Vec<SkillSummary> {
        self.catalog.read().await.summaries()
    }

    /// The sub-agent roster: the built-ins, plus whatever the last reload found
    /// on disk, with anything that shadowed something else saying so.
    pub async fn agents(&self) -> Vec<AgentSummary> {
        self.agents.read().await.summaries()
    }

    /// Characters of every request the roster costs. Shown next to the roster,
    /// because a cost nobody can see is one nobody chooses.
    pub async fn roster_cost(&self) -> usize {
        self.agents.read().await.roster_cost()
    }

    pub async fn skill_count(&self) -> usize {
        self.catalog.read().await.len()
    }

    /// This conversation's checklist, created on first use.
    async fn plan_board(&self, session_id: &str) -> PlanBoard {
        if let Some(board) = self.plans.read().await.get(session_id) {
            return board.clone();
        }
        self.plans
            .write()
            .await
            .entry(session_id.to_string())
            .or_default()
            .clone()
    }

    /// Drops a conversation's checklist.
    ///
    /// Called when a session is deleted. Without it the map is the one thing in
    /// the host that only ever grows — a few hundred bytes per conversation
    /// opened, which is nothing until a long-running window has opened a
    /// thousand of them.
    pub async fn forget_plan(&self, session_id: &str) {
        self.plans.write().await.remove(session_id);
    }

    /// Resolves a leading `/name` into the skill or sub-agent it refers to.
    ///
    /// `None` for anything that is not a command, which is almost every
    /// message. Callers send the user's own text in that case — expansion is
    /// something that happens on the way to the model, and never changes what
    /// the transcript shows the user having typed.
    pub async fn expand_command(
        &self,
        text: &str,
    ) -> Option<Result<command::Invocation, command::CommandError>> {
        self.rosters(|rosters| rosters.expand(text)).await
    }

    /// Skills and sub-agents a person can run as `/name`, for completion as
    /// they type.
    pub async fn commands(&self) -> Vec<command::CommandSummary> {
        self.rosters(|rosters| rosters.summaries()).await
    }

    /// Borrows both catalogs at once for the slash namespace.
    ///
    /// A closure rather than a returned `Rosters` because the two guards have
    /// to outlive it, and holding both across an `.await` in a caller is how a
    /// reload deadlocks against a keystroke.
    async fn rosters<T>(&self, f: impl FnOnce(command::Rosters<'_>) -> T) -> T {
        let skills = self.catalog.read().await;
        let agents = self.agents.read().await;
        // Read here rather than passed in: whether a turn can delegate is a
        // setting, and the composer asking what it may offer should get the
        // same answer the send path will act on.
        let can_delegate = !self
            .settings
            .read()
            .await
            .disabled_tools
            .iter()
            .any(|tool| tool == taurus_core::SPAWN_TOOL);
        f(command::Rosters {
            skills: &skills,
            agents: &agents,
            can_delegate,
        })
    }

    /// Every directory the last scan read, in precedence order.
    ///
    /// Resolved rather than described, so an empty library can be explained
    /// with the paths actually consulted — including the shared locations,
    /// which the user did not configure and may not know are read.
    pub async fn skill_sources(&self) -> Vec<taurus_skills::SkillSource> {
        config::skill_sources(Some(&self.workspace.read().await.clone()))
    }

    /// The standing brief in force, in the order it reaches the prompt.
    ///
    /// Exposed for the same reason the skill roster is: a file being read is
    /// invisible otherwise, and "why is it doing that" has no answer if the
    /// user cannot see which briefs are loaded.
    pub async fn instructions(&self) -> Vec<Instructions> {
        self.instructions.read().await.clone()
    }

    pub async fn tool_names(&self) -> Vec<String> {
        self.registry
            .read()
            .await
            .names()
            .map(str::to_string)
            .collect()
    }

    /// What the model is told it can call, as it goes over the wire.
    ///
    /// The same list [`Host::build_agent`] would hand a turn, minus the spawn
    /// tool that is added per turn. Exposed so the cost of advertising it can be
    /// reported: this is the part of every request that is fixed overhead, paid
    /// again on each iteration whether or not a tool is called.
    pub async fn tool_definitions(&self) -> Vec<taurus_provider::ToolDef> {
        self.registry.read().await.definitions()
    }

    /// The system prompt a turn in this workspace would carry.
    pub async fn system_prompt(&self) -> String {
        prompt::build(
            &self.workspace.read().await.clone(),
            self.catalog.read().await.prompt_section(),
            instructions::section(&self.instructions.read().await),
            self.settings.read().await.skill_synthesis_enabled,
            self.settings.read().await.agent_synthesis_enabled,
        )
    }

    /// Everything that failed to load, tagged with where it came from.
    pub async fn problems(&self) -> Vec<Problem> {
        self.problems.read().await.clone()
    }

    /// Just the problems one screen is responsible for showing.
    pub async fn problems_from(&self, sources: &[ProblemSource]) -> Vec<Problem> {
        problem::of(&self.problems.read().await, sources)
    }

    pub async fn mcp_statuses(&self) -> Vec<ServerStatus> {
        self.mcp.statuses().await
    }

    /// Every configured server, merged across layers, with how it is doing.
    ///
    /// One call rather than a listing plus a status lookup, because the two have
    /// to agree: a panel that renders a server from one snapshot and its state
    /// from another shows a connected server that is no longer configured for as
    /// long as it takes the second call to land.
    pub async fn mcp_servers(&self) -> Vec<McpServerView> {
        let workspace = self.workspace.read().await.clone();
        let (config, defined_in) = self.mcp_layers(&workspace);
        let statuses: BTreeMap<String, ServerStatus> = self
            .mcp
            .statuses()
            .await
            .into_iter()
            .map(|s| (s.name.clone(), s))
            .collect();

        config
            .servers
            .into_iter()
            .map(|(name, server)| {
                let status = statuses.get(&name).cloned();
                McpServerView::new(name, server, &defined_in, status)
            })
            .collect()
    }

    /// Both `mcp.json` layers, merged, plus which layer defined each server.
    ///
    /// The scope matters to the panel in a way it does not to the connector:
    /// editing a server has to write to the file it came from, and a workspace
    /// entry saved into the global file would silently change every other
    /// project.
    fn mcp_layers(&self, workspace: &Path) -> (taurus_mcp::McpConfig, LayerOf) {
        let mut layers = Vec::new();
        let mut defined_in: LayerOf = BTreeMap::new();
        for scope in [Scope::Global, Scope::Workspace] {
            let Some(dir) = config::scope_dir(scope, Some(workspace)) else {
                continue;
            };
            let Ok(layer) = taurus_mcp::load(&dir) else {
                continue;
            };
            for (name, server) in &layer.servers {
                // A toggle changes a server rather than defining one, so it must
                // not claim ownership: editing would then write a command line
                // into the file that only meant to switch one off.
                if !matches!(server, taurus_mcp::ServerConfig::Toggle(_)) {
                    defined_in.insert(name.clone(), scope);
                }
            }
            layers.push(layer);
        }
        let (merged, _) = config::merge_mcp(layers);
        (merged, defined_in)
    }

    /// Reconnects the MCP servers without rebuilding anything else.
    ///
    /// What the MCP panel calls after a save. [`Host::reload`] would also do it,
    /// and also rescan every skill directory, re-read both provider layers, and
    /// rebuild the agent roster — none of which a change to `mcp.json` can
    /// affect. The narrower call is the same argument `rescan_agents` makes in
    /// the other direction: editing one thing should not restart the rest.
    ///
    /// The swap is by name. Every MCP tool carries the `mcp__` prefix, so the
    /// old set can be lifted out of the live registry and a new one put back
    /// without touching the built-ins, the skill tools, or the web tools beside
    /// them.
    pub async fn reload_mcp(&self) {
        let workspace = self.workspace.read().await.clone();

        let mut problems = Vec::new();
        let mut layers = Vec::new();
        for dir in config::config_dirs(Some(&workspace)) {
            match taurus_mcp::load(&dir) {
                Ok(layer) => layers.push(layer),
                Err(e) => problems.push(Problem::new(ProblemSource::Mcp, e)),
            }
        }
        let (config, merge_problems) = config::merge_mcp(layers);
        problems.extend(Problem::tag(ProblemSource::Mcp, merge_problems));

        self.mcp.shutdown().await;
        let tools = self.mcp.connect_all(&config).await;

        // Applied to the new tools only. `reload` applies it to everything, but
        // there is nothing else here to apply it to, and a tool the user turned
        // off must not come back because its server reconnected.
        let disabled = self.settings.read().await.disabled_tools.clone();
        let mut registry = self.registry.write().await;
        let stale: Vec<String> = registry
            .names()
            .filter(|name| taurus_mcp::is_mcp_tool(name))
            .map(str::to_string)
            .collect();
        for name in stale {
            registry.remove(&name);
        }
        for tool in tools {
            if disabled.iter().any(|off| off == tool.name()) {
                continue;
            }
            registry.register(tool);
        }
        drop(registry);

        // Only this source. A malformed `providers.json` reported at the last
        // full reload is still malformed, and clearing it here would make it
        // vanish from Settings until something unrelated reloaded.
        let mut held = self.problems.write().await;
        held.retain(|p| p.source != ProblemSource::Mcp);
        held.extend(problems);
    }

    /// Connects to one server, reports what it offers, and disconnects.
    ///
    /// Nothing is registered and no live connection is disturbed — see
    /// [`taurus_mcp::probe`]. This is what makes "Test" safe to press against an
    /// edit of a server that is currently working.
    pub async fn test_mcp_server(
        &self,
        name: &str,
        server: &taurus_mcp::ServerConfig,
    ) -> Result<Vec<String>, String> {
        server.validate()?;
        taurus_mcp::probe(name, server).await
    }

    pub async fn permissions(&self) -> Arc<PermissionEngine> {
        self.permissions.read().await.clone()
    }

    /// Picks a provider and model, preferring the caller's choice, then what
    /// was used last, then whatever the backend offers first.
    pub async fn resolve_model(
        &self,
        provider_id: Option<&str>,
        model: Option<&str>,
    ) -> Result<(String, String), String> {
        let providers = self.providers().await;
        let settings = self.settings().await;

        let chosen = provider_id
            .map(str::to_string)
            .or(settings.last_provider)
            .filter(|id| providers.iter().any(|p| &p.id == id))
            .or_else(|| providers.first().map(|p| p.id.clone()))
            .ok_or_else(|| "no providers are configured".to_string())?;

        if let Some(model) = model {
            return Ok((chosen, model.to_string()));
        }

        let configured = providers
            .iter()
            .find(|p| p.id == chosen)
            .and_then(|p| p.default_model.clone())
            .filter(|m| !m.trim().is_empty());

        let provider = self.provider(&chosen).await?;
        let available = match provider.models().await {
            Ok(available) => available,
            // A listing is not something every backend has. An Azure APIM
            // route often exposes the chat endpoint and nothing else, and that
            // is no reason to be unusable when the config already says which
            // model to talk to.
            Err(e) => {
                let Some(default) = configured else {
                    return Err(format!(
                        "could not list models from '{chosen}': {e}. If this backend has no \
                         model listing, give it a `default_model` in providers.json or name \
                         one with --model."
                    ));
                };
                return Ok((chosen, default));
            }
        };

        let preferred = settings
            .last_model
            .filter(|m| available.iter().any(|a| &a.id == m))
            // Ahead of "whatever came first" but behind the model this
            // workspace was last worked in, which is a decision the user made
            // more recently than the config file.
            .or(configured)
            .or_else(|| available.first().map(|m| m.id.clone()))
            .ok_or_else(|| format!("provider '{chosen}' has no models available"))?;

        Ok((chosen, preferred))
    }
}

/// Removes the tools the user has turned off, returning a message for every
/// name that matched nothing.
///
/// An unmatched name is worth saying out loud rather than dropping. The setting
/// is a list of hand-typed strings with nothing checking them, and a typo looks
/// exactly like a tool that is quietly still enabled — the failure is silent in
/// the direction that costs tokens and leaves a tool reachable.
fn disable(registry: &mut ToolRegistry, disabled: &[String]) -> Vec<String> {
    let mut unmatched = Vec::new();
    for name in disabled {
        if registry.remove(name) {
            info!(tool = %name, "tool disabled by settings");
        } else {
            unmatched.push(format!(
                "settings.json disables '{name}', which is not a registered tool. \
                 `taurus tools` lists the names that work."
            ));
        }
    }
    unmatched
}

/// Intersects every agent's `tools:` list with the finished registry, returning
/// a message for each agent that had to be refused.
///
/// This exists because of what an *empty* allow-list means downstream: every
/// tool the parent has. An agent scoped to `[read_file, grep]` whose two tools
/// were both disabled would filter down to nothing and be handed the shell — a
/// setting reached for to *narrow* an agent widening it instead. So a scope that
/// survives in part degrades, and a scope that vanishes entirely is refused.
///
/// Only the refusal is a problem. A partial loss is recorded on the agent and
/// shown on its row, the same way a skill's missing interpreter is: the agent
/// still runs, so the status strip has nothing to send anyone to fix.
fn cross_check_tools(agents: &mut AgentCatalog, available: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut refused = Vec::new();

    for agent in agents.iter_mut() {
        let Some(wanted) = agent.frontmatter.tools.clone() else {
            continue;
        };
        let missing: Vec<String> = wanted
            .iter()
            .filter(|name| !available.contains(name))
            .cloned()
            .collect();
        if missing.is_empty() {
            continue;
        }

        if missing.len() == wanted.len() {
            problems.push(format!(
                "{}: none of the tools it is scoped to are available here ({}). An empty scope \
                 would mean every tool rather than none, so this agent has been refused instead \
                 of widened. Re-enable those tools, or correct the names.",
                located(agent),
                wanted.join(", ")
            ));
            refused.push(agent.name().to_string());
        } else {
            degrade(
                agent,
                format!(
                    "cannot use {}, which this session does not have; it runs with the rest of \
                     its tools",
                    missing.join(", ")
                ),
            );
        }
    }

    for name in refused {
        agents.remove(&name);
    }
    problems
}

/// Adds a reason without losing one already there: an agent can be both scoped
/// to a missing tool and pointed at an unconfigured provider, and a user fixing
/// one should not discover the other only afterwards.
fn degrade(agent: &mut AgentDefinition, reason: String) {
    agent.degraded = Some(match agent.degraded.take() {
        Some(existing) => format!("{existing}; {reason}"),
        None => reason,
    });
}

/// What to call an agent in a problem message. The file, when there is one —
/// the whole authoring surface is a text editor, so the path is the fix.
fn located(agent: &AgentDefinition) -> String {
    match &agent.path {
        Some(path) => path.display().to_string(),
        None => format!("the built-in agent '{}'", agent.name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{isolated_home, HomeGuard};
    use async_trait::async_trait;
    use taurus_tools::builtin::fs::{ReadFile, WriteFile};
    use taurus_tools::{DenyAll, Tool, ToolError};
    use tempfile::TempDir;

    struct DenyingPrompts;

    impl PermissionPromptFactory for DenyingPrompts {
        fn create(&self) -> Box<dyn PermissionPrompt> {
            Box::new(DenyAll)
        }
    }

    struct NoProposals;

    #[async_trait]
    impl ProposalSink for NoProposals {
        async fn submit(&self, _: taurus_skills::SkillProposal) {}
    }

    #[async_trait]
    impl AgentProposalSink for NoProposals {
        async fn submit(&self, _: taurus_agents::AgentProposal) {}
    }

    /// A host over an isolated config home.
    ///
    /// The guard comes back with it and must be held for the whole test:
    /// `set_workspace` and `remember_session` write config as a side effect,
    /// so dropping it early points those writes at the real `~/.taurus`.
    fn host(workspace: &Path) -> (Host, HomeGuard) {
        let home = isolated_home();
        let host = Host::new(
            workspace.to_path_buf(),
            Arc::new(DenyingPrompts),
            Arc::new(taurus_tools::Unattended),
            Arc::new(NoProposals),
            Arc::new(NoProposals),
        );
        (host, home)
    }

    /// Writes an agent file into a workspace's `.taurus/agents`.
    fn write_agent(workspace: &Path, name: &str, frontmatter: &str) {
        let dir = workspace.join(".taurus/agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}.md")),
            format!(
                "---\nname: {name}\ndescription: does {name}\n{frontmatter}---\n\nBe {name}.\n"
            ),
        )
        .unwrap();
    }

    fn write_settings(workspace: &Path, json: &str) {
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(workspace.join(".taurus/settings.json"), json).unwrap();
    }

    /// How many agents a roster has before anything is read from disk. Derived,
    /// so that shipping another built-in is a change to one file rather than to
    /// every test that happens to count the roster.
    fn builtins() -> usize {
        taurus_agents::builtin::definitions().len()
    }

    fn agent_problems(problems: &[Problem]) -> Vec<String> {
        problems
            .iter()
            .filter(|p| p.source == ProblemSource::Agents)
            .map(|p| p.message.clone())
            .collect()
    }

    #[tokio::test]
    async fn a_machine_with_no_agents_directory_still_has_the_builtins() {
        let dir = TempDir::new().unwrap();
        let (host, _home) = host(&dir.path().canonicalize().unwrap());
        host.reload().await;

        let names: Vec<String> = host.agents().await.into_iter().map(|a| a.name).collect();
        assert_eq!(
            names,
            vec![
                "coder".to_string(),
                "explorer".to_string(),
                "worker".to_string()
            ]
        );
        assert!(agent_problems(&host.problems().await).is_empty());
    }

    #[tokio::test]
    async fn retuning_an_agent_keeps_everything_else_in_its_file() {
        // The trap this exists for: the editor's save path rebuilds a file from
        // an `AgentProposal`, which drops `model:` and `provider:` on purpose —
        // a model does not get to choose what its delegate costs. Reusing it to
        // change one number would silently strip both from a file whose author
        // set them by hand.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        write_agent(
            &workspace,
            "reviewer",
            "tools: [read_file, grep]\nmax_iterations: 12\nmodel: gpt-4o\nprovider: apim\n",
        );

        let (host, _home) = host(&workspace);
        host.reload().await;
        host.set_agent_iterations("reviewer", 40).await.unwrap();

        let agent = host
            .agents()
            .await
            .into_iter()
            .find(|a| a.name == "reviewer")
            .expect("reviewer should still be on the roster");
        assert_eq!(agent.max_iterations, 40);
        assert_eq!(agent.model.as_deref(), Some("gpt-4o"));
        assert_eq!(agent.provider.as_deref(), Some("apim"));
        assert_eq!(
            agent.tools.as_deref(),
            Some(&["read_file".to_string(), "grep".to_string()][..])
        );

        // The body is this agent's system prompt; losing it would leave a file
        // the loader rejects.
        let text = std::fs::read_to_string(workspace.join(".taurus/agents/reviewer.md")).unwrap();
        assert!(text.contains("Be reviewer."), "{text}");
    }

    #[tokio::test]
    async fn retuning_a_builtin_writes_an_override_rather_than_failing() {
        // A built-in has no file. Refusing would make the control dead on the
        // three agents that ship, which are the ones most likely to need
        // retuning before anyone has written one of their own.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let (host, _home) = host(&workspace);
        host.reload().await;

        let path = host.set_agent_iterations("worker", 45).await.unwrap();
        assert!(path.ends_with("worker.md"), "{path}");
        assert!(std::path::Path::new(&path).exists(), "{path} should exist");

        let worker = host
            .agents()
            .await
            .into_iter()
            .find(|a| a.name == "worker")
            .expect("worker should still be on the roster");
        assert_eq!(worker.max_iterations, 45);
        // The copy shadows the built-in rather than joining it — one `worker`,
        // not two.
        assert_eq!(
            host.agents()
                .await
                .iter()
                .filter(|a| a.name == "worker")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn an_agents_limit_is_clamped_to_the_same_ceiling_a_file_is() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        write_agent(&workspace, "reviewer", "max_iterations: 12\n");

        let (host, _home) = host(&workspace);
        host.reload().await;
        host.set_agent_iterations("reviewer", 100_000)
            .await
            .unwrap();

        let agent = host
            .agents()
            .await
            .into_iter()
            .find(|a| a.name == "reviewer")
            .unwrap();
        assert_eq!(agent.max_iterations, taurus_agents::MAX_ITERATIONS_LIMIT);
    }

    #[tokio::test]
    async fn retuning_an_agent_nobody_has_says_so() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let (host, _home) = host(&workspace);
        host.reload().await;

        let err = host.set_agent_iterations("nobody", 30).await.unwrap_err();
        assert!(err.contains("nobody"), "{err}");
    }

    #[tokio::test]
    async fn a_project_agent_file_joins_the_roster() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        write_agent(&workspace, "reviewer", "tools: [read_file, grep]\n");

        let (host, _home) = host(&workspace);
        host.reload().await;

        let agents = host.agents().await;
        let reviewer = agents.iter().find(|a| a.name == "reviewer").unwrap();
        assert_eq!(reviewer.tier, AgentTier::Project);
        assert_eq!(
            reviewer.tools.as_deref(),
            Some(["read_file", "grep"].map(String::from).as_slice())
        );
        assert!(reviewer.degraded.is_none());
    }

    #[tokio::test]
    async fn an_agent_whose_whole_tool_list_was_disabled_is_refused_not_widened() {
        // The one failure in this feature that widens a permission rather than
        // breaking a feature: an empty scope means *every* tool downstream, so
        // an agent the user narrowed must never arrive there by attrition.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        write_settings(&workspace, r#"{"disabled_tools": ["run_command"]}"#);
        write_agent(&workspace, "shell-only", "tools: [run_command]\n");

        let (host, _home) = host(&workspace);
        host.reload().await;

        assert!(
            !host.agents().await.iter().any(|a| a.name == "shell-only"),
            "an agent with nothing left to be scoped to must not stay in the roster"
        );
        let reported = agent_problems(&host.problems().await);
        assert!(
            reported.iter().any(|m| m.contains("refused")),
            "the refusal must be reported, not silent: {reported:?}"
        );
    }

    #[tokio::test]
    async fn an_agent_that_loses_only_some_tools_is_degraded_and_kept() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        write_settings(&workspace, r#"{"disabled_tools": ["run_command"]}"#);
        write_agent(&workspace, "mixed", "tools: [read_file, run_command]\n");

        let (host, _home) = host(&workspace);
        host.reload().await;

        let agents = host.agents().await;
        let mixed = agents.iter().find(|a| a.name == "mixed").unwrap();
        assert!(mixed
            .degraded
            .as_ref()
            .is_some_and(|d| d.contains("run_command")));
    }

    #[tokio::test]
    async fn an_agent_naming_an_unconfigured_provider_loads_degraded() {
        // A repo can ship an agent that names a cloud model without breaking for
        // the contributor who runs Ollama only.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        write_agent(
            &workspace,
            "cloud-thinker",
            "model: gpt-5\nprovider: not-configured\n",
        );

        let (host, _home) = host(&workspace);
        host.reload().await;

        let agents = host.agents().await;
        let agent = agents.iter().find(|a| a.name == "cloud-thinker").unwrap();
        let reason = agent.degraded.as_ref().expect("it should say why");
        assert!(reason.contains("not-configured"));
        assert!(
            reason.contains("session's model"),
            "and what it falls back to"
        );

        // Degradation is not a problem: the agent still runs, so there is
        // nothing for the status strip to send anyone to fix.
        assert!(agent_problems(&host.problems().await).is_empty());
    }

    #[tokio::test]
    async fn an_oversized_roster_reports_what_it_costs() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        // Descriptions are capped at 200 characters each, so this is what
        // "too many agents" looks like: enough of them to be worth saying.
        for i in 0..20 {
            let name = format!("agent-{i:02}");
            let agents_dir = workspace.join(".taurus/agents");
            std::fs::create_dir_all(&agents_dir).unwrap();
            std::fs::write(
                agents_dir.join(format!("{name}.md")),
                format!(
                    "---\nname: {name}\ndescription: {}\n---\n\nBe {name}.\n",
                    "x".repeat(180)
                ),
            )
            .unwrap();
        }

        let (host, _home) = host(&workspace);
        host.reload().await;

        let reported = agent_problems(&host.problems().await);
        assert!(
            reported
                .iter()
                .any(|m| m.contains("characters of every request")),
            "an expense this size should be visible, not silent: {reported:?}"
        );
        assert_eq!(
            host.agents().await.len(),
            builtins() + 20,
            "and nothing is dropped for it"
        );
    }

    #[tokio::test]
    async fn a_rescan_picks_up_a_file_written_since_the_reload() {
        // The drawer's whole job. Editing a file and reopening the drawer to see
        // the old catalog is the feature not working, not a papercut.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let (host, _home) = host(&workspace);
        host.reload().await;
        assert_eq!(host.agents().await.len(), builtins());

        write_agent(&workspace, "late-arrival", "");
        host.rescan_agents().await;

        assert!(host.agents().await.iter().any(|a| a.name == "late-arrival"));
    }

    #[tokio::test]
    async fn a_rescan_clears_a_problem_the_user_has_since_fixed() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus/agents")).unwrap();
        let broken = workspace.join(".taurus/agents/broken.md");
        std::fs::write(&broken, "not an agent file").unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;
        assert_eq!(agent_problems(&host.problems().await).len(), 1);

        std::fs::remove_file(&broken).unwrap();
        host.rescan_agents().await;

        assert!(agent_problems(&host.problems().await).is_empty());
    }

    #[tokio::test]
    async fn a_rescan_leaves_other_sources_problems_alone() {
        // The problem list is shared. A rescan that swept it would silently
        // clear a providers.json error nobody had fixed.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(workspace.join(".taurus/providers.json"), "{ not json").unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;
        let before = host.problems_from(&[ProblemSource::Providers]).await.len();
        assert!(before > 0);

        host.rescan_agents().await;
        assert_eq!(
            host.problems_from(&[ProblemSource::Providers]).await.len(),
            before
        );
    }

    #[tokio::test]
    async fn reload_registers_the_skill_tools_alongside_the_builtins() {
        let dir = TempDir::new().unwrap();
        let (host, _home) = host(&dir.path().canonicalize().unwrap());
        host.reload().await;

        let tools = host.tool_names().await;
        for expected in ["read_file", "run_command", "load_skill", "propose_skill"] {
            assert!(tools.iter().any(|t| t == expected), "missing {expected}");
        }
        // The spawn tool is deliberately absent here; it is added per turn.
        assert!(!tools.iter().any(|t| t == taurus_core::SPAWN_TOOL));
    }

    #[tokio::test]
    async fn the_agent_proposal_tool_follows_its_own_setting() {
        // Two capabilities, two switches. Wanting the model to write procedures
        // is no reason to want it writing delegates, and the schemas are paid
        // for separately on every request.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/settings.json"),
            r#"{"agent_synthesis_enabled": false}"#,
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let tools = host.tool_names().await;
        assert!(!tools.iter().any(|t| t == taurus_core::PROPOSE_AGENT_TOOL));
        assert!(
            tools.iter().any(|t| t == taurus_skills::PROPOSE_TOOL),
            "turning one off must not take the other with it"
        );
    }

    #[tokio::test]
    async fn toggling_agent_synthesis_adds_and_removes_the_tool_without_a_reload() {
        // A checkbox must not restart every MCP server to take effect, which is
        // what rebuilding the registry here would do.
        let dir = TempDir::new().unwrap();
        let (host, _home) = host(&dir.path().canonicalize().unwrap());
        host.reload().await;
        assert!(host
            .tool_names()
            .await
            .iter()
            .any(|t| t == taurus_core::PROPOSE_AGENT_TOOL));

        host.set_agent_synthesis(false).await;
        assert!(!host
            .tool_names()
            .await
            .iter()
            .any(|t| t == taurus_core::PROPOSE_AGENT_TOOL));

        host.set_agent_synthesis(true).await;
        assert!(host
            .tool_names()
            .await
            .iter()
            .any(|t| t == taurus_core::PROPOSE_AGENT_TOOL));
    }

    #[tokio::test]
    async fn the_proposal_tool_is_not_advertised_when_synthesis_is_off() {
        // Its schema is one of the largest the harness ships, and with the
        // setting off nothing in the prompt tells the model what it is for.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/settings.json"),
            r#"{"skill_synthesis_enabled": false}"#,
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let tools = host.tool_names().await;
        assert!(!tools.iter().any(|t| t == taurus_skills::PROPOSE_TOOL));
        // The rest of the skill tools are about using skills, not writing them.
        assert!(tools.iter().any(|t| t == "load_skill"));
    }

    #[tokio::test]
    async fn toggling_synthesis_adds_and_removes_the_tool_without_a_reload() {
        let dir = TempDir::new().unwrap();
        let (host, _home) = host(&dir.path().canonicalize().unwrap());
        host.reload().await;
        assert!(host
            .tool_names()
            .await
            .iter()
            .any(|t| t == taurus_skills::PROPOSE_TOOL));

        host.set_skill_synthesis(false).await;
        assert!(!host
            .tool_names()
            .await
            .iter()
            .any(|t| t == taurus_skills::PROPOSE_TOOL));

        host.set_skill_synthesis(true).await;
        assert!(host
            .tool_names()
            .await
            .iter()
            .any(|t| t == taurus_skills::PROPOSE_TOOL));
    }

    #[tokio::test]
    async fn reloading_mcp_leaves_every_other_tool_where_it_was() {
        // The reason this is narrower than `reload`: a change to `mcp.json`
        // cannot affect a skill, an agent, or a provider, and restarting them to
        // pick one up costs a visible pause on every save in the panel. The
        // invariant is that the registry comes back with everything that was not
        // an MCP tool still in it.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let (host, home) = host(&workspace);
        host.reload().await;

        let before = host.tool_names().await;
        std::fs::write(
            taurus_mcp::config::config_file(home.path()),
            r#"{"mcpServers": {"broken": {"command": "definitely-not-a-real-program-xyz"}}}"#,
        )
        .unwrap();

        host.reload_mcp().await;

        assert_eq!(
            host.tool_names().await,
            before,
            "a server that fails to start must leave the rest of the registry alone"
        );
        // A server that will not start is a status, not a problem: it is
        // reported on its own row in the panel, where the thing that can fix it
        // is. Problems are for entries with no row to report on.
        assert!(host.problems_from(&[ProblemSource::Mcp]).await.is_empty());
        let servers = host.mcp_servers().await;
        assert_eq!(servers.len(), 1);
        assert!(servers[0].status.as_ref().unwrap().error.is_some());
    }

    #[tokio::test]
    async fn an_mcp_problem_is_reported_once_and_clears_when_the_entry_does() {
        // `reload_mcp` replaces this source rather than appending to it. Getting
        // that wrong stacks a duplicate on every save, and leaves a fixed entry
        // being complained about until something unrelated reloaded.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let (host, home) = host(&workspace);
        let file = taurus_mcp::config::config_file(home.path());

        std::fs::write(&file, r#"{"mcpServers": {"typo": {"commnd": "npx"}}}"#).unwrap();
        host.reload_mcp().await;
        host.reload_mcp().await;
        assert_eq!(host.problems_from(&[ProblemSource::Mcp]).await.len(), 1);

        std::fs::write(&file, r#"{"mcpServers": {}}"#).unwrap();
        host.reload_mcp().await;
        assert!(host.problems_from(&[ProblemSource::Mcp]).await.is_empty());
    }

    #[tokio::test]
    async fn reloading_mcp_reports_an_unreadable_entry_without_losing_its_neighbours() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let (host, home) = host(&workspace);
        std::fs::write(
            taurus_mcp::config::config_file(home.path()),
            r#"{"mcpServers": {
                 "typo": {"commnd": "npx"},
                 "off":  {"command": "npx", "disabled": true}
               }}"#,
        )
        .unwrap();

        host.reload().await;

        // The unreadable one is named; the one beside it still made it into the
        // listing, which is the whole point of parsing per entry.
        let problems = host.problems_from(&[ProblemSource::Mcp]).await;
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].message.contains("typo"), "{problems:?}");

        let servers = host.mcp_servers().await;
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "off");
        assert!(servers[0].disabled);
    }

    #[tokio::test]
    async fn a_workspace_can_turn_off_a_tool_it_does_not_want_advertised() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/settings.json"),
            r#"{"disabled_tools": ["run_command"]}"#,
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let tools = host.tool_names().await;
        assert!(!tools.iter().any(|t| t == "run_command"));
        // Only the named one goes.
        assert!(tools.iter().any(|t| t == "read_file"));
        assert!(
            host.problems().await.is_empty(),
            "{:?}",
            host.problems().await
        );
    }

    #[tokio::test]
    async fn a_disabled_tool_is_gone_from_the_registry_not_merely_undeclared() {
        // The distinction that matters: a tool the model cannot see but a skill
        // could still call is not turned off.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/settings.json"),
            r#"{"disabled_tools": ["run_command"]}"#,
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let registry = host.registry.read().await;
        assert!(registry.get("run_command").is_none());
        assert!(!registry
            .definitions()
            .iter()
            .any(|d| d.name == "run_command"));
    }

    /// The tools one turn actually gets, which is the shared registry plus the
    /// four a parent adds for itself.
    async fn turn_tools(host: &Host) -> Vec<String> {
        let agent = host
            .build_agent(
                taurus_core::testing::FakeProvider::new(Vec::new()),
                "test-model",
                CancellationToken::new(),
                TurnRef {
                    session_id: "s1",
                    prompt: "hello",
                },
            )
            .await;
        agent.registry().names().map(str::to_string).collect()
    }

    #[tokio::test]
    async fn a_turn_can_draw_and_ask_but_a_sub_agent_cannot() {
        // The three drawing tools address the person watching this
        // conversation. A delegate has no such person, and it shares the
        // registry below — so `ask_user` reaching it would be a worker blocked
        // on a question nobody will ever see.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let (host, _home) = host(&workspace);
        host.reload().await;

        let turn = turn_tools(&host).await;
        let shared: Vec<String> = host
            .registry
            .read()
            .await
            .names()
            .map(str::to_string)
            .collect();

        for tool in PER_TURN_TOOLS {
            assert!(turn.contains(&tool.to_string()), "turn is missing {tool}");
            assert!(
                !shared.contains(&tool.to_string()),
                "{tool} leaked to children"
            );
        }
    }

    #[tokio::test]
    async fn a_per_turn_tool_can_be_disabled_like_any_other() {
        // The set a turn adds for itself was the one `disabled_tools` could not
        // reach, which made the guarantee — a disabled tool is not registered
        // at all — quietly false for exactly four names.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/settings.json"),
            r#"{"disabled_tools": ["show_chart", "spawn_subagent"]}"#,
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let turn = turn_tools(&host).await;
        assert!(!turn.contains(&"show_chart".to_string()), "{turn:?}");
        assert!(!turn.contains(&"spawn_subagent".to_string()), "{turn:?}");
        assert!(turn.contains(&"show_table".to_string()), "{turn:?}");

        // And naming one is not reported as naming a tool that does not exist,
        // which would send someone hunting for a typo in a line that works.
        let problems = host.problems().await;
        assert!(
            !problems.iter().any(|p| p.source == ProblemSource::Tools),
            "{problems:?}"
        );
    }

    #[tokio::test]
    async fn disabling_a_tool_that_does_not_exist_says_so() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/settings.json"),
            r#"{"disabled_tools": ["run_comand"]}"#,
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let problems = host.problems().await;
        assert!(
            problems
                .iter()
                .any(|p| p.source == ProblemSource::Tools && p.message.contains("run_comand")),
            "a typo must not look like a tool that is quietly still on: {problems:?}"
        );
        // The real tool is untouched by the near-miss.
        assert!(host.tool_names().await.iter().any(|t| t == "run_command"));
    }

    #[tokio::test]
    async fn the_web_tools_stay_unregistered_until_a_backend_is_configured() {
        let dir = TempDir::new().unwrap();
        let (host, _home) = host(&dir.path().canonicalize().unwrap());
        host.reload().await;

        let tools = host.tool_names().await;
        assert!(!tools.iter().any(|t| t == "web_search"));
        assert!(!tools.iter().any(|t| t == "fetch_url"));
        // Not configuring search is the default, not a misconfiguration.
        assert!(
            host.problems().await.is_empty(),
            "{:?}",
            host.problems().await
        );
    }

    #[tokio::test]
    async fn a_workspace_can_turn_web_search_on_for_one_project() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/search.json"),
            r#"{"backend": "local",
                "backends": {"local": {"kind": "searxng", "base_url": "http://localhost:8888"}}}"#,
        )
        .unwrap();

        let other = TempDir::new().unwrap();
        let (host, _home) = host(&other.path().canonicalize().unwrap());
        host.reload().await;
        assert!(!host.tool_names().await.iter().any(|t| t == "web_search"));

        host.set_workspace(&workspace).await.unwrap();
        let tools = host.tool_names().await;
        // Both, or neither: search that cannot be followed up is half a tool.
        assert!(tools.iter().any(|t| t == "web_search"));
        assert!(tools.iter().any(|t| t == "fetch_url"));
    }

    #[tokio::test]
    async fn a_search_backend_that_cannot_run_is_reported_rather_than_registered() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/search.json"),
            r#"{"backend": "brave",
                "backends": {"brave": {"kind": "brave",
                                       "api_key_env": "TAURUS_TEST_HOST_UNSET_KEY"}}}"#,
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        assert!(!host.tool_names().await.iter().any(|t| t == "web_search"));
        let problems = host.problems().await;
        assert!(
            problems.iter().any(|p| p.source == ProblemSource::Search
                && p.message.contains("TAURUS_TEST_HOST_UNSET_KEY")),
            "the missing variable has to reach the user, tagged to search: {problems:?}"
        );
    }

    #[tokio::test]
    async fn a_first_run_leaves_an_editable_search_file_behind() {
        let dir = TempDir::new().unwrap();
        let (host, home) = host(&dir.path().canonicalize().unwrap());
        host.reload().await;

        let written = std::fs::read_to_string(home.path().join("search.json")).unwrap();
        assert!(
            written.contains("brave") && written.contains("searxng"),
            "{written}"
        );
        // Written, but off: installing the app must not start sending prompts
        // to a search engine.
        assert!(!host.tool_names().await.iter().any(|t| t == "web_search"));
    }

    #[tokio::test]
    async fn workspace_skills_are_discovered_after_a_workspace_change() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let skills = workspace.join(".taurus/skills/greet");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(
            skills.join("SKILL.md"),
            "---\nname: greet\ndescription: d\nwhen_to_use: when greeting someone\n---\nSay hello.",
        )
        .unwrap();

        let other = TempDir::new().unwrap();
        let (host, _home) = host(&other.path().canonicalize().unwrap());
        host.reload().await;
        assert_eq!(host.skill_count().await, 0);

        host.set_workspace(&workspace).await.unwrap();
        assert_eq!(host.skill_count().await, 1);
    }

    /// Writes a skill in the shape another client leaves behind: `SKILL.md`
    /// with only the two fields the Agent Skills specification requires.
    fn write_borrowed_skill(root: &Path, name: &str, description: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nDo the thing."),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn skills_installed_by_another_client_are_discovered() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        write_borrowed_skill(
            &workspace.join(".agents/skills"),
            "shared-convention",
            "Use when the task is shared between clients.",
        );
        write_borrowed_skill(
            &workspace.join(".claude/skills"),
            "installed-elsewhere",
            "Use when the skill was installed by another client.",
        );

        let (host, _home) = host(&workspace);
        host.reload().await;

        let names: Vec<String> = host.skills().await.into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["installed-elsewhere", "shared-convention"]);

        // The description stands in for the trigger line, so a borrowed skill
        // is not merely counted — it is selectable.
        let prompt = host.system_prompt().await;
        assert!(
            prompt.contains("- shared-convention: Use when the task is shared between clients."),
            "{prompt}"
        );
    }

    #[tokio::test]
    async fn a_taurus_skill_shadows_a_borrowed_one_of_the_same_name() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        write_borrowed_skill(
            &workspace.join(".claude/skills"),
            "review",
            "the borrowed one",
        );
        let native = workspace.join(".taurus/skills/review");
        std::fs::create_dir_all(&native).unwrap();
        std::fs::write(
            native.join("SKILL.md"),
            "---\nname: review\ndescription: d\nwhen_to_use: the native one\n---\nSteps here.",
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let skills = host.skills().await;
        assert_eq!(skills.len(), 1, "the name resolves to exactly one skill");
        assert_eq!(skills[0].origin, taurus_skills::SkillOrigin::Taurus);
        assert_eq!(skills[0].when_to_use, "the native one");
    }

    #[tokio::test]
    async fn a_slash_command_runs_a_skill_written_for_another_client() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let skill = workspace.join(".claude/skills/speckit-specify");
        std::fs::create_dir_all(&skill).unwrap();
        // The shape spec-kit generates: no `when_to_use`, an `$ARGUMENTS`
        // placeholder, and the invocation flags spelled with hyphens.
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: speckit-specify\n\
             description: Create or update the feature specification.\n\
             user-invocable: true\ndisable-model-invocation: false\n---\n\
             ## User Input\n\n$ARGUMENTS\n\nBuild the spec.",
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let invocation = host
            .expand_command("/speckit-specify add a dark mode toggle")
            .await
            .expect("a leading /name is a command")
            .expect("and this skill exists");

        assert_eq!(invocation.name, "speckit-specify");
        assert!(invocation.prompt.contains("add a dark mode toggle"));
        assert!(invocation.prompt.contains("Build the spec."));
        assert!(!invocation.prompt.contains("$ARGUMENTS"));

        // Still model-invocable, so it stays in the catalog as well.
        assert!(host.system_prompt().await.contains("- speckit-specify:"));
        let offered: Vec<String> = host.commands().await.into_iter().map(|c| c.name).collect();
        assert!(
            offered.contains(&"speckit-specify".to_string()),
            "and offerable as a command: {offered:?}"
        );
    }

    #[tokio::test]
    async fn a_conversation_keeps_its_checklist_and_does_not_share_it() {
        use taurus_tools::view::{Step, StepState};

        let dir = TempDir::new().unwrap();
        let (host, _home) = host(&dir.path().canonicalize().unwrap());

        let board = host.plan_board("s1").await;
        board.set(vec![Step {
            text: "Add the token type".into(),
            state: StepState::Active,
            active_form: None,
        }]);

        // The same conversation, a message later.
        assert!(
            host.plan_board("s1")
                .await
                .reminder()
                .is_some_and(|r| r.contains("Add the token type")),
            "an unfinished plan has to survive the message that interrupted it"
        );
        // A different one, which must start empty however busy the first is.
        assert_eq!(host.plan_board("s2").await.reminder(), None);

        host.forget_plan("s1").await;
        assert_eq!(host.plan_board("s1").await.reminder(), None);
    }

    #[tokio::test]
    async fn the_slash_namespace_covers_sub_agents_too() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let (host, _home) = host(&workspace);
        host.reload().await;

        // Nothing was written: these are the built-ins, which is the roster a
        // machine with no agents directory has and the one people type first.
        let offered: Vec<(String, crate::command::CommandKind)> = host
            .commands()
            .await
            .into_iter()
            .map(|c| (c.name, c.kind))
            .collect();
        assert!(
            offered.contains(&("explorer".to_string(), crate::command::CommandKind::Agent)),
            "{offered:?}"
        );

        let invocation = host
            .expand_command("/explorer find every caller of build_agent")
            .await
            .expect("a leading /name is a command")
            .expect("and this agent exists");
        assert_eq!(invocation.name, "explorer");
        assert!(invocation.prompt.contains(taurus_core::SPAWN_TOOL));
        assert!(invocation
            .prompt
            .contains("find every caller of build_agent"));
    }

    #[tokio::test]
    async fn a_message_that_merely_starts_with_a_slash_is_left_alone() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let (host, _home) = host(&workspace);
        host.reload().await;

        assert!(host
            .expand_command("/usr/bin/env is portable")
            .await
            .is_none());
        assert!(host.expand_command("what does /etc hold?").await.is_none());
    }

    #[tokio::test]
    async fn a_skills_own_reference_file_is_readable_from_outside_the_workspace() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let (host, home) = host(&workspace);

        // A user-tier skill: it lives under the home directory, so every file
        // it bundles is outside the workspace the path guard confines reads to.
        let skill = home.path().join(".claude/skills/pdf-processing");
        std::fs::create_dir_all(skill.join("references")).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: pdf-processing\ndescription: Use when handling PDFs.\n---\n\
             See references/REFERENCE.md.",
        )
        .unwrap();
        std::fs::write(skill.join("references/REFERENCE.md"), "the reference text").unwrap();
        host.reload().await;

        let ctx = host.tool_context(CancellationToken::new()).await;
        let reference = skill.join("references/REFERENCE.md");

        let read = ReadFile
            .execute(
                serde_json::json!({ "path": reference.to_str().unwrap() }),
                &ctx,
            )
            .await
            .expect("a skill's own reference file must be readable");
        assert!(read.contains("the reference text"));

        // The allowance is for reading. Nothing about it lets the agent write
        // to a directory another client owns.
        let write = WriteFile
            .execute(
                serde_json::json!({
                    "path": reference.to_str().unwrap(),
                    "content": "rewritten",
                }),
                &ctx,
            )
            .await;
        assert!(
            matches!(write, Err(ToolError::OutsideWorkspace { .. })),
            "writes must stay in the workspace, got {write:?}"
        );
    }

    #[tokio::test]
    async fn a_workspace_can_retarget_a_provider_without_restating_it() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/providers.json"),
            r#"[{"id": "ollama", "base_url": "http://gpu-box:11434"}]"#,
        )
        .unwrap();

        let other = TempDir::new().unwrap();
        let (host, _home) = host(&other.path().canonicalize().unwrap());
        host.reload().await;
        let default_url = host.provider_config("ollama").await.unwrap().base_url;

        host.set_workspace(&workspace).await.unwrap();
        let overridden = host.provider_config("ollama").await.unwrap();
        assert_eq!(overridden.base_url, "http://gpu-box:11434");
        assert_ne!(overridden.base_url, default_url);
        // The kind came from the global layer; the workspace never said it.
        assert_eq!(overridden.kind, ProviderKind::Ollama);
    }

    #[tokio::test]
    async fn the_settings_editor_is_shown_the_global_layer_not_the_merged_one() {
        // The settings UI saves back whatever it was shown. Hand it the
        // effective list and this workspace's override would be written into
        // the global file, following the user into every other project.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/providers.json"),
            r#"[{"id": "ollama", "base_url": "http://gpu-box:11434"}]"#,
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let effective = host.provider_config("ollama").await.unwrap();
        assert_eq!(effective.base_url, "http://gpu-box:11434");

        let global = host.global_providers().await;
        let entry = global.iter().find(|p| p.id == "ollama").unwrap();
        assert_ne!(
            entry.base_url, "http://gpu-box:11434",
            "the workspace override leaked into the editable global layer"
        );
    }

    #[tokio::test]
    async fn the_model_last_used_is_remembered_per_workspace() {
        let first_dir = TempDir::new().unwrap();
        let first = first_dir.path().canonicalize().unwrap();
        let second_dir = TempDir::new().unwrap();
        let second = second_dir.path().canonicalize().unwrap();

        let (host, _home) = host(&first);
        host.set_workspace(&first).await.unwrap();
        host.remember_session("ollama", "qwen-coder").await;

        // A workspace with no memory of its own inherits the global default.
        host.set_workspace(&second).await.unwrap();
        assert_eq!(
            host.settings().await.last_model.as_deref(),
            Some("qwen-coder")
        );
        host.remember_session("ollama", "gemma3").await;

        // Going back must restore that workspace's model, not the newest one.
        host.set_workspace(&first).await.unwrap();
        assert_eq!(
            host.settings().await.last_model.as_deref(),
            Some("qwen-coder")
        );
    }

    #[tokio::test]
    async fn a_broken_workspace_config_is_reported_rather_than_swallowed() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(workspace.join(".taurus/providers.json"), "{ not json").unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let problems = host.problems().await;
        assert!(
            problems
                .iter()
                .any(|p| p.source == ProblemSource::Providers
                    && p.message.contains("providers.json")),
            "a config file the user must fix has to reach the UI: {problems:?}"
        );
        // And it must reach the screen that can fix it, rather than the skills
        // list, which is where an untagged list of problems used to put it.
        assert!(
            !host
                .problems_from(&[ProblemSource::Skills, ProblemSource::Mcp])
                .await
                .iter()
                .any(|p| p.message.contains("providers.json")),
            "a provider problem must not be reported as a skill problem"
        );
        assert_eq!(
            host.problems_from(&[ProblemSource::Providers]).await.len(),
            1
        );
        // And the global layer still works.
        assert!(!host.providers().await.is_empty());
    }

    #[tokio::test]
    async fn checkpoints_live_in_the_config_home_and_follow_the_workspace() {
        // Not in the project: a checkpoint holds the contents of files in the
        // workspace, and kept there it would be committed by accident.
        let first_dir = TempDir::new().unwrap();
        let first = first_dir.path().canonicalize().unwrap();
        let second_dir = TempDir::new().unwrap();
        let second = second_dir.path().canonicalize().unwrap();

        let (host, _home) = host(&first);
        host.set_workspace(&first).await.unwrap();

        let file = first.join("a.txt");
        std::fs::write(&file, "original").unwrap();
        let recorder = host
            .checkpoints()
            .await
            .begin_turn("s1", &first, "change a.txt");
        recorder.capture(&file).await;
        std::fs::write(&file, "changed").unwrap();

        assert!(
            !first.join(".taurus/checkpoints").exists(),
            "checkpoints must not be written into the project"
        );

        host.checkpoints()
            .await
            .rewind("s1", &first, 1, false)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");

        // A different workspace is a different log, even for the same id.
        host.set_workspace(&second).await.unwrap();
        assert!(host.checkpoints().await.turns("s1").unwrap().is_empty());

        // But the conversation's history is not gone, only somewhere the open
        // workspace does not look. Everything reading a conversation asks by
        // the folder it belongs to, which is what `checkpoints_for` is for:
        // resolved against the open one instead, the Changes drawer reported
        // nothing to undo for a conversation that had rewritten the project.
        let turns = host.checkpoints_for(&first).turns("s1").unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].prompt, "change a.txt");
    }

    #[tokio::test]
    async fn a_workspace_that_is_not_a_directory_is_refused() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let (host, _home) = host(&dir.path().canonicalize().unwrap());
        assert!(host.set_workspace(&file).await.is_err());
    }

    #[tokio::test]
    async fn resolve_model_reports_a_missing_provider_clearly() {
        let dir = TempDir::new().unwrap();
        let (host, _home) = host(&dir.path().canonicalize().unwrap());
        let err = host.resolve_model(Some("nonexistent"), None).await;
        // Falls back to the configured default rather than failing outright,
        // but an unreachable backend must still produce a readable message.
        if let Err(message) = err {
            assert!(!message.is_empty());
        }
    }

    #[tokio::test]
    async fn changing_the_workspace_does_not_write_the_real_user_config() {
        // Regression: settings are persisted as a side effect of picking a
        // workspace, so tests must be pointed somewhere harmless first.
        let _home = isolated_home();
        let home = std::env::var_os(crate::config::HOME_ENV).expect("config must be isolated");
        let real_home = directories::BaseDirs::new().map(|d| d.home_dir().join(".taurus"));
        assert_ne!(
            Some(PathBuf::from(&home)),
            real_home,
            "tests are still pointed at the real config directory"
        );
        assert_eq!(crate::config::home_dir(), PathBuf::from(home));
    }

    #[tokio::test]
    async fn a_backend_with_no_model_listing_falls_back_to_its_configured_default() {
        // An Azure APIM route commonly exposes /chat/completions and nothing
        // else. Before `default_model` was consulted this was simply unusable
        // without passing --model on every single invocation.
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/providers.json"),
            r#"[{
                "id": "apim",
                "kind": "open_ai_compatible",
                "base_url": "http://127.0.0.1:1",
                "api_prefix": "/openai/v1",
                "default_model": "gpt-4o"
            }]"#,
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let (provider, model) = host
            .resolve_model(Some("apim"), None)
            .await
            .expect("an unreachable listing must not be fatal when a default is configured");
        assert_eq!(provider, "apim");
        assert_eq!(model, "gpt-4o");
    }

    #[tokio::test]
    async fn a_backend_with_neither_a_listing_nor_a_default_says_how_to_fix_it() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".taurus")).unwrap();
        std::fs::write(
            workspace.join(".taurus/providers.json"),
            r#"[{"id": "apim", "kind": "open_ai_compatible", "base_url": "http://127.0.0.1:1"}]"#,
        )
        .unwrap();

        let (host, _home) = host(&workspace);
        host.reload().await;

        let err = host.resolve_model(Some("apim"), None).await.unwrap_err();
        assert!(err.contains("default_model"), "{err}");
        assert!(err.contains("--model"), "{err}");
    }

    #[tokio::test]
    async fn an_explicit_model_short_circuits_the_backend_query() {
        let dir = TempDir::new().unwrap();
        let (host, _home) = host(&dir.path().canonicalize().unwrap());
        let (provider, model) = host
            .resolve_model(None, Some("some-model"))
            .await
            .expect("an explicit model needs no backend round trip");
        assert_eq!(model, "some-model");
        assert!(!provider.is_empty());
    }
}
