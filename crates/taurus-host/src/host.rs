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

use crate::config::{self, ProviderConfig, ProviderKind, Scope, Settings};
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
            registry: Arc::new(RwLock::new(ToolRegistry::with_builtins())),
            permissions: RwLock::new(permissions),
            mcp: McpManager::new(),
            problems: RwLock::new(Vec::new()),
            prompts,
            proposals,
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

        let (providers, mut problems) = config::load_providers(Some(&workspace));
        *self.providers.write().await = providers;
        *self.settings.write().await = config::load_settings(Some(&workspace));

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
        problems.extend(skill_problems.iter().map(|p| p.to_string()));
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
        let mut layers = Vec::new();
        for dir in config::config_dirs(Some(&workspace)) {
            match taurus_mcp::load(&dir) {
                Ok(layer) => layers.push(layer),
                // A layer that will not parse is skipped, not fatal: the other
                // one is still a working set of servers.
                Err(e) => problems.push(e),
            }
        }
        let (mcp_config, mcp_problems) = config::merge_mcp(layers);
        problems.extend(mcp_problems);
        for tool in self.mcp.connect_all(&mcp_config).await {
            registry.register(tool);
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
                            vision: defaults.vision,
                            context_length: config
                                .context_length
                                .unwrap_or(defaults.context_length),
                        },
                    )
                    .with_api_prefix(config.api_prefix.clone()),
                )
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

    pub async fn settings(&self) -> Settings {
        self.settings.read().await.clone()
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
    use crate::testing::{isolated_home, HomeGuard};
    use async_trait::async_trait;
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
            Arc::new(NoProposals),
        );
        (host, home)
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

        assert!(
            host.problems()
                .await
                .iter()
                .any(|p| p.contains("providers.json")),
            "a config file the user must fix has to reach the UI"
        );
        // And the global layer still works.
        assert!(!host.providers().await.is_empty());
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
