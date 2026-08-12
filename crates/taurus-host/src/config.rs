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
use ts_rs::TS;

pub const HOME_DIR_NAME: &str = ".taurus";
pub const WORKSPACE_DIR_NAME: &str = ".taurus";

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
pub fn config_dirs(workspace: Option<&Path>) -> Vec<PathBuf> {
    [Scope::Global, Scope::Workspace]
        .into_iter()
        .filter_map(|scope| scope_dir(scope, workspace))
        .collect()
}

pub fn user_skills_dir() -> PathBuf {
    home_dir().join("skills")
}

pub fn workspace_skills_dir(workspace: &Path) -> PathBuf {
    workspace_dir(workspace).join("skills")
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
    #[serde(default)]
    pub context_length: Option<u32>,
    /// Path prefix the OpenAI-compatible routes live under. Defaults to `/v1`,
    /// which is right for OpenAI, vLLM, LM Studio, llama.cpp, and OpenVINO
    /// Model Server from 2026.3 on; earlier OVMS builds need `/v3`.
    #[serde(default)]
    pub api_prefix: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProviderKind {
    Ollama,
    /// Anything speaking the OpenAI chat-completions API.
    OpenAiCompatible,
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
}

impl ModelEntry {
    /// The bare-id form, which is what the shorthand and the UI both produce.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_name: None,
            context_length: None,
            native_tools: None,
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
            },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Id(id) => Self::new(id),
            Wire::Full {
                id,
                display_name,
                context_length,
                native_tools,
            } => Self {
                id,
                display_name,
                context_length,
                native_tools,
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
    pub api_prefix: Option<String>,
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
        if self.api_prefix.is_some() {
            base.api_prefix = self.api_prefix.clone();
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
            api_prefix: self.api_prefix,
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
            api_prefix: config.api_prefix.clone(),
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
        api_prefix: None,
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
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_workspace: None,
            last_provider: None,
            last_model: None,
            skill_synthesis_enabled: true,
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
}

impl StoredSettings {
    fn overlay(&mut self, other: StoredSettings) {
        self.last_workspace = other.last_workspace.or(self.last_workspace.take());
        self.last_provider = other.last_provider.or(self.last_provider.take());
        self.last_model = other.last_model.or(self.last_model.take());
        self.skill_synthesis_enabled = other
            .skill_synthesis_enabled
            .or(self.skill_synthesis_enabled);
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

    (taurus_mcp::McpConfig { servers }, problems)
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
            api_prefix: None,
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
            let ws = TempDir::new().unwrap();
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
            let ws = TempDir::new().unwrap();
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
            let ws = TempDir::new().unwrap();
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
            let ws = TempDir::new().unwrap();
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
            let ws = TempDir::new().unwrap();
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
            let ws = TempDir::new().unwrap();
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
    fn editing_one_layer_leaves_the_other_alone() {
        with_home(|home| {
            let ws = TempDir::new().unwrap();
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
            let ws = TempDir::new().unwrap();
            save_providers(&default_providers());
            assert!(home.join("providers.json").is_file());
            assert!(!workspace_dir(ws.path()).join("providers.json").exists());
        });
    }

    #[test]
    fn a_workspace_toggle_disables_an_inherited_mcp_server() {
        let global: taurus_mcp::McpConfig = serde_json::from_str(
            r#"{"mcpServers": {"fs": {"command": "npx", "args": ["-y", "server-fs"]}}}"#,
        )
        .unwrap();
        let workspace: taurus_mcp::McpConfig =
            serde_json::from_str(r#"{"mcpServers": {"fs": {"disabled": true}}}"#).unwrap();

        let (merged, problems) = merge_mcp(vec![global, workspace]);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(merged.servers["fs"].disabled());
        // The inherited command line survives the toggle.
        assert_eq!(merged.servers["fs"].describe(), "npx -y server-fs");
    }

    #[test]
    fn a_toggle_with_nothing_to_toggle_is_reported() {
        let workspace: taurus_mcp::McpConfig =
            serde_json::from_str(r#"{"mcpServers": {"ghost": {"disabled": true}}}"#).unwrap();
        let (merged, problems) = merge_mcp(vec![workspace]);
        assert!(merged.servers.is_empty());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("ghost"), "{problems:?}");
    }

    #[test]
    fn a_workspace_server_replaces_the_inherited_definition() {
        let global: taurus_mcp::McpConfig =
            serde_json::from_str(r#"{"mcpServers": {"fs": {"command": "old"}}}"#).unwrap();
        let workspace: taurus_mcp::McpConfig =
            serde_json::from_str(r#"{"mcpServers": {"fs": {"command": "new"}}}"#).unwrap();
        let (merged, _) = merge_mcp(vec![global, workspace]);
        assert_eq!(merged.servers["fs"].describe(), "new");
    }
}
