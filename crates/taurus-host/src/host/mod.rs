//! Assembly of a running harness, independent of how it is driven.
//!
//! Both frontends — the desktop app and the CLI — need the same things: a
//! workspace, a permission engine bound to it, a tool registry carrying
//! built-ins plus skills plus MCP tools, and an [`Agent`] configured with the
//! system prompt those imply. Building that twice is how the two would drift,
//! so it is built once here and the frontends supply only what genuinely
//! differs: how to ask the user for permission, and where skill proposals go.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;

use taurus_agents::catalog::{AgentCatalog, SharedAgentCatalog};
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
use taurus_tools::builtin::present::{
    AskUser, OpenFile, ShowChart, ShowFlow, ShowSequence, ShowTable,
};
use taurus_tools::PlanBoard;
use taurus_tools::{
    Asker, CheckpointStore, PermissionEngine, PermissionPrompt, ToolContext, ToolRegistry,
};

use crate::command;
use crate::config::{self, ProviderConfig, ProviderKind, Scope, Settings, Theme};
use crate::document::{fingerprint, Document, Saved, MAX_DOCUMENT_BYTES};
use crate::freshness::Freshness;
use crate::instructions::{self, Instructions};
use crate::mcp_view::{LayerOf, McpServerView};
use crate::memory;
use crate::notebook;
use crate::problem::{self, Problem, ProblemSource};
use crate::prompt;
use crate::secrets;
use crate::sessions::SubagentLogs;
use crate::theme::{self, CustomTheme};
use crate::usage;

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
/// Every one is here for the same reason: a sub-agent must not have them.
/// `spawn_subagent` is the depth cap, `update_plan`'s checklist belongs to the
/// turn that wrote it, and the rest speak to the person watching this
/// conversation, which a delegate does not have.
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
    taurus_tools::builtin::present::SHOW_FLOW_TOOL,
    taurus_tools::builtin::present::ASK_USER_TOOL,
    taurus_tools::builtin::present::OPEN_FILE_TOOL,
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
    /// The conversation's "nobody is here" switch, where it has one.
    ///
    /// Held by the caller rather than built here, because it belongs to the
    /// conversation rather than to the turn: it is set by somebody about to
    /// walk away, often while the turn is already running. See
    /// [`taurus_tools::Asking::unattended`].
    ///
    /// `None` for a caller that answers its own prompts — the CLI has a policy
    /// for that, and a piped run has an asker that answers nothing.
    pub unattended: Option<Arc<AtomicBool>>,
}

/// The active theme, and what resolving it depended on. See
/// [`Host::active_theme`].
struct ResolvedTheme {
    id: String,
    /// The workspace whose layer it was read with, which trust decides.
    reading: Option<PathBuf>,
    seen: Freshness,
    theme: Option<CustomTheme>,
    problems: Vec<String>,
}

/// The providers built so far, and how many times they have been forgotten.
/// See [`Host::provider`].
#[derive(Default)]
struct BuiltProviders {
    generation: u64,
    providers: std::collections::HashMap<String, Arc<dyn Provider>>,
}

pub struct Host {
    workspace: RwLock<PathBuf>,
    /// What reads tabular files.
    ///
    /// One per host rather than one per call, so that the single line naming a
    /// concrete engine is in `Host::new` and nowhere else. Everything from the
    /// tools to the commands takes it as `dyn Engine` — see
    /// [`taurus_data::engine`] for why that is worth the indirection while the
    /// choice is still open.
    engine: Arc<dyn taurus_data::Engine>,
    providers: RwLock<Vec<ProviderConfig>>,
    /// Each provider, built once from its config and kept.
    ///
    /// Building one resolves its API key, and on macOS, Windows and a Linux
    /// desktop that is a call into the OS credential store: tens of
    /// milliseconds when it is unlocked, and as long as a dialog stays open
    /// when it is not. Every message asks for its conversation's provider, so
    /// building one per call put that read on every turn. See
    /// [`Self::provider`].
    built: std::sync::Mutex<BuiltProviders>,
    settings: RwLock<Settings>,
    catalog: SharedCatalog,
    /// The standing brief for this machine and this workspace. Held rather
    /// than read per turn, because reading it is six `stat`s and a handful of
    /// file reads and a turn is not the place to pay for them again — but
    /// checked per turn, which is one `stat` each and is. See
    /// [`Self::refresh_for_turn`].
    instructions: RwLock<Vec<Instructions>>,
    /// What the held instructions were read from, so a turn can tell in a few
    /// `stat`s whether reading them again would produce anything different.
    instructions_seen: RwLock<Freshness>,
    /// The sub-agent roster. Seeded with the built-ins so `explorer` and
    /// `worker` work before anything has been scanned.
    ///
    /// Shared rather than owned so `propose_agent` can check a proposed name
    /// against the roster as it stands now. A turn delegates against a frozen
    /// snapshot; a duplicate check has to see the live set.
    agents: SharedAgentCatalog,
    /// Each agent's `(provider, model)`, resolved when the roster is scanned.
    /// Resolved there rather than per turn because resolving can build a
    /// provider, and the first build of each reads its key out of the OS
    /// keychain — see [`Self::built`]. That is also why a turn checks the
    /// roster's fingerprint before rescanning it. See
    /// [`Self::refresh_for_turn`].
    agent_models: RwLock<ModelOverrides>,
    /// What the held roster was scanned from. Compared per turn; the scan it
    /// guards parses every agent file, cross-checks each one's tools, and can
    /// reach the OS keychain, so learning that nothing moved has to be cheaper
    /// than that by a wide margin.
    agents_seen: RwLock<Freshness>,
    /// What the held skill catalog was scanned from.
    ///
    /// The same bargain as the roster above: discovery parses a `SKILL.md` for
    /// every skill installed and validates each one's frontmatter, so a turn
    /// asks a `stat` per skill whether that work would produce anything
    /// different. Without this a skill written into `.taurus/skills` was
    /// invisible until the app was restarted — the one piece of config that
    /// still worked that way after agents and instructions stopped.
    skills_seen: RwLock<Freshness>,
    /// The same, for the two hook files. Cheapest of the three to check and to
    /// re-read, and the promise it keeps is the one the hooks documentation
    /// already made: a hook edited in an editor takes effect on the next
    /// message rather than the next launch.
    hooks_seen: RwLock<Freshness>,
    /// The active theme as last resolved, and the files it was resolved from.
    ///
    /// It rides on every status push, and resolving it reads the theme file
    /// and reads and base64-encodes its logo — up to 256 KB — for an answer
    /// that moves when somebody edits a theme. Held, and checked with a `stat`
    /// per file: the bargain the roster and the skills make. See
    /// [`Self::active_theme`].
    theme_seen: RwLock<Option<ResolvedTheme>>,
    /// The one index refresh that may be running for this workspace.
    ///
    /// Held here because all three things that start one pass through this
    /// struct: the warm-up a turn kicks off, `search_code` when the model
    /// reaches for it, and **Build index now**. See [`taurus_index::inflight`].
    indexing: Arc<taurus_index::Indexing>,
    /// The commands running in the background.
    ///
    /// On the host rather than beside a session because that is what they are:
    /// a build started in one turn is read in another, and a dev server
    /// outlives the conversation that started it. Ended when the workspace
    /// changes, and by the window on its way out — see
    /// [`taurus_tools::Jobs::stop_all`].
    jobs: Arc<taurus_tools::Jobs>,
    /// What the last command read of the workspace, shared by every turn in
    /// it. See [`taurus_tools::SweepCache`]: held here, a turn's first command
    /// reads only what changed since the last turn's commands, where one per
    /// turn read the whole workspace again first. Cleared when the workspace
    /// changes.
    sweeps: Arc<taurus_tools::SweepCache>,
    /// Shared rather than owned so sub-agents can be handed the same registry:
    /// it has no spawn tool, which is what caps delegation depth.
    registry: Arc<RwLock<ToolRegistry>>,
    /// The user's configured hooks, rebuilt on every reload.
    ///
    /// Held rather than read per call for the reason the registry is: a tool
    /// call would otherwise re-read and re-merge two files, and matching a hook
    /// is meant to cost a string comparison.
    hooks: RwLock<Arc<taurus_hooks::HookRunner>>,
    permissions: RwLock<Arc<PermissionEngine>>,
    mcp: McpManager,
    /// Held for the whole of an MCP reload, so two cannot interleave.
    ///
    /// A reload shuts every server down, spends seconds starting them again,
    /// then swaps their tools into the registry — and the MCP panel starts one
    /// on every save. Two at once let the second's shutdown drop connections
    /// the first had just made, while the first went on to register tools
    /// pointing at them: tools that failed on every call, under a panel that
    /// said connected.
    ///
    /// The local half never takes this. It carries the MCP tools across at the
    /// moment it swaps the registry — see [`Self::reload_local`] — so a
    /// settings save does not wait behind a server that is still starting.
    mcp_reload: tokio::sync::Mutex<()>,
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
        let permissions = Arc::new(
            PermissionEngine::new(&workspace, config::home_dir(), prompts.create())
                .with_workspace_rules(crate::trust::is_trusted(&workspace)),
        );
        // Both layers are read here and again on every `reload`, because the
        // workspace layer changes underneath a running host.
        let (providers, _) = config::load_providers(Some(&workspace));
        let settings = config::load_settings(Some(&workspace));
        Self {
            providers: RwLock::new(providers),
            built: std::sync::Mutex::new(BuiltProviders::default()),
            settings: RwLock::new(settings),
            workspace: RwLock::new(workspace),
            // The one line in the harness that names a data engine. Everything
            // downstream holds it as `dyn Engine`.
            engine: Arc::new(taurus_data::DataFusionEngine::new()),
            catalog: Arc::new(RwLock::new(SkillCatalog::default())),
            jobs: Arc::new(taurus_tools::Jobs::new()),
            sweeps: Arc::new(taurus_tools::SweepCache::new()),
            indexing: Arc::new(taurus_index::Indexing::new()),
            instructions: RwLock::new(Vec::new()),
            instructions_seen: RwLock::new(Freshness::default()),
            agents: Arc::new(RwLock::new(AgentCatalog::default())),
            agent_models: RwLock::new(ModelOverrides::new()),
            agents_seen: RwLock::new(Freshness::default()),
            skills_seen: RwLock::new(Freshness::default()),
            hooks_seen: RwLock::new(Freshness::default()),
            theme_seen: RwLock::new(None),
            registry: Arc::new(RwLock::new(ToolRegistry::with_builtins())),
            hooks: RwLock::new(Arc::new(taurus_hooks::HookRunner::default())),
            permissions: RwLock::new(permissions),
            // Handed the keychain, so a server that wants OAuth can be signed
            // in to. See `secrets::Keychain`.
            mcp: McpManager::with_vault(Arc::new(crate::secrets::Keychain)),
            mcp_reload: tokio::sync::Mutex::new(()),
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
    ///
    /// Two halves, and callers that care about how soon the first one lands
    /// should run them separately — see [`Host::reload_local`].
    pub async fn reload(&self) {
        self.reload_local().await;
        self.reload_mcp().await;
    }

    /// Everything a reload does except start an MCP server.
    ///
    /// The split exists because the two halves cost wildly different amounts.
    /// This one reads a handful of directories and finishes in milliseconds;
    /// [`Host::reload_mcp`] spawns child processes and waits on them, which is
    /// seconds when a server is an `npx` package being unpacked. Running them
    /// together made the second the price of the first, and the window's very
    /// first `get_status` waits on the first — so a user with three MCP servers
    /// opened the app onto a shell with no providers, no model picker and no
    /// rail until every one of those servers had answered.
    ///
    /// Startup calls the two in order, marking itself loaded in between. The
    /// MCP tools that are running survive this half — it carries them across
    /// rather than rebuilding them — so a change that cannot affect a server
    /// calls this alone, and [`Host::reload`] is for when the servers should
    /// restart too.
    pub async fn reload_local(&self) {
        let workspace = self.workspace.read().await.clone();

        let (providers, provider_problems) = config::load_providers(Some(&workspace));
        let mut problems = Problem::tag(ProblemSource::Providers, provider_problems);
        *self.providers.write().await = providers;
        self.forget_providers();
        *self.settings.write().await = config::load_settings(Some(&workspace));

        // Through the same two loaders a turn calls, so a reload and a turn
        // cannot come to disagree about what is installed — and so that both
        // record the fingerprint the turn will check against.
        problems.extend(self.load_skills(&workspace).await);
        problems.extend(self.load_hooks(&workspace).await);

        // Re-read on every reload for the reason providers are: these files
        // belong to the workspace, and a switch changes which of them exist.
        // Through the same call a turn makes, so the two cannot come to
        // disagree about what an instruction file is.
        problems.extend(self.load_instructions(&workspace).await);

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

        // Reading tabular data. Registered unconditionally, unlike the two
        // blocks above: there is nothing to configure and nothing to be
        // unreachable, so there is no state in which advertising these costs
        // the model a turn to discover. What they need is a folder, and every
        // workspace has one.
        //
        // Into the shared registry rather than per turn, for the reason
        // `search_code` is: a delegate sent to work out what is in an
        // unfamiliar export is exactly who wants them, and the per-turn set is
        // the one sub-agents do not get.
        {
            let dir = self.data_dir_for(&workspace);
            registry.register(Arc::new(taurus_data::LoadDataset::new(
                self.engine.clone(),
                dir.clone(),
            )));
            registry.register(Arc::new(taurus_data::ProfileDataset::new(
                self.engine.clone(),
                dir.clone(),
            )));
            registry.register(Arc::new(taurus_data::QueryData::new(
                self.engine.clone(),
                dir.clone(),
            )));
            registry.register(Arc::new(taurus_data::RunRecipe::new(
                self.engine.clone(),
                dir,
                &workspace,
            )));
        }

        // The notebook. Into the shared registry rather than per turn, for the
        // reason the data tools give: a delegate asked what the design says is
        // exactly who wants it, and the per-turn set is the one sub-agents do not
        // get. Nothing to configure and nothing to be unreachable — a person with
        // no notes gets an empty read and a message saying so.
        registry.register(Arc::new(notebook::ReadNote::new(&workspace)));
        registry.register(Arc::new(notebook::WriteNote::new(&workspace)));
        registry.register(Arc::new(notebook::OpenNote::new(&workspace)));

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
                        let mut search = taurus_index::SearchCode::new(
                            provider.clone(),
                            &embedding_model,
                            taurus_index::index_dir(
                                &config::home_dir(),
                                &crate::sessions::workspace_key(&workspace),
                            ),
                        );
                        // The second stage, when one is configured. A provider
                        // that cannot be resolved is a problem worth reporting
                        // but not one worth withholding the search over: the
                        // tool works without it, and taking `search_code` away
                        // because its optional half is misconfigured would cost
                        // far more than the reordering was worth.
                        search = search.with_indexing(self.indexing.clone());
                        match self.rerank_for(&provider).await {
                            Ok(Some((reranker, model))) => {
                                info!(model = %model, "reranking enabled");
                                search = search.with_rerank(reranker, model);
                            }
                            Ok(None) => {}
                            Err(message) => problems.push(Problem {
                                source: ProblemSource::Providers,
                                message,
                            }),
                        }
                        registry.register(Arc::new(search));
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
        } else if !self.settings.read().await.rerank_model.trim().is_empty() {
            // Stage two of a feature whose stage one is off. Unreachable
            // through the panel, which only offers the field once an embedding
            // model is named, but `settings.json` is hand-edited and this is
            // exactly the edit somebody makes on the way to turning search on.
            // Saying nothing would leave them waiting for a reordering of
            // results that are never produced.
            problems.push(Problem {
                source: ProblemSource::Providers,
                message: "a reranking model is set but no embedding model is, so there is no \
                          search for it to reorder. Name one under Settings → Search."
                    .into(),
            });
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

        // Last, so it applies to everything this half assembled — built-ins,
        // skill tools, web — rather than to whichever of them happened to
        // register before the setting was read.
        let disabled = self.settings.read().await.disabled_tools.clone();
        // Two families are held back rather than applied here, and for the same
        // reason: naming one of them must not be reported as naming a tool that
        // does not exist, which is the one message that would send someone
        // looking for a typo in a line that works.
        //
        // The per-turn tools are not in this registry to be removed from — a
        // turn adds them to its own copy, and takes them away there. The MCP
        // tools are not here *yet* — they are carried across at the swap below,
        // through the same list — and `reload_mcp` never registers one the
        // settings disable, so the effect is identical; what would differ is a
        // warning about the user's own working config, appearing or not
        // depending on whether a server happened to be up this second.
        let here: Vec<String> = disabled
            .iter()
            .filter(|name| {
                !PER_TURN_TOOLS.contains(&name.as_str()) && !taurus_mcp::is_mcp_tool(name)
            })
            .cloned()
            .collect();
        problems.extend(Problem::tag(
            ProblemSource::Tools,
            disable(&mut registry, &here),
        ));

        // After the registry is finished, and deliberately so: an agent scoped
        // to tools the user has since disabled is exactly the case this catches.
        // With the MCP tools that are running, which the swap below carries
        // across: an agent scoped to a server's tools must not be refused by a
        // reload that leaves the server up.
        let running =
            |name: &str| taurus_mcp::is_mcp_tool(name) && !disabled.iter().any(|off| off == name);
        let mut available: Vec<String> = registry.names().map(str::to_string).collect();
        available.extend(
            self.registry
                .read()
                .await
                .names()
                .filter(|name| running(name))
                .map(str::to_string),
        );
        problems.extend(self.load_agents(&workspace, &available).await);

        // The MCP tools come across from the registry this replaces. They are
        // `reload_mcp`'s to change, and rebuilding without them would leave
        // every caller choosing between no MCP tools and a restart of every
        // server. Read under the write lock that swaps, so a reconnect finishing
        // alongside cannot have its new tools replaced with the old ones; and
        // filtered by the settings just read, so a tool switched off since does
        // not come back.
        let mut live = self.registry.write().await;
        let carried: Vec<_> = live
            .names()
            .filter(|name| running(name))
            .filter_map(|name| live.get(name))
            .collect();
        for tool in carried {
            registry.register(tool);
        }
        *live = registry;
        drop(live);

        // Every source but MCP's, which `reload_mcp` reports and this half
        // never looks at. Replacing the list wholesale would clear a server's
        // problem until something next reconnected it.
        let mut held = self.problems.write().await;
        problems.extend(held.drain(..).filter(|p| p.source == ProblemSource::Mcp));
        *held = problems;
    }

    /// Reads the standing brief and installs it, returning what to report.
    ///
    /// One path for the reload and the per-turn check, so the fingerprint that
    /// decides whether to read again is always taken from the files the read
    /// actually depended on — sources *and* the imports they pulled in, which
    /// are only knowable by having read them.
    ///
    /// Stamped after the read rather than before, which is the opposite of what
    /// [`Self::load_agents`] does, and for a reason the roster does not have: a
    /// newly added import is not in any earlier list, so a fingerprint taken
    /// beforehand could not name it and would never settle. The cost is a
    /// window of one read — a file edited in the microseconds between being
    /// read and being stamped waits for its next change to be noticed. That is
    /// the same class of blind spot [`crate::freshness`] already documents, and
    /// this window is at a turn boundary rather than across a whole turn.
    async fn load_instructions(&self, workspace: &Path) -> Vec<Problem> {
        let loaded = instructions::load(instructions::sources(Some(workspace)));
        info!(files = loaded.instructions.len(), "instructions loaded");
        // Files plus directories. The named briefs and whatever they import are
        // watched by name; Copilot's scoped instructions live in a folder, so
        // that is watched by rule — otherwise the first file written into an
        // empty `.github/instructions` would be one nothing was looking for.
        *self.instructions_seen.write().await =
            Freshness::of_files(loaded.read.iter().map(PathBuf::as_path)).and(Freshness::of_dirs(
                instructions::scoped_dirs(Some(workspace))
                    .iter()
                    .map(PathBuf::as_path),
                instructions::SCOPED_SUFFIX,
                true,
            ));
        *self.instructions.write().await = loaded.instructions;
        Problem::tag(ProblemSource::Instructions, loaded.problems)
    }

    /// Scans every skill directory and installs what it finds.
    ///
    /// The catalog is shared rather than owned — the `load_skill`,
    /// `run_skill_script` and `propose_skill` tools all hold the same handle —
    /// so replacing its contents is enough. Nothing here rebuilds the tool
    /// registry, which is what makes this safe to call at a turn boundary
    /// rather than only from a full reload.
    async fn load_skills(&self, workspace: &Path) -> Vec<Problem> {
        let sources = config::skill_sources(Some(workspace));
        // Taken before the scan, for the reason `load_instructions` takes its
        // own before reading: a skill saved while this runs has to leave the
        // fingerprint stale rather than be recorded as already seen.
        *self.skills_seen.write().await = skill_freshness(&sources);

        // On a blocking thread: a scan lists every source directory and reads
        // and validates a `SKILL.md` for every skill installed, and it runs at
        // each turn boundary where the library moved.
        let scanned = {
            let sources = sources.clone();
            tokio::task::spawn_blocking(move || SkillCatalog::discover(&sources)).await
        };
        let (catalog, skill_problems) =
            scanned.unwrap_or_else(|_| SkillCatalog::discover(&sources));
        info!(
            skills = catalog.len(),
            problems = skill_problems.len(),
            "skills loaded"
        );
        *self.catalog.write().await = catalog;
        Problem::tag(
            ProblemSource::Skills,
            skill_problems.iter().map(|p| p.to_string()),
        )
    }

    /// Re-reads both hook files and installs the runner they describe.
    async fn load_hooks(&self, workspace: &Path) -> Vec<Problem> {
        *self.hooks_seen.write().await = hook_freshness(workspace);

        let (hooks, hook_problems) = config::load_hooks(Some(workspace));
        *self.hooks.write().await = Arc::new(hooks);
        Problem::tag(ProblemSource::Hooks, hook_problems)
    }

    /// Rescans the skill directories without touching anything else.
    ///
    /// What the Skills drawer calls, for the reason [`Host::rescan_agents`]
    /// exists: a drawer showing the catalog as it was at startup is not showing
    /// the feature working. Narrower than [`Host::reload`] — scanning a
    /// directory should not restart every MCP server.
    ///
    /// Answers whether what the shell shows about skills moved — the count, or
    /// a problem with one — so a caller pushes a status only when there is
    /// something new in it.
    pub async fn rescan_skills(&self) -> bool {
        let before = (
            self.skill_count().await,
            self.problem_text(ProblemSource::Skills).await,
        );
        let workspace = self.workspace.read().await.clone();
        let found = self.load_skills(&workspace).await;
        self.replace_problems(ProblemSource::Skills, found).await;
        before
            != (
                self.skill_count().await,
                self.problem_text(ProblemSource::Skills).await,
            )
    }

    /// The check a turn makes, asked for outside one.
    ///
    /// Same gate, same cost: a `stat` of each file, and a re-read only where
    /// something moved. It exists because a turn is not the only moment a
    /// person expects their edits to have landed — returning to the window
    /// after writing a skill in an editor is the other one, and polling for it
    /// would be a watcher with extra steps.
    ///
    /// Not safe mid-turn, for the reason the whole design is at turn
    /// boundaries: a turn runs against the brief, roster and catalog it started
    /// with. The caller is the one that knows whether a turn is running.
    ///
    /// Answers whether anything was re-read, so returning to the window pushes
    /// a status only when something on disk actually moved.
    pub async fn refresh_config(&self) -> bool {
        self.refresh_for_turn().await
    }

    /// Re-reads the config this turn is about to be built from.
    ///
    /// Called from the two places a turn begins — [`Self::expand_command`],
    /// which resolves a leading `/name` before anything else happens, and
    /// [`Self::build_agent`], which assembles everything else. Both, because a
    /// `/reviewer` typed at an agent written a moment ago is resolved before
    /// the agent is ever built, so refreshing only in the second would leave
    /// the new agent unreachable by the name it was given. Calling it twice
    /// costs a second `stat` of each file: the first call moves the
    /// fingerprint, and the second finds nothing to do.
    ///
    /// A turn boundary is the only moment any of this is safe to swap. Taurus does not
    /// watch these files: a watcher fires whenever an editor happens to save,
    /// which is routinely the middle of a running turn — and the roster a turn
    /// delegates against, and the brief it was given, have to be the ones it
    /// started with. Here nothing is in flight, and the turn about to start is
    /// the earliest one that could have used the change anyway. See
    /// [`crate::freshness`].
    ///
    /// Both halves are gated on a fingerprint rather than read outright,
    /// because both cost more than a `stat`: instructions are a handful of file
    /// reads and an import resolution, and a roster scan parses every agent
    /// file, cross-checks its tools, and can reach the OS keychain.
    ///
    /// Answers whether anything the status reports may have moved: a brief or
    /// a hook file re-read, or a roster or catalog whose count or problems
    /// changed.
    async fn refresh_for_turn(&self) -> bool {
        let workspace = self.workspace.read().await.clone();
        let mut moved = false;

        // Every fingerprint at once, on a blocking thread. Each is a `stat` per
        // file and a directory listing or two, which with a large skill library
        // is a few hundred blocking calls — made twice a turn, on the runtime
        // that carries every other command.
        let seen = self.instructions_seen.read().await.clone();
        let taken = {
            let (seen, workspace) = (seen.clone(), workspace.clone());
            tokio::task::spawn_blocking(move || TurnStamps::take(&seen, &workspace)).await
        };
        let now = taken.unwrap_or_else(|_| TurnStamps::take(&seen, &workspace));

        // Against the files the last read depended on, restated — not against
        // the source list. The two are different sets whenever a brief imports
        // anything, and comparing across them would never be equal, which is a
        // gate that is always open rather than a gate.
        if seen != now.instructions {
            let found = self.load_instructions(&workspace).await;
            self.replace_problems(ProblemSource::Instructions, found)
                .await;
            moved = true;
        }

        if *self.agents_seen.read().await != now.agents {
            moved |= self.rescan_agents().await;
        }

        if *self.skills_seen.read().await != now.skills {
            moved |= self.rescan_skills().await;
        }

        // Rebuilt rather than restated, unlike instructions: a hook file has no
        // imports, so the set to watch is knowable from the config layer — and
        // rebuilding it is what also notices the set *changing*, which is what
        // trusting a workspace does.
        if *self.hooks_seen.read().await != now.hooks {
            let found = self.load_hooks(&workspace).await;
            self.replace_problems(ProblemSource::Hooks, found).await;
            moved = true;
        }
        moved
    }

    /// One source's problems, as text: what a rescan compares to tell whether
    /// it moved anything the shell shows.
    async fn problem_text(&self, source: ProblemSource) -> Vec<String> {
        self.problems
            .read()
            .await
            .iter()
            .filter(|p| p.source == source)
            .map(|p| p.message.clone())
            .collect()
    }

    /// Swaps out every problem from one source, leaving the others alone.
    async fn replace_problems(&self, source: ProblemSource, found: Vec<Problem>) {
        let mut problems = self.problems.write().await;
        problems.retain(|p| p.source != source);
        problems.extend(found);
    }

    /// Rescans the agent directories without touching anything else.
    ///
    /// The whole authoring surface for an agent is a text editor, so a drawer
    /// that shows the catalog as it was at startup is not showing the feature
    /// working. This is what opening it calls. It is deliberately narrower than
    /// [`Host::reload`]: rescanning a directory should not restart every MCP
    /// server, which a full reload does.
    ///
    /// Answers whether the count or the agents' problems moved, the way
    /// [`Self::rescan_skills`] does and for the same reason.
    pub async fn rescan_agents(&self) -> bool {
        let before = (
            self.agents().await.len(),
            self.problem_text(ProblemSource::Agents).await,
        );
        let workspace = self.workspace.read().await.clone();
        let available: Vec<String> = self
            .registry
            .read()
            .await
            .names()
            .map(str::to_string)
            .collect();
        let found = self.load_agents(&workspace, &available).await;
        self.replace_problems(ProblemSource::Agents, found).await;
        before
            != (
                self.agents().await.len(),
                self.problem_text(ProblemSource::Agents).await,
            )
    }

    /// Discovers the roster, checks it against `available`, resolves its
    /// models, and installs it. Returns what to report.
    async fn load_agents(&self, workspace: &Path, available: &[String]) -> Vec<Problem> {
        let sources = config::agent_sources(Some(workspace));
        // Taken before the scan, for the reason `load_instructions` takes its
        // own before reading: a file saved while this runs has to leave the
        // fingerprint stale rather than be recorded as already seen.
        *self.agents_seen.write().await = agent_freshness(&sources);

        let (mut agents, errors) = AgentCatalog::discover(&sources);
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

    /// Everything that failed to load, tagged with where it came from.
    pub async fn problems(&self) -> Vec<Problem> {
        self.problems.read().await.clone()
    }

    /// Just the problems one screen is responsible for showing.
    pub async fn problems_from(&self, sources: &[ProblemSource]) -> Vec<Problem> {
        problem::of(&self.problems.read().await, sources)
    }
}

/// What each configured server's tools add to every request, estimated.
///
/// Attributed by the configured name rather than by splitting a tool name on
/// `__`, and to the *longest* configured name that matches. Both halves are
/// load-bearing: a server may be called `code__search`, so splitting would
/// charge `mcp__code__search__query` to a server called `code` — and matching
/// every prefix would charge it to `code__search` and `code` both, which is the
/// same tool counted twice in a total. The longest matching name is the one
/// that actually registered it.
///
/// A server with nothing registered is absent from the map rather than zero.
/// Which of those the panel shows is the caller's decision, taken from whether
/// the server connected: a connected server exposing no tools genuinely costs
/// nothing, and a disabled one has nothing to measure at all.
/// Adds a recipe's output to the Data pane's list, and says why when it cannot.
///
/// `None` when it was added, and when its name already belongs to another
/// file: that one is left alone on purpose — see the tool, which makes the same
/// call for the same reason.
fn list_output(dir: &Path, workspace: &Path, output: &Path) -> Option<String> {
    let shown = taurus_tools::path_guard::display(workspace, output);
    let name = taurus_data::catalog::suggest_name(output);
    if taurus_data::catalog::taken_by(dir, &name, &shown).is_some() {
        return None;
    }
    let format = match taurus_data::Format::of(output) {
        Ok(format) => format,
        Err(e) => {
            return Some(format!(
                "{shown} was written, but it is not in the list: {e}"
            ))
        }
    };
    taurus_data::catalog::register(
        dir,
        taurus_data::Dataset {
            name,
            path: shown.clone(),
            format,
        },
    )
    .err()
    .map(|e| format!("{shown} was written, but it could not be added to the list: {e}"))
}

fn mcp_schema_tokens(
    servers: &[String],
    advertised: &[taurus_provider::ToolDef],
) -> BTreeMap<String, u32> {
    let prefixes: Vec<(String, &str)> = servers
        .iter()
        .map(|name| (taurus_mcp::namespaced(name, ""), name.as_str()))
        .collect();

    let mut totals = BTreeMap::new();
    for def in advertised {
        let owner = prefixes
            .iter()
            .filter(|(prefix, _)| def.name.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len());
        if let Some((_, name)) = owner {
            *totals.entry(name.to_string()).or_insert(0) += usage::schema_cost(def);
        }
    }
    totals
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

/// The fingerprint of everywhere an agent could be defined.
///
/// `.md` and not `.agent.md`, because both spellings are read: Copilot's
/// doubled extension still ends in `.md`, and narrowing the suffix would leave
/// a Taurus-native file in the same folder unwatched.
/// A fingerprint over the `hooks.json` of every layer that would be read.
///
/// The directories rather than a fixed pair, because [`config::config_dirs`] is
/// trust-gated: an untrusted workspace contributes no layer at all, and
/// trusting one adds a file that was not being watched a moment ago. Built from
/// the config layer on both sides of the comparison, so that change registers
/// as a change.
fn hook_freshness(workspace: &Path) -> Freshness {
    let files: Vec<PathBuf> = config::config_dirs(Some(workspace))
        .iter()
        .map(|dir| taurus_hooks::config_file(dir))
        .collect();
    Freshness::of_files(files.iter().map(PathBuf::as_path))
}

/// A fingerprint over every `SKILL.md` a scan of these sources would read.
///
/// One level inside each source directory, because that is the layout: a source
/// holds a folder per skill and the folder holds the file. See
/// [`Freshness::of_child_dirs`] for why neither of the other two shapes fits.
fn skill_freshness(sources: &[taurus_skills::SkillSource]) -> Freshness {
    Freshness::of_child_dirs(
        sources.iter().map(|s| s.dir.as_path()),
        taurus_skills::catalog::SKILL_FILE,
    )
}

fn agent_freshness(sources: &[taurus_agents::AgentSource]) -> Freshness {
    Freshness::of_dirs(sources.iter().map(|s| s.dir.as_path()), ".md", false)
}

/// What a turn boundary compares with what is held: the brief restated, and
/// the roster, the skills and the hooks fingerprinted afresh. See
/// [`Host::refresh_for_turn`].
struct TurnStamps {
    instructions: Freshness,
    agents: Freshness,
    skills: Freshness,
    hooks: Freshness,
}

impl TurnStamps {
    /// A `stat` per file each and a directory listing or two, every one of them
    /// a blocking call — which is why a turn takes these on a blocking thread.
    fn take(instructions: &Freshness, workspace: &Path) -> Self {
        Self {
            instructions: instructions.refreshed(),
            agents: agent_freshness(&config::agent_sources(Some(workspace))),
            skills: skill_freshness(&config::skill_sources(Some(workspace))),
            hooks: hook_freshness(workspace),
        }
    }
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

mod catalog;
mod data;
mod mcp;
mod notes;
mod providers;
mod settings;
mod turn;
mod workspace;

#[cfg(test)]
mod tests;
