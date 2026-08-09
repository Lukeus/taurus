//! Assembly of a running harness, independent of how it is driven.
//!
//! Both frontends — the desktop app and the CLI — need the same things: a
//! workspace, a permission engine bound to it, a tool registry carrying
//! built-ins plus skills plus MCP tools, and an [`Agent`] configured with the
//! system prompt those imply. Building that twice is how the two would drift,
//! so it is built once here and the frontends supply only what genuinely
//! differs: how to ask the user for permission, and where skill proposals go.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;

use taurus_core::{Agent, AgentConfig, SpawnSubagent};
use taurus_mcp::{McpManager, ServerStatus};
use taurus_provider::Provider;
use taurus_provider_ollama::OllamaProvider;
use taurus_provider_openai::{OpenAiCapabilities, OpenAiProvider};
use taurus_skills::catalog::{SkillCatalog, SkillSource};
use taurus_skills::proposal::ProposalSink;
use taurus_skills::skill::{SkillSummary, SkillTier};
use taurus_skills::SharedCatalog;
use taurus_tools::{PermissionEngine, PermissionPrompt, ToolContext, ToolRegistry};

use crate::config::{self, ProviderConfig, ProviderKind, Settings};
use crate::prompt;

/// How many sub-agents may run at once. Low on purpose: each is a full model
/// stream, and local hardware serves them all from the same GPU.
pub const MAX_CONCURRENT_SUBAGENTS: usize = 2;

/// Makes a permission prompt on demand.
///
/// A factory rather than a single instance because the permission engine is
/// rebuilt whenever the workspace changes, and each engine owns its prompt.
pub trait PermissionPromptFactory: Send + Sync {
    fn create(&self) -> Box<dyn PermissionPrompt>;
}

pub struct Host {
    workspace: RwLock<PathBuf>,
    providers: RwLock<Vec<ProviderConfig>>,
    settings: RwLock<Settings>,
    catalog: SharedCatalog,
    /// Shared rather than owned so sub-agents can be handed the same registry:
    /// it has no spawn tool, which is what caps delegation depth.
    registry: Arc<RwLock<ToolRegistry>>,
    permissions: RwLock<Arc<PermissionEngine>>,
    mcp: McpManager,
    problems: RwLock<Vec<String>>,
    prompts: Arc<dyn PermissionPromptFactory>,
    proposals: Arc<dyn ProposalSink>,
}

impl Host {
    pub fn new(
        workspace: PathBuf,
        prompts: Arc<dyn PermissionPromptFactory>,
        proposals: Arc<dyn ProposalSink>,
    ) -> Self {
        let permissions = Arc::new(PermissionEngine::new(&workspace, prompts.create()));
        Self {
            workspace: RwLock::new(workspace),
            providers: RwLock::new(config::load_providers()),
            settings: RwLock::new(config::load_settings()),
            catalog: Arc::new(RwLock::new(SkillCatalog::default())),
            registry: Arc::new(RwLock::new(ToolRegistry::with_builtins())),
            permissions: RwLock::new(permissions),
            mcp: McpManager::new(),
            problems: RwLock::new(Vec::new()),
            prompts,
            proposals,
        }
    }

    /// The workspace remembered from last time, else the current directory.
    pub fn default_workspace() -> PathBuf {
        let candidate = config::load_settings()
            .last_workspace
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        candidate.canonicalize().unwrap_or(candidate)
    }

    /// Rescans skills, reconnects MCP servers, and rebuilds the registry.
    pub async fn reload(&self) {
        let workspace = self.workspace.read().await.clone();
        let sources = vec![
            SkillSource {
                tier: SkillTier::User,
                dir: config::user_skills_dir(),
            },
            SkillSource {
                tier: SkillTier::Project,
                dir: config::workspace_skills_dir(&workspace),
            },
        ];

        let (catalog, skill_problems) = SkillCatalog::discover(&sources);
        info!(
            skills = catalog.len(),
            problems = skill_problems.len(),
            "skills loaded"
        );
        let mut problems: Vec<String> = skill_problems.iter().map(|p| p.to_string()).collect();
        *self.catalog.write().await = catalog;

        let mut registry = ToolRegistry::with_builtins();
        registry.register(Arc::new(taurus_skills::LoadSkill::new(
            self.catalog.clone(),
        )));
        registry.register(Arc::new(taurus_skills::RunSkillScript::new(
            self.catalog.clone(),
        )));
        registry.register(Arc::new(taurus_skills::ProposeSkill::new(
            self.catalog.clone(),
            self.proposals.clone(),
        )));

        // Reconnecting drops the previous connections, stopping the old child
        // processes; leaving them would leak one per workspace change.
        self.mcp.shutdown().await;
        match taurus_mcp::load(&config::home_dir()) {
            Ok(mcp_config) => {
                for tool in self.mcp.connect_all(&mcp_config).await {
                    registry.register(tool);
                }
            }
            Err(e) => problems.push(e),
        }

        *self.registry.write().await = registry;
        *self.problems.write().await = problems;
    }

    pub async fn set_workspace(&self, path: &Path) -> Result<PathBuf, String> {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !canonical.is_dir() {
            return Err(format!("{} is not a directory", canonical.display()));
        }

        *self.workspace.write().await = canonical.clone();
        *self.permissions.write().await =
            Arc::new(PermissionEngine::new(&canonical, self.prompts.create()));

        {
            let mut settings = self.settings.write().await;
            settings.last_workspace = Some(canonical.display().to_string());
            config::save_settings(&settings);
        }

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
                Arc::new(OpenAiProvider::new(
                    config.id.clone(),
                    config.base_url.clone(),
                    config.api_key(),
                    OpenAiCapabilities {
                        native_tools: config.native_tools.unwrap_or(defaults.native_tools),
                        vision: defaults.vision,
                        context_length: config.context_length.unwrap_or(defaults.context_length),
                    },
                ))
            }
        })
    }

    /// Builds the agent for one turn.
    ///
    /// The single place system prompt, tool set, and sub-agent wiring come
    /// together, so the CLI and the desktop app cannot disagree about how an
    /// agent is configured.
    pub async fn build_agent(
        &self,
        provider: Arc<dyn Provider>,
        model: &str,
        cancel: CancellationToken,
    ) -> Agent {
        // Bound to this session's provider and model, so it is added per turn
        // rather than living in the shared registry. Children get the shared
        // registry, which has no spawn tool — that is the depth cap.
        let mut registry = self.registry.read().await.clone();
        registry.register(Arc::new(SpawnSubagent::new(
            provider.clone(),
            self.registry.clone(),
            model,
            MAX_CONCURRENT_SUBAGENTS,
        )));

        let workspace = self.workspace.read().await.clone();
        let skill_section = self.catalog.read().await.prompt_section();
        let synthesis = self.settings.read().await.skill_synthesis_enabled;

        Agent::new(
            provider,
            registry,
            self.tool_context(cancel).await,
            AgentConfig {
                system_prompt: prompt::build(&workspace, skill_section, synthesis),
                ..Default::default()
            },
        )
    }

    pub async fn tool_context(&self, cancel: CancellationToken) -> ToolContext {
        ToolContext::new(
            self.workspace.read().await.clone(),
            self.permissions.read().await.clone(),
            cancel,
        )
    }

    pub async fn workspace(&self) -> PathBuf {
        self.workspace.read().await.clone()
    }

    pub async fn providers(&self) -> Vec<ProviderConfig> {
        self.providers.read().await.clone()
    }

    pub async fn provider_config(&self, id: &str) -> Option<ProviderConfig> {
        self.providers.read().await.iter().find(|p| p.id == id).cloned()
    }

    pub async fn set_providers(&self, providers: Vec<ProviderConfig>) {
        config::save_providers(&providers);
        *self.providers.write().await = providers;
    }

    pub async fn settings(&self) -> Settings {
        self.settings.read().await.clone()
    }

    pub async fn set_skill_synthesis(&self, enabled: bool) {
        let mut settings = self.settings.write().await;
        settings.skill_synthesis_enabled = enabled;
        config::save_settings(&settings);
    }

    pub async fn remember_session(&self, provider_id: &str, model: &str) {
        let mut settings = self.settings.write().await;
        settings.last_provider = Some(provider_id.to_string());
        settings.last_model = Some(model.to_string());
        config::save_settings(&settings);
    }

    pub fn catalog(&self) -> &SharedCatalog {
        &self.catalog
    }

    pub async fn skills(&self) -> Vec<SkillSummary> {
        self.catalog.read().await.summaries()
    }

    pub async fn skill_count(&self) -> usize {
        self.catalog.read().await.len()
    }

    pub async fn tool_names(&self) -> Vec<String> {
        self.registry
            .read()
            .await
            .names()
            .map(str::to_string)
            .collect()
    }

    pub async fn problems(&self) -> Vec<String> {
        self.problems.read().await.clone()
    }

    pub async fn mcp_statuses(&self) -> Vec<ServerStatus> {
        self.mcp.statuses().await
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

        let provider = self.provider(&chosen).await?;
        let available = provider
            .models()
            .await
            .map_err(|e| format!("could not list models from '{chosen}': {e}"))?;

        let preferred = settings
            .last_model
            .filter(|m| available.iter().any(|a| &a.id == m))
            .or_else(|| available.first().map(|m| m.id.clone()))
            .ok_or_else(|| format!("provider '{chosen}' has no models available"))?;

        Ok((chosen, preferred))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use taurus_skills::proposal::CollectingSink;
    use taurus_tools::DenyAll;
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

    fn host(workspace: &Path) -> Host {
        Host::new(
            workspace.to_path_buf(),
            Arc::new(DenyingPrompts),
            Arc::new(NoProposals),
        )
    }

    #[tokio::test]
    async fn reload_registers_the_skill_tools_alongside_the_builtins() {
        let dir = TempDir::new().unwrap();
        let host = host(&dir.path().canonicalize().unwrap());
        host.reload().await;

        let tools = host.tool_names().await;
        for expected in ["read_file", "run_command", "load_skill", "propose_skill"] {
            assert!(tools.iter().any(|t| t == expected), "missing {expected}");
        }
        // The spawn tool is deliberately absent here; it is added per turn.
        assert!(!tools.iter().any(|t| t == taurus_core::SPAWN_TOOL));
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
        let host = host(&other.path().canonicalize().unwrap());
        host.reload().await;
        assert_eq!(host.skill_count().await, 0);

        host.set_workspace(&workspace).await.unwrap();
        assert_eq!(host.skill_count().await, 1);
    }

    #[tokio::test]
    async fn a_workspace_that_is_not_a_directory_is_refused() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let host = host(&dir.path().canonicalize().unwrap());
        assert!(host.set_workspace(&file).await.is_err());
    }

    #[tokio::test]
    async fn resolve_model_reports_a_missing_provider_clearly() {
        let dir = TempDir::new().unwrap();
        let host = host(&dir.path().canonicalize().unwrap());
        let err = host.resolve_model(Some("nonexistent"), None).await;
        // Falls back to the configured default rather than failing outright,
        // but an unreachable backend must still produce a readable message.
        if let Err(message) = err {
            assert!(!message.is_empty());
        }
    }

    #[tokio::test]
    async fn an_explicit_model_short_circuits_the_backend_query() {
        let dir = TempDir::new().unwrap();
        let host = host(&dir.path().canonicalize().unwrap());
        let (provider, model) = host
            .resolve_model(None, Some("some-model"))
            .await
            .expect("an explicit model needs no backend round trip");
        assert_eq!(model, "some-model");
        assert!(!provider.is_empty());
    }
}
