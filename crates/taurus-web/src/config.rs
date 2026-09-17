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
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use taurus_tools::expand_env;

/// Which service a backend talks to. Each speaks its own dialect; the kind is
/// what selects the request shape and the response parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
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
    /// Lets `fetch_url` reach loopback and private-network addresses.
    ///
    /// Off by default. It governs the tool rather than any one backend — a
    /// search backend's URL is one the user wrote down, and is never subject to
    /// this — but it lives here because `search.json` is the file the web tools
    /// are configured from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_private_hosts: Option<bool>,
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

/// A backend with everything resolved that can be resolved without spending
/// anything: the URL, the limits, and whether a key is there to be read.
///
/// The key itself is not read here. Where it comes from can be the macOS
/// keychain, whose reads raise a permission dialog, and this is built on every
/// config reload — including the one a window waits on before it can show
/// anything. A dialog nobody has noticed yet held that window for as long as it
/// stayed open. So the question asked at build time is only "is there a key",
/// which is enough to keep a tool that is guaranteed to fail from being offered,
/// and the read happens on the first search, when there is a person waiting on
/// the answer to explain a dialog to. See [`ApiKey`].
#[derive(Clone, Debug)]
pub struct Backend {
    pub id: String,
    pub kind: BackendKind,
    pub base_url: String,
    /// `None` for a backend that sends no key, which today is SearXNG.
    pub api_key: Option<ApiKey>,
    pub max_results: u8,
    /// File-level, not really the backend's own — but the web tools are
    /// registered together and only when a backend resolves, so this is the one
    /// value that reaches them both.
    pub allow_private_hosts: bool,
}

/// Where a backend's key comes from, asked the two questions that cost
/// different amounts to answer.
///
/// Implemented by the host, where the credential store lives; see
/// [`merge_with`] for why it cannot be reached from this crate.
pub trait KeySource: Send + Sync {
    /// Whether [`Self::read`] would find a key, without reading one.
    fn present(&self, id: &str, variable: Option<&str>) -> bool;

    /// The key. Can block — on a keychain, for as long as its dialog is open —
    /// so it is only ever called off the async runtime.
    fn read(&self, id: &str, variable: Option<&str>) -> Option<String>;
}

/// The environment alone, which is where keys come from when no richer source
/// is supplied. Nothing in it can block, so both answers are the same read.
pub struct Environment;

impl KeySource for Environment {
    fn present(&self, id: &str, variable: Option<&str>) -> bool {
        env_key(id, variable).is_some()
    }

    fn read(&self, id: &str, variable: Option<&str>) -> Option<String> {
        env_key(id, variable)
    }
}

/// A backend's key, read the first time a search needs it and kept after that.
///
/// Kept per backend rather than per search, because a keychain read is not free
/// even once allowed. A key saved or cleared through Taurus rebuilds the backend
/// and so starts a fresh one of these; nothing else needs to invalidate it.
///
/// A read that finds nothing is not kept: a key the keychain refused because a
/// dialog was dismissed is exactly the one to ask for again on the next search.
#[derive(Clone)]
pub struct ApiKey {
    id: String,
    variable: Option<String>,
    source: Arc<dyn KeySource>,
    read: Arc<OnceLock<String>>,
}

impl ApiKey {
    pub fn new(id: &str, variable: Option<&str>, source: Arc<dyn KeySource>) -> Self {
        Self {
            id: id.to_string(),
            variable: variable.map(str::to_string),
            source,
            read: Arc::new(OnceLock::new()),
        }
    }

    /// The key, reading it if this is the first time it has been asked for.
    /// Blocks on that first read; see [`KeySource::read`].
    pub fn read(&self) -> Option<String> {
        if let Some(key) = self.read.get() {
            return Some(key.clone());
        }
        let key = self.source.read(&self.id, self.variable.as_deref())?;
        Some(self.read.get_or_init(|| key).clone())
    }

    /// What to tell somebody whose search has no key to send.
    pub fn missing(&self) -> String {
        missing_key(self.variable.as_deref())
    }
}

/// Never the key: a `Backend` is logged, and this is inside it.
impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKey")
            .field("id", &self.id)
            .field("variable", &self.variable)
            .field("read", &self.read.get().is_some())
            .finish()
    }
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
    merge_with(layers, Arc::new(Environment))
}

/// Reads a key straight out of the environment. The behaviour when no richer
/// source is supplied, and the fallback inside every richer source there is.
pub fn env_key(_id: &str, variable: Option<&str>) -> Option<String> {
    variable
        .and_then(|name| std::env::var(name).ok())
        .filter(|key| !key.trim().is_empty())
}

/// [`merge`], with the caller deciding where a backend's key comes from.
///
/// The credential store lives in `taurus-host`, which depends on this crate, so
/// it cannot be reached from here. Rather than invert that, the host passes in
/// how to find a key — which also keeps the precedence rule in one place, next
/// to the identical one for model providers, instead of implemented twice and
/// drifting.
///
/// `keys` is handed the backend's id and the variable name its config names,
/// if any. Only [`KeySource::present`] is asked here; the read waits for a
/// search.
pub fn merge_with(
    layers: Vec<SearchFile>,
    keys: Arc<dyn KeySource>,
) -> (Option<Backend>, Vec<String>) {
    let mut problems = Vec::new();
    let mut selected: Option<String> = None;
    let mut allow_private_hosts = false;
    let mut backends: BTreeMap<String, BackendEntry> = BTreeMap::new();

    for layer in layers {
        if layer.backend.is_some() {
            selected = layer.backend;
        }
        if let Some(allow) = layer.allow_private_hosts {
            allow_private_hosts = allow;
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

    match resolve(&id, entry, keys, allow_private_hosts) {
        Ok(backend) => (Some(backend), problems),
        Err(e) => {
            problems.push(format!("search backend '{id}': {e}"));
            (None, problems)
        }
    }
}

/// Turns a merged entry into something that can make a request, or says what it
/// is missing.
fn resolve(
    id: &str,
    entry: &BackendEntry,
    keys: Arc<dyn KeySource>,
    allow_private_hosts: bool,
) -> Result<Backend, String> {
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

    let variable = entry.api_key_env.as_deref();
    // Reported rather than shrugged off: a search that goes out without a key
    // comes back 401, which reads as a bad key rather than a missing one.
    let api_key = if kind.needs_key() {
        if !keys.present(id, variable) {
            return Err(missing_key(variable));
        }
        Some(ApiKey::new(id, variable, keys))
    } else {
        None
    };

    Ok(Backend {
        id: id.to_string(),
        kind,
        base_url: base_url.trim_end_matches('/').to_string(),
        api_key,
        max_results: entry.max_results.unwrap_or(DEFAULT_MAX_RESULTS),
        allow_private_hosts,
    })
}

/// Why a backend has no key, in terms of the two places one can come from.
fn missing_key(variable: Option<&str>) -> String {
    match variable {
        Some(name) => format!("needs a key — none saved, and {name} is not set"),
        None => "needs a key; save one in Settings › Search".into(),
    }
}

/// The file a first run leaves behind: every backend spelled out, none of them
/// selected. Turning search on is then a one-word edit rather than a trip to
/// the documentation for a schema.
pub fn starter_file() -> SearchFile {
    SearchFile {
        backend: None,
        allow_private_hosts: None,
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
        assert_eq!(
            backend.api_key.as_ref().and_then(ApiKey::read).as_deref(),
            Some("bsa-key")
        );
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

    /// A source that counts what it is asked, and answers from a slot a test
    /// can empty.
    #[derive(Default)]
    struct Counted {
        key: std::sync::Mutex<Option<String>>,
        present: std::sync::atomic::AtomicUsize,
        reads: std::sync::atomic::AtomicUsize,
    }

    impl KeySource for Counted {
        fn present(&self, _: &str, _: Option<&str>) -> bool {
            self.present
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.key.lock().unwrap().is_some()
        }

        fn read(&self, _: &str, _: Option<&str>) -> Option<String> {
            self.reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.key.lock().unwrap().clone()
        }
    }

    const KEYED: &str = r#"{"backend": "tavily", "backends": {"tavily": {"kind": "tavily"}}}"#;

    #[test]
    fn building_a_backend_asks_whether_there_is_a_key_and_never_reads_it() {
        // The read is the one that can wait on a keychain dialog, and this is
        // built on the reload a window waits on before it can draw.
        let keys = Arc::new(Counted::default());
        *keys.key.lock().unwrap() = Some("tvly-key".into());
        let (backend, problems) = merge_with(vec![file(KEYED)], keys.clone());
        let backend = backend.unwrap_or_else(|| panic!("{problems:?}"));
        assert_eq!(keys.present.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(keys.reads.load(std::sync::atomic::Ordering::Relaxed), 0);

        let key = backend.api_key.unwrap();
        assert_eq!(key.read().as_deref(), Some("tvly-key"));
        assert_eq!(key.read().as_deref(), Some("tvly-key"));
        assert_eq!(
            keys.reads.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a key already read was read again"
        );
    }

    #[test]
    fn a_key_that_could_not_be_read_is_asked_for_again() {
        // A dismissed keychain dialog is a `None` that the next search should
        // not inherit.
        let keys = Arc::new(Counted::default());
        *keys.key.lock().unwrap() = Some("tvly-key".into());
        let (backend, _) = merge_with(vec![file(KEYED)], keys.clone());
        let key = backend.unwrap().api_key.unwrap();

        let saved = keys.key.lock().unwrap().take();
        assert_eq!(key.read(), None);
        *keys.key.lock().unwrap() = saved;
        assert_eq!(key.read().as_deref(), Some("tvly-key"));
    }

    #[test]
    fn a_backend_with_no_key_present_is_not_built() {
        let keys = Arc::new(Counted::default());
        let (backend, problems) = merge_with(vec![file(KEYED)], keys.clone());
        assert!(backend.is_none());
        assert!(problems[0].contains("Settings › Search"), "{problems:?}");
        assert_eq!(keys.reads.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn the_key_never_appears_in_a_logged_backend() {
        let keys = Arc::new(Counted::default());
        *keys.key.lock().unwrap() = Some("tvly-secret-value".into());
        let (backend, _) = merge_with(vec![file(KEYED)], keys);
        let backend = backend.unwrap();
        backend.api_key.as_ref().unwrap().read();
        assert!(!format!("{backend:?}").contains("tvly-secret-value"));
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
