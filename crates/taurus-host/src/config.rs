//! On-disk configuration and the paths it lives at.
//!
//! Everything sits under `~/.taurus` on all three platforms rather than each
//! OS's idiomatic app-data directory. Users edit these files by hand and share
//! skill directories between machines, so one predictable path beats three
//! correct ones.
//!
//! Every config file exists in two scopes: the global `~/.taurus` and the
//! workspace's own `.taurus`. The workspace layer is read second and wins, the
//! same precedence skills already use — a project skill shadows a user skill of
//! the same name, and a project provider entry shadows a user one. Layering is
//! per-key rather than per-file, so a workspace overriding one provider's
//! `base_url` does not have to restate the rest of the list.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use taurus_agents::{AgentSource, AgentTier};
use taurus_skills::{SkillOrigin, SkillSource, SkillTier};
use ts_rs::TS;

pub const HOME_DIR_NAME: &str = ".taurus";
pub const WORKSPACE_DIR_NAME: &str = ".taurus";

/// The cross-client convention for shared skills, from the Agent Skills
/// specification. Anything installed here is visible to every client that
/// follows it, Taurus included.
pub const AGENTS_DIR_NAME: &str = ".agents";

/// Read for compatibility rather than convention: a large number of skills are
/// already installed here, and copying them into `.taurus` to use them would
/// leave the user maintaining two of everything.
pub const CLAUDE_DIR_NAME: &str = ".claude";

/// Where GitHub Copilot keeps a person's own customizations.
pub const COPILOT_DIR_NAME: &str = ".copilot";

/// Where GitHub Copilot keeps a repository's, which is not a dotdir of its own
/// but the folder the repository already has for everything GitHub reads.
pub const GITHUB_DIR_NAME: &str = ".github";

/// Overrides the config location. Set by tests so they cannot write to the
/// real `~/.taurus`, and usable to run an isolated instance.
pub const HOME_ENV: &str = "TAURUS_HOME";

/// Which of the two layers a value is read from or written to.
///
/// Re-exported from `taurus-tools`, which needs the same distinction for
/// persisted permission decisions. One enum and one TypeScript type: config
/// layering and permission layering are the same two places, and a second
/// identical type would only invite them to drift.
///
/// Ordering matters here: [`Scope::Workspace`] is applied after
/// [`Scope::Global`] and wins on conflict.
pub use taurus_tools::Scope;

/// `~/.taurus`, or whatever `TAURUS_HOME` points at.
///
/// The override exists because settings are written as a side effect of normal
/// operation — picking a workspace persists it — so any test touching that path
/// would otherwise reach into the user's live configuration.
pub fn home_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(HOME_ENV).filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    directories::BaseDirs::new()
        .map(|d| d.home_dir().join(HOME_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(HOME_DIR_NAME))
}

pub fn workspace_dir(workspace: &Path) -> PathBuf {
    workspace.join(WORKSPACE_DIR_NAME)
}

/// The directory a scope's files live in.
///
/// Returns `None` for [`Scope::Workspace`] with no workspace, which is the
/// state the app is in before one has been chosen.
pub fn scope_dir(scope: Scope, workspace: Option<&Path>) -> Option<PathBuf> {
    match scope {
        Scope::Global => Some(home_dir()),
        Scope::Workspace => workspace.map(workspace_dir),
    }
}

/// Both config directories in precedence order, lowest first.
///
/// Trust-gated, like every other read here: an untrusted workspace yields the
/// global directory alone. See [`crate::trust`].
pub fn config_dirs(workspace: Option<&Path>) -> Vec<PathBuf> {
    all_config_dirs(crate::trust::for_reading(workspace))
}

/// The same, without the gate. For [`crate::trust::pending`], which has to
/// describe what trusting a workspace *would* read.
pub(crate) fn all_config_dirs(workspace: Option<&Path>) -> Vec<PathBuf> {
    [Scope::Global, Scope::Workspace]
        .into_iter()
        .filter_map(|scope| scope_dir(scope, workspace))
        .collect()
}

/// Every directory a sub-agent can be defined in, lowest precedence first.
///
/// Claude's and Copilot's as well as Taurus's own, because all three keep the
/// same thing there: frontmatter naming the agent and a markdown body that is
/// its system prompt.
///
/// The skill list's shape, and for the same reasons — read the locations other
/// clients already write to, so an agent written for one of them works here
/// without being moved, and read Taurus's own last so a name you wrote wins
/// over a borrowed one you did not.
///
/// Copilot's are marked borrowed: Taurus reads them and never writes back. See
/// [`taurus_agents::AgentDefinition::borrowed`].
pub fn agent_sources(workspace: Option<&Path>) -> Vec<AgentSource> {
    all_agent_sources(crate::trust::for_reading(workspace))
}

/// The same, without the trust gate. See [`all_config_dirs`].
pub(crate) fn all_agent_sources(workspace: Option<&Path>) -> Vec<AgentSource> {
    let borrowed = |tier, dir| AgentSource {
        tier,
        dir,
        borrowed: true,
    };
    let mut sources = vec![
        borrowed(
            AgentTier::User,
            home_root().join(CLAUDE_DIR_NAME).join("agents"),
        ),
        borrowed(
            AgentTier::User,
            home_root().join(COPILOT_DIR_NAME).join("agents"),
        ),
        AgentSource {
            tier: AgentTier::User,
            dir: user_agents_dir(),
            borrowed: false,
        },
    ];
    if let Some(workspace) = workspace {
        sources.push(borrowed(
            AgentTier::Project,
            workspace.join(CLAUDE_DIR_NAME).join("agents"),
        ));
        sources.push(borrowed(
            AgentTier::Project,
            // `.github/agents`, matching where Copilot reads a repository's —
            // the same tier split `skill_sources` has for the same origin.
            workspace.join(GITHUB_DIR_NAME).join("agents"),
        ));
        sources.push(AgentSource {
            tier: AgentTier::Project,
            dir: workspace_agents_dir(workspace),
            borrowed: false,
        });
    }
    sources
}

pub fn user_skills_dir() -> PathBuf {
    home_dir().join("skills")
}

pub fn workspace_skills_dir(workspace: &Path) -> PathBuf {
    workspace_dir(workspace).join("skills")
}

/// The directory the shared skill conventions hang off.
///
/// Normally the user's home, so the conventions sit beside `~/.taurus` where
/// every other client writes them. Under `TAURUS_HOME` they move *inside* the
/// override instead of beside it: an override means "this instance keeps all
/// its config here", and reaching for a sibling of the override directory would
/// scan somewhere the user never pointed at — for the tests, which set that
/// variable specifically so they cannot touch real state, a shared system temp
/// directory.
pub(crate) fn home_root() -> PathBuf {
    if let Some(dir) = std::env::var_os(HOME_ENV).filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Every directory a skill can come from, lowest precedence first.
///
/// Two axes decide the order. Tier: a project skill shadows a user one, which
/// is the convention every client shares and the one users already expect.
/// Origin within a tier: the shared locations are read first and `.taurus`
/// last, so a skill written for Taurus wins over a borrowed one of the same
/// name — you can override a skill you did not write without editing it.
///
/// Reading the shared locations is what lets a skill installed by another
/// client work here untouched. It also means a skill in a repository arrives
/// from a directory Taurus does not own — and a skill can carry a script, so
/// the project tier of this list is behind [`crate::trust`].
pub fn skill_sources(workspace: Option<&Path>) -> Vec<SkillSource> {
    all_skill_sources(crate::trust::for_reading(workspace))
}

/// The same, without the trust gate. See [`all_config_dirs`].
pub(crate) fn all_skill_sources(workspace: Option<&Path>) -> Vec<SkillSource> {
    let home = home_root();
    let mut sources = Vec::new();

    let mut push = |tier, origin, dir: PathBuf| sources.push(SkillSource { tier, origin, dir });

    push(
        SkillTier::User,
        SkillOrigin::Agents,
        home.join(AGENTS_DIR_NAME).join("skills"),
    );
    push(
        SkillTier::User,
        SkillOrigin::Claude,
        home.join(CLAUDE_DIR_NAME).join("skills"),
    );
    push(
        SkillTier::User,
        SkillOrigin::Copilot,
        home.join(COPILOT_DIR_NAME).join("skills"),
    );
    push(SkillTier::User, SkillOrigin::Taurus, user_skills_dir());

    if let Some(workspace) = workspace {
        push(
            SkillTier::Project,
            SkillOrigin::Agents,
            workspace.join(AGENTS_DIR_NAME).join("skills"),
        );
        push(
            SkillTier::Project,
            SkillOrigin::Claude,
            workspace.join(CLAUDE_DIR_NAME).join("skills"),
        );
        // `.github/skills`, not `.copilot/skills`: a repository's Copilot
        // customizations live in the folder GitHub already reads, and a person's
        // live in a dotdir. The one origin whose two tiers do not share a name.
        push(
            SkillTier::Project,
            SkillOrigin::Copilot,
            workspace.join(GITHUB_DIR_NAME).join("skills"),
        );
        push(
            SkillTier::Project,
            SkillOrigin::Taurus,
            workspace_skills_dir(workspace),
        );
    }

    sources
}

pub fn user_agents_dir() -> PathBuf {
    home_dir().join("agents")
}

pub fn workspace_agents_dir(workspace: &Path) -> PathBuf {
    workspace_dir(workspace).join("agents")
}

pub fn providers_file(scope: Scope, workspace: Option<&Path>) -> Option<PathBuf> {
    scope_dir(scope, workspace).map(|d| d.join("providers.json"))
}

pub fn settings_file(scope: Scope, workspace: Option<&Path>) -> Option<PathBuf> {
    scope_dir(scope, workspace).map(|d| d.join("settings.json"))
}

pub fn search_file(scope: Scope, workspace: Option<&Path>) -> Option<PathBuf> {
    scope_dir(scope, workspace).map(|d| taurus_web::config::config_file(&d))
}

/// One layer's `hooks.json`. Ungated, like every other path resolver here —
/// the gate is on reading, not on naming. See [`load_hooks`].
pub fn hooks_file(scope: Scope, workspace: Option<&Path>) -> Option<PathBuf> {
    scope_dir(scope, workspace).map(|d| taurus_hooks::config_file(&d))
}

/// Both layers of `hooks.json`, merged, with anything unusable reported.
///
/// Trust-gated through [`config_dirs`], so a cloned repository's hooks do not
/// run until its config is being read at all. That gate is what makes honoring
/// a project's hook file reasonable in the first place — and the direction of
/// what a hook can do is the other half: it refuses calls, it never permits
/// them. See [`taurus_hooks`].
pub fn load_hooks(workspace: Option<&Path>) -> (taurus_hooks::HookRunner, Vec<String>) {
    let mut problems = Vec::new();
    let mut layers = Vec::new();
    for dir in config_dirs(workspace) {
        match taurus_hooks::load(&dir) {
            Ok(layer) => layers.push(layer),
            // One unparseable layer must not cost the other, the same rule
            // every other layered file follows.
            Err(e) => problems.push(e),
        }
    }
    let (hooks, merge_problems) = taurus_hooks::merge(layers);
    problems.extend(merge_problems);
    (taurus_hooks::HookRunner::new(hooks), problems)
}

pub fn mcp_file(scope: Scope, workspace: Option<&Path>) -> Option<PathBuf> {
    scope_dir(scope, workspace).map(|d| taurus_mcp::config::config_file(&d))
}

/// Ensures a layer's `mcp.json` exists, and returns it.
///
/// There is no UI for adding servers — the format is the one Claude Desktop
/// uses and people paste it between the two — so the app's job is to put the
/// file in front of them rather than to reimplement it as a form. Creating it
/// empty first is what makes that a working route on a machine that has never
/// had one, instead of an editor opening on nothing.
pub fn ensure_mcp_file(scope: Scope, workspace: Option<&Path>) -> Result<PathBuf, String> {
    let path = mcp_file(scope, workspace)
        .ok_or_else(|| "no workspace is open, so it has no config directory".to_string())?;
    if !path.exists() {
        write_config(&path, "{\n  \"mcpServers\": {}\n}\n");
    }
    // `write_config` swallows its failure by design, so the check is here: an
    // editor opening on a file that was never created is a worse outcome than
    // being told why.
    if !path.exists() {
        return Err(format!("could not create {}", path.display()));
    }
    Ok(path)
}

/// Creates a starter agent file in a scope, and returns it.
///
/// Authoring is a text editor, which is a fine surface once you know the
/// format and an opaque one before that. So the file arrives already carrying
/// every key with a comment saying what it does — the same reasoning behind
/// [`ensure_mcp_file`], pushed one step further because there is no other tool
/// this format is shared with to copy an example from.
///
/// An existing file is never overwritten: an agent someone has written is the
/// last thing a "new agent" button should be able to destroy.
pub fn create_agent_file(
    scope: Scope,
    workspace: Option<&Path>,
    name: &str,
) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("an agent needs a name".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || name.starts_with('-')
        || name.ends_with('-')
    {
        return Err(format!(
            "'{name}' must be kebab-case: lowercase letters, digits, and hyphens, like \
             'code-reviewer'"
        ));
    }

    let dir = match scope {
        Scope::Global => user_agents_dir(),
        Scope::Workspace => workspace_agents_dir(
            workspace
                .ok_or_else(|| "no workspace is open, so it has no config directory".to_string())?,
        ),
    };
    let path = dir.join(format!("{name}.md"));
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }

    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    std::fs::write(&path, agent_template(name))
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(path)
}

/// The starter file. Every key it can carry, with what it does next to it.
fn agent_template(name: &str) -> String {
    format!(
        "---\n\
         # The name you delegate to. Must match this file's name.\n\
         name: {name}\n\
         # The one line the main agent reads when choosing a sub-agent. Say when\n\
         # to use it, not what it is. Under 200 characters — it is sent on every\n\
         # request.\n\
         description: Describe when to hand work to this agent.\n\
         # The tools it may call, and the only ones it may call. Remove this key\n\
         # entirely to give it everything the main agent has.\n\
         tools: [read_file, list_dir, glob, grep]\n\
         # Tool round trips before it is stopped. 1 to 50.\n\
         max_iterations: 20\n\
         # Optional. Run this agent on a different model than the session's.\n\
         # Add `provider:` too if that model belongs to another provider.\n\
         # model: qwen3:32b\n\
         ---\n\
         \n\
         Everything below the frontmatter is this agent's system prompt.\n\
         \n\
         It cannot ask questions and it cannot delegate further, so tell it what\n\
         to do and what to report back. The agent that calls it sees only its\n\
         final reply, not its tool calls.\n"
    )
}

/// The global `search.json` alone, which is what an editor must edit.
///
/// The same rule `global_providers` follows: hand the editor the *merged* view
/// and saving it writes this workspace's overrides into every other workspace.
pub fn load_global_search() -> taurus_web::SearchFile {
    taurus_web::load(&home_dir()).unwrap_or_default()
}

pub fn save_search(file: &taurus_web::SearchFile) {
    let Some(path) = search_file(Scope::Global, None) else {
        return;
    };
    if let Ok(json) = serde_json::to_string_pretty(file) {
        write_config(&path, &json);
    }
}

/// Reads both layers of `search.json` and resolves the selected backend.
///
/// Returns `None` when web search is off, which is the default and not a
/// problem — see [`taurus_web::merge`] for the distinction between "not turned
/// on" and "turned on but unusable".
///
/// Writes a starter global file when none exists, the same courtesy
/// `providers.json` gets: every backend spelled out, none of them selected, so
/// enabling search is a one-word edit rather than a trip to the documentation.
pub fn load_search(workspace: Option<&Path>) -> (Option<taurus_web::Backend>, Vec<String>) {
    let mut problems = Vec::new();
    let mut layers = Vec::new();

    let global_dir = home_dir();
    if !taurus_web::config::config_file(&global_dir).exists() {
        if let Ok(json) = serde_json::to_string_pretty(&taurus_web::starter_file()) {
            write_config(&taurus_web::config::config_file(&global_dir), &json);
        }
    }

    for dir in config_dirs(workspace) {
        match taurus_web::load(&dir) {
            Ok(layer) => layers.push(layer),
            // One unparseable layer must not cost the other, the same rule
            // every other layered file follows.
            Err(e) => problems.push(e),
        }
    }

    let (backend, merge_problems) = taurus_web::merge_with(layers, |id, variable| {
        crate::secrets::resolve(&search_key_id(id), variable)
    });
    problems.extend(merge_problems);
    (backend, problems)
}

/// Credential-store id for a search backend's key.
///
/// Namespaced, because backend ids and provider ids are user-chosen and land in
/// the same store: without this, a search backend called `brave` and a provider
/// called `brave` would be one entry, and saving either would overwrite the
/// other's key.
pub fn search_key_id(backend_id: &str) -> String {
    format!("search:{backend_id}")
}

/// Where a search backend's key is coming from, for the settings screen.
pub fn search_key_status(backend_id: &str, api_key_env: Option<&str>) -> crate::secrets::KeyStatus {
    crate::secrets::status(&search_key_id(backend_id), api_key_env)
}

/// How to reach one model backend.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct ProviderConfig {
    /// Stable id used in the UI and in session records.
    pub id: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// The models this backend serves, when it will not say so itself.
    ///
    /// Empty means ask the backend — right for Ollama and for anything that
    /// answers `/v1/models` usefully. A non-empty list *is* the menu: the
    /// listing endpoint is not called at all, so a gateway that advertises
    /// four hundred models nobody has quota for can be cut down to the three
    /// that work.
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    /// Which of them to select first. Not a substitute for `models` — a
    /// provider can name a default without listing anything, and did so
    /// before `models` existed.
    #[serde(default)]
    pub default_model: Option<String>,
    /// Name of the environment variable holding the API key.
    ///
    /// The key itself is never written here: a config file full of secrets is
    /// the thing users accidentally commit. Optional now that keys can live in
    /// the OS credential store — a provider with no variable named reads from
    /// there, and one that names a variable prefers it. See [`crate::secrets`].
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Header the key is sent in, for gateways that do not use bearer auth.
    ///
    /// Unset means `Authorization: Bearer <key>`, which is what OpenAI and
    /// everything imitating it expects. A name here sends the key raw in that
    /// header instead, with no scheme prefix — `api-key` for Azure OpenAI,
    /// `Ocp-Apim-Subscription-Key` for an Azure APIM gateway.
    #[serde(default)]
    pub api_key_header: Option<String>,
    /// Overrides for backends that cannot be probed. Ignored for Ollama, which
    /// reports its own capabilities per model.
    #[serde(default)]
    pub native_tools: Option<bool>,
    /// The context window, in tokens.
    ///
    /// For a backend that cannot be asked, this *is* the window. For Ollama it
    /// is a ceiling on one that can: a local model reports the window it was
    /// trained for, which is regularly larger than the machine running it can
    /// serve at any usable speed, and unset means
    /// [`taurus_provider_ollama::DEFAULT_CONTEXT_LIMIT`] rather than unbounded.
    /// A model trained for less keeps its own smaller number either way.
    #[serde(default)]
    pub context_length: Option<u32>,
    /// Whether the models here read images. Unset means the provider's own
    /// default, which for an OpenAI-compatible endpoint is yes — set it false
    /// for a self-hosted server fronting text-only weights, so a screenshot is
    /// refused here with an explanation rather than a round trip away with a
    /// wire error. Ignored by every other kind: Ollama probes, and Anthropic
    /// and Gemini take images on every model they serve.
    #[serde(default)]
    pub vision: Option<bool>,
    /// Path prefix the OpenAI-compatible routes live under. Defaults to `/v1`,
    /// which is right for OpenAI, vLLM, LM Studio, llama.cpp, and OpenVINO
    /// Model Server from 2026.3 on; earlier OVMS builds need `/v3`.
    #[serde(default)]
    pub api_prefix: Option<String>,
    /// How an Anthropic provider asks the model to reason: `adaptive`,
    /// `disabled`, or unset for the model's own default.
    ///
    /// Unset is not a missing setting but the only one valid on every model
    /// that API has served — newer ones reason by default, older ones do not,
    /// and neither rejects a request that says nothing. Naming a mode is an
    /// override, and the wrong one is a 400 rather than a preference. Ignored
    /// by every other kind.
    #[serde(default)]
    pub thinking: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProviderKind {
    Ollama,
    /// Anything speaking the OpenAI chat-completions API.
    OpenAiCompatible,
    /// Anthropic's Messages API. Not OpenAI-shaped: the key rides `x-api-key`,
    /// the system prompt is a top-level field, and tool input is an object
    /// rather than a string, so it is a kind of its own rather than a `base_url`
    /// pointed at a different host.
    Anthropic,
    /// Google's Gemini `generateContent` API.
    Gemini,
}

/// One model a provider serves.
///
/// Written either way round in `providers.json`, because most entries have
/// nothing to say beyond their own name:
///
/// ```jsonc
/// "models": [
///   "gpt-4o",
///   { "id": "llama-3.1-8b", "context_length": 8192, "native_tools": false }
/// ]
/// ```
///
/// The overrides exist because an OpenAI-compatible endpoint reports no
/// capabilities at all, and one gateway commonly fronts models that do not
/// share them. Left unset a model inherits the provider's own values, so the
/// shorthand above means exactly what a bare id meant before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct ModelEntry {
    pub id: String,
    /// What the picker shows. Defaults to the id, which is what a raw
    /// `/v1/models` listing gives and what most deployments want.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Overrides the provider's, for a model that does not share it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
}

impl ModelEntry {
    /// The bare-id form, which is what the shorthand and the UI both produce.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_name: None,
            context_length: None,
            native_tools: None,
            vision: None,
        }
    }

    /// What to show in a picker.
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id)
    }
}

/*
 * Accepts the string shorthand as well as the full object.
 *
 * Serialization deliberately does not mirror it: everything is written back as
 * an object, so the UI has one shape to edit and the file has one shape to
 * read. A hand-written `["gpt-4o"]` keeps working and becomes `[{"id": ...}]`
 * the first time Settings saves that provider.
 */
impl<'de> Deserialize<'de> for ModelEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Id(String),
            Full {
                id: String,
                #[serde(default)]
                display_name: Option<String>,
                #[serde(default)]
                context_length: Option<u32>,
                #[serde(default)]
                native_tools: Option<bool>,
                #[serde(default)]
                vision: Option<bool>,
            },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Id(id) => Self::new(id),
            Wire::Full {
                id,
                display_name,
                context_length,
                native_tools,
                vision,
            } => Self {
                id,
                display_name,
                context_length,
                native_tools,
                vision,
            },
        })
    }
}

impl ProviderConfig {
    /// The key to send, from the environment if a variable names one and the
    /// credential store otherwise.
    pub fn api_key(&self) -> Option<String> {
        crate::secrets::resolve(&self.id, self.api_key_env.as_deref())
    }

    /// Which of the two the key came from, for display.
    pub fn key_status(&self) -> crate::secrets::KeyStatus {
        crate::secrets::status(&self.id, self.api_key_env.as_deref())
    }
}

/// One entry of a `providers.json`, before layers are merged.
///
/// Only `id` is required. Every other field is optional so a workspace can
/// retarget one provider's `base_url` without restating its kind, key, and
/// capability overrides — the common case, and the one where restating invites
/// the two layers to drift.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Replaces the inherited list wholesale rather than adding to it — a
    /// workspace that names models is stating which ones it wants, and an
    /// append could not express dropping one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ModelEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

impl ProviderEntry {
    /// Applies whatever this entry sets on top of an inherited provider.
    fn apply_to(&self, base: &mut ProviderConfig) {
        if let Some(kind) = self.kind {
            base.kind = kind;
        }
        if let Some(url) = &self.base_url {
            base.base_url = url.clone();
        }
        // A set-to-null in the file parses as `None`, which reads as "inherit"
        // rather than "clear". Clearing an inherited value is not expressible,
        // and has not been worth the double-Option it would cost.
        if let Some(models) = &self.models {
            base.models = models.clone();
        }
        if self.default_model.is_some() {
            base.default_model = self.default_model.clone();
        }
        if self.api_key_env.is_some() {
            base.api_key_env = self.api_key_env.clone();
        }
        if self.api_key_header.is_some() {
            base.api_key_header = self.api_key_header.clone();
        }
        if self.native_tools.is_some() {
            base.native_tools = self.native_tools;
        }
        if self.context_length.is_some() {
            base.context_length = self.context_length;
        }
        if self.vision.is_some() {
            base.vision = self.vision;
        }
        if self.api_prefix.is_some() {
            base.api_prefix = self.api_prefix.clone();
        }
        if self.thinking.is_some() {
            base.thinking = self.thinking.clone();
        }
    }

    /// Turns a standalone entry into a provider, or explains what it is missing.
    fn into_config(self) -> Result<ProviderConfig, String> {
        let kind = self.kind.ok_or_else(|| {
            format!(
                "provider '{}' is new in this layer and needs a `kind`",
                self.id
            )
        })?;
        let base_url = self.base_url.ok_or_else(|| {
            format!(
                "provider '{}' is new in this layer and needs a `base_url`",
                self.id
            )
        })?;
        Ok(ProviderConfig {
            id: self.id,
            kind,
            base_url,
            models: self.models.unwrap_or_default(),
            default_model: self.default_model,
            api_key_env: self.api_key_env,
            api_key_header: self.api_key_header,
            native_tools: self.native_tools,
            context_length: self.context_length,
            vision: self.vision,
            api_prefix: self.api_prefix,
            thinking: self.thinking,
        })
    }
}

impl From<&ProviderConfig> for ProviderEntry {
    fn from(config: &ProviderConfig) -> Self {
        Self {
            id: config.id.clone(),
            kind: Some(config.kind),
            base_url: Some(config.base_url.clone()),
            // An empty list is "ask the backend", which is also what omitting
            // the field means — so it is omitted rather than written as `[]`.
            models: Some(config.models.clone()).filter(|m| !m.is_empty()),
            default_model: config.default_model.clone(),
            api_key_env: config.api_key_env.clone(),
            api_key_header: config.api_key_header.clone(),
            native_tools: config.native_tools,
            context_length: config.context_length,
            vision: config.vision,
            api_prefix: config.api_prefix.clone(),
            thinking: config.thinking.clone(),
        }
    }
}

fn default_providers() -> Vec<ProviderConfig> {
    vec![ProviderConfig {
        id: "ollama".into(),
        kind: ProviderKind::Ollama,
        base_url: taurus_provider_ollama::DEFAULT_BASE_URL.into(),
        models: Vec::new(),
        default_model: None,
        api_key_env: None,
        api_key_header: None,
        native_tools: None,
        context_length: None,
        vision: None,
        api_prefix: None,
        thinking: None,
    }]
}

/// Reads one layer's `providers.json`, distinguishing absent from unreadable.
fn read_provider_layer(path: &Path) -> Result<Option<Vec<ProviderEntry>>, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    serde_json::from_str::<Vec<ProviderEntry>>(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads both layers and merges them by provider id.
///
/// Writes the default global file if none exists, so a first run leaves
/// something editable behind. Returns any layer that failed to parse as a
/// problem rather than an error: one bad file must not cost the user the other
/// layer, and they need to see *why* in the UI.
pub fn load_providers(workspace: Option<&Path>) -> (Vec<ProviderConfig>, Vec<String>) {
    // Gated first, and for the sharpest reason in this file: a provider entry
    // names the base URL a whole conversation is sent to.
    let workspace = crate::trust::for_reading(workspace);
    let mut problems = Vec::new();

    let global_path =
        providers_file(Scope::Global, workspace).expect("global scope always resolves");
    let mut merged: Vec<ProviderConfig> = match read_provider_layer(&global_path) {
        Ok(Some(entries)) if !entries.is_empty() => {
            let mut configs = Vec::new();
            for entry in entries {
                let id = entry.id.clone();
                match entry.into_config() {
                    Ok(config) => configs.push(config),
                    // The global layer inherits from nothing, so a partial
                    // entry there is simply incomplete.
                    Err(e) => problems.push(format!("{}: {e}", id)),
                }
            }
            if configs.is_empty() {
                default_providers()
            } else {
                configs
            }
        }
        Ok(Some(_)) => default_providers(),
        Ok(None) => {
            let providers = default_providers();
            save_providers(&providers);
            providers
        }
        Err(e) => {
            // Do not overwrite a file the user is mid-edit on; fall back and
            // let them see the parse error.
            tracing::warn!(error = %e, "providers.json is invalid; using defaults");
            problems.push(e);
            default_providers()
        }
    };

    let Some(path) = providers_file(Scope::Workspace, workspace) else {
        return (merged, problems);
    };
    match read_provider_layer(&path) {
        Ok(Some(entries)) => {
            for entry in entries {
                match merged.iter_mut().find(|p| p.id == entry.id) {
                    Some(base) => entry.apply_to(base),
                    None => {
                        let id = entry.id.clone();
                        match entry.into_config() {
                            Ok(config) => merged.push(config),
                            Err(e) => problems.push(format!("{}: {e}", id)),
                        }
                    }
                }
            }
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(error = %e, "workspace providers.json is invalid; ignoring it");
            problems.push(e);
        }
    }

    (merged, problems)
}

/// Writes the provider list to the global layer.
///
/// Edits from the UI always land globally: the workspace layer is a hand-written
/// override, and round-tripping the merged view into it would bake every
/// inherited value into the project file.
pub fn save_providers(providers: &[ProviderConfig]) {
    let Some(path) = providers_file(Scope::Global, None) else {
        return;
    };
    let entries: Vec<ProviderEntry> = providers.iter().map(ProviderEntry::from).collect();
    if let Ok(json) = serde_json::to_string_pretty(&entries) {
        write_config(&path, &json);
    }
}

/// Which palette the window paints in.
///
/// `System` is not a third palette: it is the absence of a choice, deferring to
/// what the OS reports and following it when it changes at dusk. Storing the
/// deferral rather than the resolved value is what makes that keep working —
/// writing "light" the moment someone opens the app in daylight would pin it
/// there for the night.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Preferences that survive a restart, with both layers already resolved.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Settings {
    #[serde(default)]
    pub last_workspace: Option<String>,
    #[serde(default)]
    pub last_provider: Option<String>,
    #[serde(default)]
    pub last_model: Option<String>,
    /// Whether the agent may propose skills on its own after a turn.
    #[serde(default = "default_true")]
    pub skill_synthesis_enabled: bool,
    /// Whether the agent may propose sub-agents on its own.
    ///
    /// Separate from skills because they are separate capabilities: a skill is
    /// a procedure the agent follows, an agent is a worker it hands a task to,
    /// and wanting one is no reason to want the other. Both default on, and
    /// both are gated by the same thing that makes that safe — nothing is
    /// written until the user approves the card.
    #[serde(default = "default_true")]
    pub agent_synthesis_enabled: bool,
    /// Tools to leave out of the harness entirely, by the exact name
    /// `taurus tools` prints.
    ///
    /// Every registered tool's schema is sent with every request, so this is
    /// the only lever on the one part of the prompt that is fixed overhead —
    /// it is paid on each iteration whether or not the tool is ever called.
    /// It matters most with MCP servers attached, where a dozen tools nobody
    /// uses can outweigh the whole system prompt.
    ///
    /// A disabled tool is never registered, so it is not merely hidden from
    /// the model: skills and sub-agents cannot reach it either. Hiding a tool
    /// that could still be invoked would be a permission gap wearing a
    /// token-saving costume.
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    /// Which palette the window paints in. Purely a UI preference, kept here so
    /// it is one more hand-editable line in the same file as the rest rather
    /// than state stranded in the webview's local storage.
    #[serde(default)]
    pub theme: Theme,
    /// The custom theme painting over that palette, by the name of its file
    /// under `themes/`. Empty is the built-in one.
    ///
    /// Separate from [`Settings::theme`] rather than a fourth variant of it,
    /// because the two answer different questions and are changed at very
    /// different rates. `theme` is light, dark, or follow the system — a
    /// choice somebody revisits at dusk. This is whose colours those are, and
    /// it is set once. Folding them together would mean picking a brand
    /// quietly throws away "follow the system", which is both the default and
    /// the only one of the three that can change on its own.
    #[serde(default)]
    pub theme_id: String,
    /// Model used to embed the workspace for `search_code`, on the same
    /// provider the conversation is using. Empty means no semantic search.
    ///
    /// Off by default, and deliberately so: it needs an embedding model pulled
    /// — `ollama pull nomic-embed-text` — and a tool the model can see is a
    /// tool it will try. One that cannot work costs it a turn to find that out,
    /// which is the same rule the web tools follow.
    ///
    /// Naming a *model* rather than a boolean because two indexes built by
    /// different models are not comparable, so the name is what the index is
    /// keyed on. Changing it discards the index rather than mixing vectors that
    /// mean different things.
    #[serde(default)]
    pub embedding_model: String,
    /// Which provider serves [`Settings::embedding_model`]. Empty means the one
    /// the conversation is on.
    ///
    /// Empty is right for a local setup, where the embedding model sits on the
    /// same server as the chat model and a second entry naming the same machine
    /// would be one more thing to keep in step. It stopped being right the
    /// moment a hosted backend was in play: Anthropic has no embedding endpoint
    /// at all and points at Voyage instead, so somebody chatting to Claude has
    /// to be able to index somewhere else without switching the conversation to
    /// get it.
    #[serde(default)]
    pub embedding_provider: String,
    /// Reranking model that reorders `search_code`'s shortlist before the model
    /// reads it. Empty means the similarity order is the answer.
    ///
    /// A second retrieval stage, and optional in a way the embedding model is
    /// not: without an embedding model there is no index and no tool, whereas
    /// without this there is a search that already works and is merely less
    /// accurate. That is why every failure here falls back rather than
    /// surfacing — see `taurus_index::SearchCode::with_rerank`.
    ///
    /// Named separately from [`Settings::embedding_model`] because the two are
    /// different namespaces on every backend that serves both, and because the
    /// index is keyed on the embedding model alone: changing this one reorders
    /// results but does not invalidate a single vector, so it must not discard
    /// the index the way changing that one does.
    #[serde(default)]
    pub rerank_model: String,
    /// Which provider serves [`Settings::rerank_model`]. Empty means the same
    /// one the index embeds on.
    ///
    /// Needed as its own setting, unlike embedding, because the common local
    /// setup cannot serve both. Ollama has no reranking route at all, so a
    /// machine embedding on Ollama has to name an OpenAI-compatible provider
    /// here — llama.cpp started with `--reranking` is the usual second entry.
    /// Anyone already running everything on one such server leaves this empty
    /// and it resolves to that server.
    #[serde(default)]
    pub rerank_provider: String,
    /// Where to send traces, in OTLP over HTTP. Empty means nowhere.
    ///
    /// There is no default and localhost is not one. A harness that reads
    /// private repositories has no business having an opinion about where a
    /// description of that work should be sent, so an endpoint is a thing
    /// somebody types — `http://localhost:4318` for a collector on this
    /// machine, or whatever Langfuse, Phoenix, or Honeycomb gave you.
    ///
    /// What is sent is the shape of a turn: which model, how many tokens, how
    /// long, which tools ran, what failed. Not the conversation — that is
    /// [`Settings::otlp_capture_content`], and it is a separate decision.
    ///
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` overrides this, because that is the
    /// variable every other instrumented program reads and tracing one run
    /// should not mean editing a file.
    #[serde(default)]
    pub otlp_endpoint: String,
    /// Whether exported traces may carry the messages themselves.
    ///
    /// Off, and it takes saying so to turn on. Token counts *describe* a
    /// conversation; messages *are* it — the files read, the commands run,
    /// whatever was pasted in — and a trace exporter is a network destination.
    /// Turning telemetry on should tell somebody how much a turn cost, never
    /// what was in it.
    ///
    /// Worth having anyway: debugging why a model went the wrong way is
    /// reading the prompt it actually got, and a collector you run yourself is
    /// a reasonable place to read it.
    #[serde(default)]
    pub otlp_capture_content: bool,
    /// Model turns one message may take before the turn is stopped.
    ///
    /// A ceiling on a loop that has no other one: a model that keeps calling
    /// tools without ever answering would otherwise run until the context
    /// window did. Twenty-five is enough for the work most turns are, and low
    /// enough that a model stuck in a loop is caught in seconds rather than
    /// minutes.
    ///
    /// Raise it for long refactors that legitimately need more rounds. It is
    /// bounded by [`taurus_agents::MAX_ITERATIONS_LIMIT`], the same ceiling a
    /// sub-agent's `max_iterations` is validated against — one limit for the
    /// two places iterations are counted, so a turn and the agent it delegates
    /// to cannot be governed by different rules.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
}

fn default_true() -> bool {
    true
}

fn default_max_iterations() -> u32 {
    25
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_workspace: None,
            last_provider: None,
            last_model: None,
            skill_synthesis_enabled: true,
            agent_synthesis_enabled: true,
            disabled_tools: Vec::new(),
            theme: Theme::System,
            theme_id: String::new(),
            embedding_model: String::new(),
            embedding_provider: String::new(),
            rerank_model: String::new(),
            rerank_provider: String::new(),
            otlp_endpoint: String::new(),
            otlp_capture_content: false,
            max_iterations: default_max_iterations(),
        }
    }
}

/// One layer's settings file. Every field is optional so an unset field means
/// "inherit" rather than "reset to the default" — without that, a workspace
/// file written to remember a model would also silently pin every other
/// preference to its default.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StoredSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_synthesis_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_synthesis_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
    /// See [`Settings::theme_id`]. Per-layer like the rest, which is what lets
    /// a repository carry its own branding in `.taurus/settings.json` beside
    /// the theme file in `.taurus/themes/` that it names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme_id: Option<String>,
    /// See [`Settings::embedding_model`]. Per-layer like everything else here,
    /// so one project can index with a different model — or not at all —
    /// without touching the global setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    /// See [`Settings::embedding_provider`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_provider: Option<String>,
    /// See [`Settings::rerank_model`]. Per-layer so a project whose codebase
    /// actually benefits from reranking can turn it on without paying the
    /// extra round trip on every other workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_model: Option<String>,
    /// See [`Settings::rerank_provider`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_provider: Option<String>,
    /// See [`Settings::otlp_endpoint`]. Per-layer so one project can be traced
    /// without every other workspace on the machine being traced too — which
    /// is the usual shape of wanting this at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otlp_endpoint: Option<String>,
    /// See [`Settings::otlp_capture_content`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otlp_capture_content: Option<bool>,
    /// See [`Settings::max_iterations`]. Per-layer so one project that needs
    /// long turns can raise it without loosening the ceiling everywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
}

impl StoredSettings {
    fn overlay(&mut self, other: StoredSettings) {
        self.last_workspace = other.last_workspace.or(self.last_workspace.take());
        self.last_provider = other.last_provider.or(self.last_provider.take());
        self.last_model = other.last_model.or(self.last_model.take());
        self.skill_synthesis_enabled = other
            .skill_synthesis_enabled
            .or(self.skill_synthesis_enabled);
        self.agent_synthesis_enabled = other
            .agent_synthesis_enabled
            .or(self.agent_synthesis_enabled);
        // Replaced rather than merged, like every other field: a workspace that
        // sets this is stating the list it wants, and a merge would make a
        // global entry impossible to undo for one project.
        self.disabled_tools = other.disabled_tools.or(self.disabled_tools.take());
        self.theme = other.theme.or(self.theme);
        self.theme_id = other.theme_id.or(self.theme_id.take());
        self.embedding_model = other.embedding_model.or(self.embedding_model.take());
        self.embedding_provider = other.embedding_provider.or(self.embedding_provider.take());
        self.rerank_model = other.rerank_model.or(self.rerank_model.take());
        self.rerank_provider = other.rerank_provider.or(self.rerank_provider.take());
        self.otlp_endpoint = other.otlp_endpoint.or(self.otlp_endpoint.take());
        self.otlp_capture_content = other.otlp_capture_content.or(self.otlp_capture_content);
        self.max_iterations = other.max_iterations.or(self.max_iterations);
    }

    fn resolve(self) -> Settings {
        let defaults = Settings::default();
        Settings {
            last_workspace: self.last_workspace,
            last_provider: self.last_provider,
            last_model: self.last_model,
            skill_synthesis_enabled: self
                .skill_synthesis_enabled
                .unwrap_or(defaults.skill_synthesis_enabled),
            agent_synthesis_enabled: self
                .agent_synthesis_enabled
                .unwrap_or(defaults.agent_synthesis_enabled),
            disabled_tools: self.disabled_tools.unwrap_or(defaults.disabled_tools),
            theme: self.theme.unwrap_or(defaults.theme),
            theme_id: self.theme_id.unwrap_or(defaults.theme_id),
            embedding_model: self.embedding_model.unwrap_or(defaults.embedding_model),
            embedding_provider: self
                .embedding_provider
                .unwrap_or(defaults.embedding_provider),
            rerank_model: self.rerank_model.unwrap_or(defaults.rerank_model),
            rerank_provider: self.rerank_provider.unwrap_or(defaults.rerank_provider),
            otlp_endpoint: self.otlp_endpoint.unwrap_or(defaults.otlp_endpoint),
            otlp_capture_content: self
                .otlp_capture_content
                .unwrap_or(defaults.otlp_capture_content),
            // Clamped rather than rejected: this file is hand-edited, and a
            // settings file that will not load is a worse answer to a typo'd
            // number than a number brought back into range. Zero would be a
            // turn that cannot take a single step, so the floor is one.
            max_iterations: self
                .max_iterations
                .unwrap_or(defaults.max_iterations)
                .clamp(1, taurus_agents::MAX_ITERATIONS_LIMIT),
        }
    }
}

pub fn read_settings(scope: Scope, workspace: Option<&Path>) -> StoredSettings {
    settings_file(scope, workspace)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Reads both layers and resolves them field by field.
pub fn load_settings(workspace: Option<&Path>) -> Settings {
    let workspace = crate::trust::for_reading(workspace);
    let mut merged = read_settings(Scope::Global, workspace);
    merged.overlay(read_settings(Scope::Workspace, workspace));
    merged.resolve()
}

/// Read-modify-writes one layer.
///
/// Only the fields the closure touches are written, so hand-edited keys in the
/// same file survive, and a value that is only meaningful in the other layer is
/// never copied down into this one.
pub fn edit_settings(
    scope: Scope,
    workspace: Option<&Path>,
    edit: impl FnOnce(&mut StoredSettings),
) {
    let Some(path) = settings_file(scope, workspace) else {
        return;
    };
    let mut stored = read_settings(scope, workspace);
    edit(&mut stored);
    if let Ok(json) = serde_json::to_string_pretty(&stored) {
        write_config(&path, &json);
    }
}

/// Writes a config file, creating its directory.
///
/// Failures are swallowed: persistence is a side effect of work the user asked
/// for, and a read-only workspace must cost them the memory of a preference,
/// not the action itself.
fn write_config(path: &Path, json: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = replace_file(path, json) {
        tracing::warn!(path = %path.display(), error = %e, "could not write config");
    }
}

/// Puts `contents` at `path` without ever leaving it partly written.
///
/// `fs::write` truncates before it writes, and these files are rewritten
/// constantly — `remember_session` saves settings at both layers after every
/// turn — so the window in which an interrupted write leaves an empty
/// `providers.json` is not a narrow one. That file is the whole provider list,
/// and the keychain entries keyed to those ids are orphaned along with it.
///
/// The temporary file is made in the target's own directory, because a rename
/// is only atomic within one filesystem and `~/.taurus` need not share one with
/// the system temp directory. Dropping it unpersisted removes it, so a failure
/// part way leaves neither a torn file nor litter behind.
///
/// One deliberate trade: this needs the *directory* to be writable, where a
/// plain overwrite needed only the file to be. A `.taurus` directory that is
/// read-only while its files are writable is a strange enough arrangement to be
/// worth losing, given what it buys.
fn replace_file(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(contents.as_bytes())?;
    // Flushed before the rename: otherwise a crash just after it could leave
    // the real name pointing at content that never reached the disk.
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Merges MCP server maps in layer order, lowest precedence first.
///
/// Lives here rather than in `taurus-mcp` because the layering rule is this
/// crate's concern; the MCP crate only knows how to read one file.
pub fn merge_mcp(layers: Vec<taurus_mcp::McpConfig>) -> (taurus_mcp::McpConfig, Vec<String>) {
    let mut servers: BTreeMap<String, taurus_mcp::ServerConfig> = BTreeMap::new();
    let mut problems = Vec::new();

    for layer in layers {
        // Carried through rather than dropped. An entry that would not parse is
        // the one the user most needs told about, and it is the only thing in
        // this file with nowhere else to be reported: it has no server to hang a
        // failed status off.
        for (name, reason) in layer.invalid {
            problems.push(format!("mcp server '{name}' {reason}"));
        }
        for (name, server) in layer.servers {
            match server {
                // A bare `{"disabled": true}` toggles an inherited server
                // rather than replacing it, so turning off one of your global
                // servers for one project does not mean copying its command
                // line into the project file.
                taurus_mcp::ServerConfig::Toggle(toggle) => match servers.get_mut(&name) {
                    Some(base) => base.set_disabled(toggle.disabled),
                    None => problems.push(format!(
                        "mcp server '{name}' is toggled but never defined; \
                             give it a `command` or a `url`"
                    )),
                },
                replacement => {
                    servers.insert(name, replacement);
                }
            }
        }
    }

    (
        taurus_mcp::McpConfig {
            servers,
            invalid: BTreeMap::new(),
        },
        problems,
    )
}

/// One layer's `mcp.json` as text, or empty when there is no file yet.
///
/// Text rather than a parsed config because every edit below is a read-modify-
/// write that has to preserve what this version does not model.
fn read_mcp_text(scope: Scope, workspace: Option<&Path>) -> Result<(PathBuf, String), String> {
    let path = mcp_file(scope, workspace)
        .ok_or_else(|| "no workspace is open, so it has no config directory".to_string())?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    Ok((path, text))
}

/// Writes one layer's `mcp.json`, reporting a failure rather than swallowing it.
///
/// [`write_config`] logs and moves on, which is right for settings written as a
/// side effect of normal operation. It is wrong here: this is someone pressing
/// Save, and a save that did nothing has to say so.
fn write_mcp_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    replace_file(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Adds or replaces one server in one layer.
///
/// The whole edit surface for the MCP panel is these three functions, and all
/// three go through `taurus_mcp::config`'s raw-JSON edits — so saving a server
/// never rewrites the ones beside it. That is what makes a form safe to point at
/// a file people also edit by hand.
pub fn save_mcp_server(
    scope: Scope,
    workspace: Option<&Path>,
    name: &str,
    server: &taurus_mcp::ServerConfig,
) -> Result<PathBuf, String> {
    let name = name.trim();
    validate_server_name(name)?;
    let (path, text) = read_mcp_text(scope, workspace)?;
    let updated = taurus_mcp::config::upsert_entry(&text, name, server)?;
    write_mcp_text(&path, &updated)?;
    Ok(path)
}

pub fn delete_mcp_server(
    scope: Scope,
    workspace: Option<&Path>,
    name: &str,
) -> Result<PathBuf, String> {
    let (path, text) = read_mcp_text(scope, workspace)?;
    let updated = taurus_mcp::config::remove_entry(&text, name)?;
    write_mcp_text(&path, &updated)?;
    Ok(path)
}

pub fn set_mcp_server_disabled(
    scope: Scope,
    workspace: Option<&Path>,
    name: &str,
    disabled: bool,
) -> Result<PathBuf, String> {
    let (path, text) = read_mcp_text(scope, workspace)?;
    let updated = taurus_mcp::config::set_entry_disabled(&text, name, disabled)?;
    write_mcp_text(&path, &updated)?;
    Ok(path)
}

/// What a server may be called.
///
/// The name is not decoration: it becomes part of every one of that server's
/// tool names, as `mcp__<server>__<tool>`. A name with a space or a double
/// underscore in it produces a tool name the model cannot reliably call and that
/// no provider's schema validation accepts, and the failure arrives much later,
/// as a model that will not use a server that connected fine.
fn validate_server_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a server needs a name".into());
    }
    if name.contains("__") {
        return Err(format!(
            "'{name}' cannot contain a double underscore: tool names are built as \
             `mcp__<server>__<tool>`, and a second one makes the server unidentifiable"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "'{name}' must be letters, digits, hyphens, or underscores — it becomes part of every \
             tool name this server offers"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::isolated_home;
    use tempfile::TempDir;

    /// Runs `body` against an isolated, empty `~/.taurus`.
    fn with_home<T>(body: impl FnOnce(&Path) -> T) -> T {
        let home = isolated_home();
        body(home.path())
    }

    fn write(path: PathBuf, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// A temp workspace whose own config layer is allowed to take effect.
    ///
    /// The layering tests below are about layering, so they are written against
    /// a workspace that is being read. That the gate stops an untrusted one
    /// being read at all is [`crate::trust`]'s to prove, and it does.
    fn trusted_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        crate::trust::trust(dir.path()).expect("trust the test workspace");
        dir
    }

    #[test]
    fn the_starter_agent_file_is_a_working_agent() {
        // A template that a reader has to fix before it loads would teach the
        // format by making them debug it.
        with_home(|_| {
            let dir = TempDir::new().unwrap();
            let path =
                create_agent_file(Scope::Workspace, Some(dir.path()), "code-reviewer").unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            let (frontmatter, body) = taurus_agents::parse_agent_md(&text, &path).unwrap();
            taurus_agents::validate(&frontmatter, &body, &path.display().to_string())
                .expect("the file the app writes must satisfy the rules the app enforces");
            assert_eq!(frontmatter.name, "code-reviewer");
        });
    }

    #[test]
    fn a_starter_file_never_overwrites_an_agent_someone_wrote() {
        with_home(|_| {
            let dir = TempDir::new().unwrap();
            create_agent_file(Scope::Workspace, Some(dir.path()), "keeper").unwrap();
            let err = create_agent_file(Scope::Workspace, Some(dir.path()), "keeper").unwrap_err();
            assert!(err.contains("already exists"));
        });
    }

    #[test]
    fn a_starter_file_rejects_a_name_the_catalog_would_reject() {
        // Better here than as a file that appears and then will not load.
        with_home(|_| {
            let dir = TempDir::new().unwrap();
            for bad in ["Code Reviewer", "code_reviewer", "-lead", ""] {
                assert!(
                    create_agent_file(Scope::Workspace, Some(dir.path()), bad).is_err(),
                    "'{bad}' should be rejected"
                );
            }
        });
    }

    #[test]
    fn a_model_entry_reads_as_a_bare_string_or_as_an_object() {
        // The shorthand is what most entries want to be, and what every
        // hand-written config that predates the overrides already is.
        let entries: Vec<ModelEntry> = serde_json::from_str(
            r#"["gpt-4o", {"id": "llama-3.1-8b", "context_length": 8192, "native_tools": false}]"#,
        )
        .expect("both spellings must parse");

        assert_eq!(entries[0], ModelEntry::new("gpt-4o"));
        assert_eq!(entries[1].context_length, Some(8192));
        assert_eq!(entries[1].native_tools, Some(false));
        // Unset is not `false`: it means "inherit the provider's".
        assert_eq!(entries[0].native_tools, None);
    }

    #[test]
    fn a_model_can_declare_itself_text_only() {
        // The one override a gateway needs that the others cannot express:
        // vision defaults on for an OpenAI-compatible provider, so a text-only
        // model behind one has to be able to say so per model.
        let entries: Vec<ModelEntry> =
            serde_json::from_str(r#"["gpt-4o", {"id": "llama-3.1-8b", "vision": false}]"#)
                .expect("the vision override must parse");

        assert_eq!(entries[1].vision, Some(false));
        assert_eq!(entries[0].vision, None);
    }

    #[test]
    fn a_model_labels_itself_with_its_id_until_told_otherwise() {
        assert_eq!(ModelEntry::new("gpt-4o").label(), "gpt-4o");
        let named = ModelEntry {
            display_name: Some("GPT-4o".into()),
            ..ModelEntry::new("gpt-4o")
        };
        assert_eq!(named.label(), "GPT-4o");
    }

    #[test]
    fn a_workspace_replaces_the_model_list_rather_than_adding_to_it() {
        // Appending could not express dropping one, and a workspace that names
        // models is stating which it wants — not which to add to someone
        // else's list.
        let mut base = ProviderConfig {
            id: "apim".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: "https://gateway".into(),
            models: vec![ModelEntry::new("gpt-4o"), ModelEntry::new("o3")],
            default_model: None,
            api_key_env: None,
            api_key_header: None,
            native_tools: None,
            context_length: None,
            vision: None,
            api_prefix: None,
            thinking: None,
        };

        ProviderEntry {
            id: "apim".into(),
            models: Some(vec![ModelEntry::new("gpt-4o-mini")]),
            ..ProviderEntry::default()
        }
        .apply_to(&mut base);
        assert_eq!(base.models, vec![ModelEntry::new("gpt-4o-mini")]);

        // Saying nothing about models leaves the inherited list alone, which is
        // the whole point of the overlay: retarget a base URL without
        // restating the menu.
        ProviderEntry {
            id: "apim".into(),
            base_url: Some("https://other".into()),
            ..ProviderEntry::default()
        }
        .apply_to(&mut base);
        assert_eq!(base.models, vec![ModelEntry::new("gpt-4o-mini")]);
        assert_eq!(base.base_url, "https://other");
    }

    #[test]
    fn a_search_backend_and_a_provider_of_the_same_name_keep_separate_keys() {
        // Both ids are user-chosen and both land in one credential store, so
        // without the namespace a backend called `brave` and a provider called
        // `brave` would be a single entry, and saving either would silently
        // destroy the other's key.
        assert_ne!(search_key_id("brave"), "brave");
        assert_eq!(search_key_id("brave"), "search:brave");
    }

    #[test]
    fn a_named_variable_beats_a_stored_search_key() {
        // The same precedence provider keys follow, which is the point of
        // routing both through `secrets` rather than reimplementing it: an
        // environment variable is the escape hatch for CI and for machines
        // with no keychain, so it has to win.
        let _home = isolated_home();
        std::env::set_var("TAURUS_TEST_SEARCH_KEY", "from-the-environment");

        let status = search_key_status("brave", Some("TAURUS_TEST_SEARCH_KEY"));
        assert!(
            matches!(status, crate::secrets::KeyStatus::Environment { ref variable }
                if variable == "TAURUS_TEST_SEARCH_KEY"),
            "{status:?}"
        );

        std::env::remove_var("TAURUS_TEST_SEARCH_KEY");
        assert_eq!(
            search_key_status("brave", Some("TAURUS_TEST_SEARCH_KEY")),
            crate::secrets::KeyStatus::Missing
        );
    }

    #[test]
    fn rewriting_a_config_replaces_it_and_leaves_nothing_behind() {
        let home = isolated_home();
        let path = home.path().join("settings.json");

        write_config(&path, r#"{"last_model":"a"}"#);
        write_config(&path, r#"{"last_model":"b"}"#);

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"last_model":"b"}"#
        );

        // Settings are rewritten after every turn, so a temp file that outlived
        // its rename would accumulate one per turn forever.
        let stray: Vec<String> = std::fs::read_dir(home.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "settings.json")
            .collect();
        assert!(stray.is_empty(), "temp files left behind: {stray:?}");
    }

    /// Stands in for the crash or full disk that cannot be staged in a test.
    ///
    /// A directory that refuses new entries fails the write at the same point
    /// an interruption would — before the rename — which is exactly the moment
    /// a plain `fs::write` would already have truncated the file.
    #[cfg(unix)]
    #[test]
    fn a_config_write_that_cannot_finish_leaves_the_old_one_intact() {
        use std::os::unix::fs::PermissionsExt;

        let home = isolated_home();
        let dir = home.path().join("locked");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("providers.json");
        write_config(&path, r#"[{"id":"ollama"}]"#);

        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o500);
        std::fs::set_permissions(&dir, perms).unwrap();

        write_config(&path, "half a file");

        // Restored before asserting, so a failure here cannot leave a directory
        // the test harness is unable to clean up.
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"[{"id":"ollama"}]"#,
            "a write that could not complete must not destroy the provider list"
        );
    }

    #[test]
    fn a_workspace_entry_overrides_one_field_and_inherits_the_rest() {
        with_home(|home| {
            let ws = trusted_workspace();
            write(
                home.join("providers.json"),
                r#"[{"id": "vllm", "kind": "open_ai_compatible", "base_url": "http://a",
                     "api_key_env": "KEY", "context_length": 8192}]"#,
            );
            write(
                workspace_dir(ws.path()).join("providers.json"),
                r#"[{"id": "vllm", "base_url": "http://b"}]"#,
            );

            let (providers, problems) = load_providers(Some(ws.path()));
            assert!(problems.is_empty(), "{problems:?}");
            let vllm = providers.iter().find(|p| p.id == "vllm").unwrap();
            assert_eq!(vllm.base_url, "http://b");
            assert_eq!(vllm.api_key_env.as_deref(), Some("KEY"));
            assert_eq!(vllm.context_length, Some(8192));
            assert_eq!(vllm.kind, ProviderKind::OpenAiCompatible);
        });
    }

    #[test]
    fn an_api_prefix_layers_like_every_other_field() {
        // What an OpenVINO Model Server before 2026.3 needs, set for one
        // project without disturbing the global entry.
        with_home(|home| {
            let ws = trusted_workspace();
            write(
                home.join("providers.json"),
                r#"[{"id": "ov", "kind": "open_ai_compatible",
                     "base_url": "http://localhost:8000", "context_length": 8192}]"#,
            );
            write(
                workspace_dir(ws.path()).join("providers.json"),
                r#"[{"id": "ov", "api_prefix": "/v3"}]"#,
            );

            let (providers, problems) = load_providers(Some(ws.path()));
            assert!(problems.is_empty(), "{problems:?}");
            let ov = providers.iter().find(|p| p.id == "ov").unwrap();
            assert_eq!(ov.api_prefix.as_deref(), Some("/v3"));
            assert_eq!(ov.context_length, Some(8192), "inherited, not reset");

            // The global layer still says nothing about a prefix.
            assert!(load_providers(None)
                .0
                .iter()
                .find(|p| p.id == "ov")
                .unwrap()
                .api_prefix
                .is_none());
        });
    }

    #[test]
    fn a_workspace_can_add_a_provider_the_global_layer_never_saw() {
        with_home(|_| {
            let ws = trusted_workspace();
            write(
                workspace_dir(ws.path()).join("providers.json"),
                r#"[{"id": "local", "kind": "ollama", "base_url": "http://127.0.0.1:11434"}]"#,
            );

            let (providers, problems) = load_providers(Some(ws.path()));
            assert!(problems.is_empty(), "{problems:?}");
            assert!(providers.iter().any(|p| p.id == "local"));
            // The default global provider is still there.
            assert!(providers.iter().any(|p| p.id == "ollama"));
        });
    }

    #[test]
    fn a_new_workspace_provider_missing_its_kind_is_reported_not_silently_dropped() {
        with_home(|_| {
            let ws = trusted_workspace();
            write(
                workspace_dir(ws.path()).join("providers.json"),
                r#"[{"id": "mystery", "base_url": "http://x"}]"#,
            );

            let (providers, problems) = load_providers(Some(ws.path()));
            assert!(!providers.iter().any(|p| p.id == "mystery"));
            assert_eq!(problems.len(), 1);
            assert!(problems[0].contains("kind"), "{problems:?}");
        });
    }

    #[test]
    fn a_malformed_workspace_layer_keeps_the_global_one() {
        with_home(|home| {
            let ws = trusted_workspace();
            write(
                home.join("providers.json"),
                r#"[{"id": "ollama", "kind": "ollama", "base_url": "http://a"}]"#,
            );
            write(
                workspace_dir(ws.path()).join("providers.json"),
                "{ not json",
            );

            let (providers, problems) = load_providers(Some(ws.path()));
            assert_eq!(providers.len(), 1);
            assert_eq!(providers[0].base_url, "http://a");
            assert_eq!(problems.len(), 1);
            assert!(problems[0].contains("providers.json"), "{problems:?}");
        });
    }

    #[test]
    fn a_first_run_leaves_an_editable_global_file_behind() {
        with_home(|home| {
            let (providers, _) = load_providers(None);
            assert_eq!(providers.len(), 1);
            assert!(home.join("providers.json").is_file());
        });
    }

    #[test]
    fn settings_resolve_field_by_field_rather_than_file_by_file() {
        with_home(|home| {
            let ws = trusted_workspace();
            write(
                home.join("settings.json"),
                r#"{"last_model": "global-model", "skill_synthesis_enabled": false}"#,
            );
            write(
                workspace_dir(ws.path()).join("settings.json"),
                r#"{"last_model": "project-model"}"#,
            );

            let settings = load_settings(Some(ws.path()));
            assert_eq!(settings.last_model.as_deref(), Some("project-model"));
            // Not reset to the default just because the workspace file omits it.
            assert!(!settings.skill_synthesis_enabled);
        });
    }

    #[test]
    fn a_turn_gets_twenty_five_steps_until_someone_says_otherwise() {
        with_home(|home| {
            write(home.join("settings.json"), r#"{"last_model": "m"}"#);
            assert_eq!(load_settings(None).max_iterations, 25);
        });
    }

    #[test]
    fn a_hand_edited_step_count_is_clamped_rather_than_rejected() {
        // This file is edited by hand, and a settings file that will not load
        // is a worse answer to a typo than a number brought back into range.
        // Zero is the one that matters most: it would be a turn that cannot
        // take a single step, which reads as the app being broken.
        let ceiling = taurus_agents::MAX_ITERATIONS_LIMIT;
        for (written, resolved) in [
            (0, 1),
            (1, 1),
            (30, 30),
            (ceiling, ceiling),
            (ceiling + 1, ceiling),
            (100_000, ceiling),
        ] {
            with_home(|home| {
                write(
                    home.join("settings.json"),
                    &format!(r#"{{"max_iterations": {written}}}"#),
                );
                assert_eq!(
                    load_settings(None).max_iterations,
                    resolved,
                    "{written} should resolve to {resolved}"
                );
            });
        }
    }

    #[test]
    fn a_workspace_can_raise_the_step_count_without_touching_the_global_one() {
        // A repository whose refactors legitimately need more rounds should not
        // have to loosen the limit for every other project.
        with_home(|home| {
            let ws = trusted_workspace();
            write(home.join("settings.json"), r#"{"max_iterations": 10}"#);
            write(
                workspace_dir(ws.path()).join("settings.json"),
                r#"{"max_iterations": 40}"#,
            );

            assert_eq!(load_settings(Some(ws.path())).max_iterations, 40);
            assert_eq!(load_settings(None).max_iterations, 10);
        });
    }

    /// Following the OS is the absence of a choice, so it has to be what an
    /// untouched install resolves to — a settings file that has never mentioned
    /// the theme must not pin one.
    #[test]
    fn a_theme_nobody_set_follows_the_system() {
        with_home(|home| {
            write(home.join("settings.json"), r#"{"last_model": "m"}"#);
            assert_eq!(load_settings(None).theme, Theme::System);
        });
    }

    #[test]
    fn a_stored_theme_survives_the_round_trip() {
        with_home(|home| {
            edit_settings(Scope::Global, None, |s| s.theme = Some(Theme::Light));

            let text = std::fs::read_to_string(home.join("settings.json")).unwrap();
            assert!(text.contains("\"theme\": \"light\""), "{text}");
            assert_eq!(load_settings(None).theme, Theme::Light);
        });
    }

    #[test]
    fn editing_one_layer_leaves_the_other_alone() {
        with_home(|home| {
            let ws = trusted_workspace();
            write(
                home.join("settings.json"),
                r#"{"last_model": "global-model"}"#,
            );

            edit_settings(Scope::Workspace, Some(ws.path()), |s| {
                s.last_model = Some("project-model".into())
            });

            assert_eq!(
                read_settings(Scope::Global, None).last_model.as_deref(),
                Some("global-model")
            );
            let stored = read_settings(Scope::Workspace, Some(ws.path()));
            assert_eq!(stored.last_model.as_deref(), Some("project-model"));
            // The workspace file records only what was set there.
            assert!(stored.last_workspace.is_none());
            assert!(stored.skill_synthesis_enabled.is_none());
        });
    }

    #[test]
    fn an_unset_field_is_not_written_and_so_does_not_pin_a_default() {
        with_home(|ws_home| {
            edit_settings(Scope::Global, None, |s| {
                s.last_provider = Some("ollama".into())
            });
            let text = std::fs::read_to_string(ws_home.join("settings.json")).unwrap();
            assert!(text.contains("last_provider"));
            assert!(
                !text.contains("skill_synthesis_enabled"),
                "an untouched field must not be materialized: {text}"
            );
        });
    }

    #[test]
    fn saving_providers_writes_the_global_layer_only() {
        with_home(|home| {
            let ws = trusted_workspace();
            save_providers(&default_providers());
            assert!(home.join("providers.json").is_file());
            assert!(!workspace_dir(ws.path()).join("providers.json").exists());
        });
    }

    #[test]
    fn a_workspace_toggle_disables_an_inherited_mcp_server() {
        let global = taurus_mcp::parse(
            r#"{"mcpServers": {"fs": {"command": "npx", "args": ["-y", "server-fs"]}}}"#,
        )
        .unwrap();
        let workspace = taurus_mcp::parse(r#"{"mcpServers": {"fs": {"disabled": true}}}"#).unwrap();

        let (merged, problems) = merge_mcp(vec![global, workspace]);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(merged.servers["fs"].disabled());
        // The inherited command line survives the toggle.
        assert_eq!(merged.servers["fs"].describe(), "npx -y server-fs");
    }

    #[test]
    fn a_toggle_with_nothing_to_toggle_is_reported() {
        let workspace =
            taurus_mcp::parse(r#"{"mcpServers": {"ghost": {"disabled": true}}}"#).unwrap();
        let (merged, problems) = merge_mcp(vec![workspace]);
        assert!(merged.servers.is_empty());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("ghost"), "{problems:?}");
    }

    #[test]
    fn a_workspace_server_replaces_the_inherited_definition() {
        let global = taurus_mcp::parse(r#"{"mcpServers": {"fs": {"command": "old"}}}"#).unwrap();
        let workspace = taurus_mcp::parse(r#"{"mcpServers": {"fs": {"command": "new"}}}"#).unwrap();
        let (merged, _) = merge_mcp(vec![global, workspace]);
        assert_eq!(merged.servers["fs"].describe(), "new");
    }

    #[test]
    fn an_entry_that_will_not_parse_is_reported_by_name_and_costs_nobody_their_other_servers() {
        // Both halves of the fix, at the layer that reports them. The message
        // has to name the server, because the file it came from can hold a dozen.
        let layer = taurus_mcp::parse(
            r#"{"mcpServers": {
                 "works": {"command": "npx"},
                 "broken": {"commnd": "npx"}
               }}"#,
        )
        .unwrap();
        let (merged, problems) = merge_mcp(vec![layer]);

        assert!(merged.servers.contains_key("works"));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("broken"), "{problems:?}");
        assert!(problems[0].contains("command"), "{problems:?}");
    }

    #[test]
    fn saving_one_server_creates_the_file_and_leaves_the_others_alone() {
        let home = isolated_home();
        {
            let existing = taurus_mcp::ServerConfig::Stdio {
                command: "old".into(),
                args: vec![],
                env: BTreeMap::new(),
                disabled: false,
            };
            save_mcp_server(Scope::Global, None, "first", &existing).unwrap();
            save_mcp_server(
                Scope::Global,
                None,
                "second",
                &taurus_mcp::ServerConfig::Http {
                    url: "https://example.com/mcp".into(),
                    headers: BTreeMap::new(),
                    disabled: false,
                },
            )
            .unwrap();

            let config = taurus_mcp::load(home.path()).unwrap();
            assert_eq!(config.servers.len(), 2, "{:?}", config.invalid);
            assert_eq!(config.servers["first"].describe(), "old");

            set_mcp_server_disabled(Scope::Global, None, "first", true).unwrap();
            let config = taurus_mcp::load(home.path()).unwrap();
            assert!(config.servers["first"].disabled());
            assert!(!config.servers["second"].disabled());

            delete_mcp_server(Scope::Global, None, "first").unwrap();
            let config = taurus_mcp::load(home.path()).unwrap();
            assert_eq!(config.servers.len(), 1);
            assert!(config.servers.contains_key("second"));
        }
    }

    #[test]
    fn a_name_that_would_break_its_own_tool_names_is_refused_at_the_save() {
        let _home = isolated_home();
        let server = taurus_mcp::ServerConfig::Stdio {
            command: "npx".into(),
            args: vec![],
            env: BTreeMap::new(),
            disabled: false,
        };
        // Every one of these produces `mcp__<name>__<tool>` that cannot be
        // parsed back, or that a provider's schema validation rejects — and
        // the symptom is a server that connects and is then never called.
        for name in ["", "my server", "a__b", "sérveur"] {
            assert!(
                save_mcp_server(Scope::Global, None, name, &server).is_err(),
                "{name:?} must not be savable"
            );
        }
        assert!(save_mcp_server(Scope::Global, None, "my-server_2", &server).is_ok());
    }
}
