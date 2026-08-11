//! Search backend configuration.
//!
//! One file, `search.json`, in the same two layers every other config file
//! uses. It names backends and picks one of them; the tools are only registered
//! when that pick resolves to something that can actually run, so the model is
//! never shown a `web_search` that is guaranteed to fail.
//!
//! There is no default backend. Searching the web means sending the user's
//! prompt to a third party, and that is not a thing to start doing because a
//! program was installed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use taurus_tools::expand_env;

/// Which service a backend talks to. Each speaks its own dialect; the kind is
/// what selects the request shape and the response parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// Brave Search API. Key goes in `X-Subscription-Token`.
    Brave,
    /// Tavily, which is built for agents and returns page extracts rather than
    /// snippets. Key goes in `Authorization: Bearer`.
    Tavily,
    /// A SearXNG instance, yours or someone else's. No key; the whole point is
    /// that there is no account behind it.
    Searxng,
}

impl BackendKind {
    /// Where this service lives when the config does not say.
    ///
    /// SearXNG has no default: there is no canonical instance, and guessing
    /// `localhost` would turn a missing setting into a connection refused.
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Brave => Some("https://api.search.brave.com"),
            Self::Tavily => Some("https://api.tavily.com"),
            Self::Searxng => None,
        }
    }

    /// Whether a credential is required to get an answer at all.
    pub fn needs_key(self) -> bool {
        !matches!(self, Self::Searxng)
    }
}

/// One `search.json` as written, before layers are merged.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchFile {
    /// Id of the backend to use. Unset means web search stays off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default)]
    pub backends: BTreeMap<String, BackendEntry>,
}

/// One backend as written. Every field but `kind` is optional, and `kind`
/// itself is optional so a workspace can retarget one field of an inherited
/// backend without restating the rest — the same rule `providers.json` follows.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BackendEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<BackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Name of the environment variable holding the API key. The key itself is
    /// never written here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Results requested per search when the model does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u8>,
}

impl BackendEntry {
    fn apply_to(&self, base: &mut BackendEntry) {
        if self.kind.is_some() {
            base.kind = self.kind;
        }
        if self.base_url.is_some() {
            base.base_url = self.base_url.clone();
        }
        if self.api_key_env.is_some() {
            base.api_key_env = self.api_key_env.clone();
        }
        if self.max_results.is_some() {
            base.max_results = self.max_results;
        }
    }
}

/// A backend with everything resolved, including the key read out of the
/// environment. Holding the key rather than its variable name means the tool
/// cannot be built in a state where it discovers at call time that it has no
/// credential.
#[derive(Clone, Debug)]
pub struct Backend {
    pub id: String,
    pub kind: BackendKind,
    pub base_url: String,
    pub api_key: Option<String>,
    pub max_results: u8,
}

/// Results returned when neither the model nor the config asks for a number.
pub const DEFAULT_MAX_RESULTS: u8 = 5;

pub fn config_file(dir: &Path) -> PathBuf {
    dir.join("search.json")
}

/// Reads one layer's `search.json`. A missing file means no opinion, not an
/// error, so a machine that has never configured search is not a machine with a
/// problem.
pub fn load(dir: &Path) -> Result<SearchFile, String> {
    let path = config_file(dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(SearchFile::default());
    };
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Merges layers lowest-precedence first and resolves the selected backend.
///
/// Returns `None` for the backend whenever search is off or unusable, together
/// with the reasons — a caller that got `None` and no problems knows the user
/// simply has not turned it on, and one that got `None` with a problem has
/// something to show them.
pub fn merge(layers: Vec<SearchFile>) -> (Option<Backend>, Vec<String>) {
    let mut problems = Vec::new();
    let mut selected: Option<String> = None;
    let mut backends: BTreeMap<String, BackendEntry> = BTreeMap::new();

    for layer in layers {
        if layer.backend.is_some() {
            selected = layer.backend;
        }
        for (id, entry) in layer.backends {
            match backends.get_mut(&id) {
                Some(base) => entry.apply_to(base),
                None => {
                    backends.insert(id, entry);
                }
            }
        }
    }

    let Some(id) = selected.filter(|id| !id.is_empty()) else {
        return (None, problems);
    };
    let Some(entry) = backends.get(&id) else {
        problems.push(format!(
            "search.json selects the backend '{id}', which nothing defines"
        ));
        return (None, problems);
    };

    match resolve(&id, entry) {
        Ok(backend) => (Some(backend), problems),
        Err(e) => {
            problems.push(format!("search backend '{id}': {e}"));
            (None, problems)
        }
    }
}

/// Turns a merged entry into something that can make a request, or says what it
/// is missing.
fn resolve(id: &str, entry: &BackendEntry) -> Result<Backend, String> {
    let kind = entry
        .kind
        .ok_or_else(|| "needs a `kind` of `brave`, `tavily`, or `searxng`".to_string())?;

    let base_url = match entry.base_url.as_deref() {
        Some(url) => expand_env(url)?,
        None => kind
            .default_base_url()
            .ok_or_else(|| "needs a `base_url` naming your SearXNG instance".to_string())?
            .to_string(),
    };

    let api_key = match entry.api_key_env.as_deref() {
        Some(name) => {
            let key = std::env::var(name)
                .ok()
                .filter(|k| !k.trim().is_empty())
                // Reported rather than shrugged off: a search that goes out
                // without a key comes back 401, which reads as a bad key rather
                // than a missing one.
                .ok_or_else(|| format!("environment variable {name} is not set"))?;
            Some(key)
        }
        None if kind.needs_key() => {
            return Err("needs an `api_key_env` naming the variable that holds its key".into())
        }
        None => None,
    };

    Ok(Backend {
        id: id.to_string(),
        kind,
        base_url: base_url.trim_end_matches('/').to_string(),
        api_key,
        max_results: entry.max_results.unwrap_or(DEFAULT_MAX_RESULTS),
    })
}

/// The file a first run leaves behind: every backend spelled out, none of them
/// selected. Turning search on is then a one-word edit rather than a trip to
/// the documentation for a schema.
pub fn starter_file() -> SearchFile {
    SearchFile {
        backend: None,
        backends: BTreeMap::from([
            (
                "brave".to_string(),
                BackendEntry {
                    kind: Some(BackendKind::Brave),
                    api_key_env: Some("BRAVE_API_KEY".into()),
                    ..Default::default()
                },
            ),
            (
                "tavily".to_string(),
                BackendEntry {
                    kind: Some(BackendKind::Tavily),
                    api_key_env: Some("TAVILY_API_KEY".into()),
                    ..Default::default()
                },
            ),
            (
                "searxng".to_string(),
                BackendEntry {
                    kind: Some(BackendKind::Searxng),
                    base_url: Some("http://localhost:8888".into()),
                    ..Default::default()
                },
            ),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn file(json: &str) -> SearchFile {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        let loaded = load(dir.path()).unwrap();
        assert!(loaded.backend.is_none());
        assert!(loaded.backends.is_empty());
    }

    #[test]
    fn a_malformed_file_names_itself() {
        let dir = TempDir::new().unwrap();
        std::fs::write(config_file(dir.path()), "{ not json").unwrap();
        let err = load(dir.path()).unwrap_err();
        assert!(err.contains("search.json"), "{err}");
    }

    #[test]
    fn nothing_selected_means_search_is_off_and_that_is_not_a_problem() {
        let (backend, problems) = merge(vec![file(
            r#"{"backends": {"brave": {"kind": "brave", "api_key_env": "X"}}}"#,
        )]);
        assert!(backend.is_none());
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn an_absent_file_selects_nothing() {
        let (backend, problems) = merge(vec![SearchFile::default()]);
        assert!(backend.is_none());
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_searxng_instance_needs_no_key() {
        let (backend, problems) = merge(vec![file(
            r#"{"backend": "local",
                "backends": {"local": {"kind": "searxng", "base_url": "http://localhost:8888"}}}"#,
        )]);
        let backend = backend.unwrap_or_else(|| panic!("{problems:?}"));
        assert_eq!(backend.kind, BackendKind::Searxng);
        assert!(backend.api_key.is_none());
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_searxng_backend_without_a_url_says_so() {
        let (backend, problems) = merge(vec![file(
            r#"{"backend": "l", "backends": {"l": {"kind": "searxng"}}}"#,
        )]);
        assert!(backend.is_none());
        assert!(problems[0].contains("base_url"), "{problems:?}");
    }

    #[test]
    fn a_hosted_backend_defaults_its_base_url() {
        std::env::set_var("TAURUS_TEST_SEARCH_BRAVE", "bsa-key");
        let (backend, _) = merge(vec![file(
            r#"{"backend": "brave",
                "backends": {"brave": {"kind": "brave", "api_key_env": "TAURUS_TEST_SEARCH_BRAVE"}}}"#,
        )]);
        let backend = backend.unwrap();
        assert_eq!(backend.base_url, "https://api.search.brave.com");
        assert_eq!(backend.api_key.as_deref(), Some("bsa-key"));
        assert_eq!(backend.max_results, DEFAULT_MAX_RESULTS);
    }

    #[test]
    fn an_unset_key_variable_names_itself_rather_than_searching_without_one() {
        let (backend, problems) = merge(vec![file(
            r#"{"backend": "brave",
                "backends": {"brave": {"kind": "brave",
                                       "api_key_env": "TAURUS_TEST_SEARCH_DEFINITELY_UNSET"}}}"#,
        )]);
        assert!(backend.is_none());
        assert!(
            problems[0].contains("TAURUS_TEST_SEARCH_DEFINITELY_UNSET"),
            "{problems:?}"
        );
    }

    #[test]
    fn selecting_a_backend_nothing_defines_is_reported() {
        let (backend, problems) = merge(vec![file(r#"{"backend": "ghost"}"#)]);
        assert!(backend.is_none());
        assert!(problems[0].contains("ghost"), "{problems:?}");
    }

    #[test]
    fn a_workspace_layer_overrides_one_field_and_inherits_the_rest() {
        std::env::set_var("TAURUS_TEST_SEARCH_LAYERED", "key");
        let global = file(
            r#"{"backend": "brave",
                "backends": {"brave": {"kind": "brave",
                                       "api_key_env": "TAURUS_TEST_SEARCH_LAYERED",
                                       "max_results": 8}}}"#,
        );
        let workspace = file(r#"{"backends": {"brave": {"base_url": "http://proxy.internal"}}}"#);

        let (backend, problems) = merge(vec![global, workspace]);
        let backend = backend.unwrap_or_else(|| panic!("{problems:?}"));
        assert_eq!(backend.base_url, "http://proxy.internal");
        // Not reset just because the workspace layer omitted them.
        assert_eq!(backend.kind, BackendKind::Brave);
        assert_eq!(backend.max_results, 8);
    }

    #[test]
    fn a_workspace_can_switch_backends_without_redefining_one() {
        std::env::set_var("TAURUS_TEST_SEARCH_SWITCH", "key");
        let global = file(
            r#"{"backend": "brave",
                "backends": {"brave": {"kind": "brave", "api_key_env": "TAURUS_TEST_SEARCH_SWITCH"},
                             "local": {"kind": "searxng", "base_url": "http://localhost:8888"}}}"#,
        );
        let (backend, problems) = merge(vec![global, file(r#"{"backend": "local"}"#)]);
        let backend = backend.unwrap_or_else(|| panic!("{problems:?}"));
        assert_eq!(backend.id, "local");
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_workspace_can_add_a_backend_the_global_layer_never_saw() {
        let (backend, problems) = merge(vec![
            SearchFile::default(),
            file(
                r#"{"backend": "mine",
                    "backends": {"mine": {"kind": "searxng", "base_url": "http://searx.lan"}}}"#,
            ),
        ]);
        assert_eq!(
            backend.unwrap_or_else(|| panic!("{problems:?}")).base_url,
            "http://searx.lan"
        );
    }

    #[test]
    fn a_trailing_slash_does_not_double_up_in_request_urls() {
        let (backend, _) = merge(vec![file(
            r#"{"backend": "l", "backends": {"l": {"kind": "searxng",
                                                   "base_url": "http://localhost:8888/"}}}"#,
        )]);
        assert_eq!(backend.unwrap().base_url, "http://localhost:8888");
    }

    #[test]
    fn a_base_url_may_name_an_environment_variable() {
        std::env::set_var("TAURUS_TEST_SEARCH_URL", "http://searx.internal");
        let (backend, problems) = merge(vec![file(
            r#"{"backend": "l",
                "backends": {"l": {"kind": "searxng", "base_url": "${TAURUS_TEST_SEARCH_URL}"}}}"#,
        )]);
        assert_eq!(
            backend.unwrap_or_else(|| panic!("{problems:?}")).base_url,
            "http://searx.internal"
        );
    }

    #[test]
    fn the_starter_file_selects_nothing_but_round_trips() {
        let json = serde_json::to_string(&starter_file()).unwrap();
        let (backend, problems) = merge(vec![serde_json::from_str(&json).unwrap()]);
        assert!(backend.is_none(), "a first run must not enable search");
        assert!(problems.is_empty(), "{problems:?}");
        // Every backend the file offers is one word away from working.
        assert!(json.contains("brave") && json.contains("tavily") && json.contains("searxng"));
    }
}
