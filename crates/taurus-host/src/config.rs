//! On-disk configuration and the paths it lives at.
//!
//! Everything sits under `~/.taurus` on all three platforms rather than each
//! OS's idiomatic app-data directory. Users edit these files by hand and share
//! skill directories between machines, so one predictable path beats three
//! correct ones.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const HOME_DIR_NAME: &str = ".taurus";
pub const WORKSPACE_DIR_NAME: &str = ".taurus";

/// `~/.taurus`, created on first use.
pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().join(HOME_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(HOME_DIR_NAME))
}

pub fn user_skills_dir() -> PathBuf {
    home_dir().join("skills")
}

pub fn workspace_skills_dir(workspace: &std::path::Path) -> PathBuf {
    workspace.join(WORKSPACE_DIR_NAME).join("skills")
}

pub fn providers_file() -> PathBuf {
    home_dir().join("providers.json")
}

pub fn settings_file() -> PathBuf {
    home_dir().join("settings.json")
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
    /// Default model to select when this provider is chosen.
    #[serde(default)]
    pub default_model: Option<String>,
    /// Name of the environment variable holding the API key.
    ///
    /// The key itself is never written here: a config file full of secrets is
    /// the thing users accidentally commit.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Overrides for backends that cannot be probed. Ignored for Ollama, which
    /// reports its own capabilities per model.
    #[serde(default)]
    pub native_tools: Option<bool>,
    #[serde(default)]
    pub context_length: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProviderKind {
    Ollama,
    /// Anything speaking the OpenAI chat-completions API.
    OpenAiCompatible,
}

impl ProviderConfig {
    pub fn api_key(&self) -> Option<String> {
        self.api_key_env
            .as_ref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|k| !k.trim().is_empty())
    }
}

fn default_providers() -> Vec<ProviderConfig> {
    vec![ProviderConfig {
        id: "ollama".into(),
        kind: ProviderKind::Ollama,
        base_url: taurus_provider_ollama::DEFAULT_BASE_URL.into(),
        default_model: None,
        api_key_env: None,
        native_tools: None,
        context_length: None,
    }]
}

/// Reads `~/.taurus/providers.json`, writing the default if it is absent.
pub fn load_providers() -> Vec<ProviderConfig> {
    let path = providers_file();
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Vec<ProviderConfig>>(&text) {
            Ok(providers) if !providers.is_empty() => providers,
            Ok(_) => default_providers(),
            Err(e) => {
                // Do not overwrite a file the user is mid-edit on; fall back
                // and let them see the parse error in the log.
                tracing::warn!(path = %path.display(), error = %e, "providers.json is invalid; using defaults");
                default_providers()
            }
        },
        Err(_) => {
            let providers = default_providers();
            save_providers(&providers);
            providers
        }
    }
}

pub fn save_providers(providers: &[ProviderConfig]) {
    let path = providers_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(providers) {
        let _ = std::fs::write(path, json);
    }
}

/// Preferences that survive a restart.
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

pub fn load_settings() -> Settings {
    std::fs::read_to_string(settings_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(settings: &Settings) {
    let path = settings_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, json);
    }
}
