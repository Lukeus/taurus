//! The gate between a model's intent and the user's machine.
//!
//! Read-only work inside the workspace runs unattended, because prompting for
//! every file read makes the harness unusable. Anything that writes, executes,
//! or leaves the machine needs a decision, and "always" decisions persist so
//! the user grants each capability once rather than once per call.
//!
//! An "always" decision persists into one of two layers: this workspace, or
//! every workspace. Both are consulted, so a grant made once globally is not
//! asked again in a project that has never seen it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use ts_rs::TS;
use uuid::Uuid;

use crate::tool::{Effect, Tool, ToolError};

/// Where workspace decisions live, relative to the workspace root.
const ALLOWLIST_PATH: &str = ".taurus/permissions.json";

/// Where global decisions live, relative to the config home.
const GLOBAL_ALLOWLIST_FILE: &str = "permissions.json";

/// Which layer a persisted decision belongs to.
///
/// The same two layers the config files use. It is defined here because this
/// is the lowest crate that needs the distinction; `taurus-host` re-exports it
/// so there is one `Scope` and one TypeScript type across the whole harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Scope {
    Global,
    Workspace,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermissionRequest {
    pub id: String,
    pub tool: String,
    pub effect: Effect,
    /// One line describing this specific call.
    pub preview: String,
    /// What "always allow" would grant, in words, so the user knows the scope
    /// of the broader decision they are being offered.
    pub always_scope: String,
    /// The same, for granting it in every workspace. `None` when that is not
    /// on offer for this call, which is how a frontend knows not to show it.
    pub always_global_scope: Option<String>,
    #[ts(type = "unknown")]
    pub input: serde_json::Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PermissionDecision {
    AllowOnce,
    /// Persists for this workspace only.
    AllowAlways,
    /// Persists for every workspace. Offered only where
    /// [`PermissionRequest::always_global_scope`] is set.
    AllowAlwaysGlobal,
    Deny,
}

/// One persisted rule and the layer it came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AllowedRule {
    pub rule: String,
    pub scope: Scope,
}

/// Whether "allow everywhere" may be offered for this kind of call.
///
/// Never for command execution. A workspace grant for `git` is scoped to a
/// project the user has already decided to trust; the same grant globally
/// applies in every repository they ever open, including one just cloned to
/// look at. That is the decision most worth making per project, so the wider
/// option is not put on the table and a frontend that asks for it anyway is
/// narrowed back to the workspace.
///
/// This governs what the harness will *create*, not what it will honor: a
/// `run_command:*` rule hand-written into the global file still applies. That
/// is deliberate — editing that file is an explicit, informed act, and
/// silently ignoring configuration someone wrote is its own kind of surprise.
/// The gap only exists in the direction of "you had to mean it".
fn global_offerable(effect: Effect) -> bool {
    effect != Effect::Execute
}

/// Implemented by the UI layer. The agent loop blocks on this.
#[async_trait]
pub trait PermissionPrompt: Send + Sync {
    async fn request(&self, request: PermissionRequest) -> PermissionDecision;
}

/// Denies everything without asking. Used for headless runs and in tests where
/// a prompt would hang.
pub struct DenyAll;

#[async_trait]
impl PermissionPrompt for DenyAll {
    async fn request(&self, _: PermissionRequest) -> PermissionDecision {
        PermissionDecision::Deny
    }
}

/// Allows everything without asking. For tests only; never wire this to the UI.
pub struct AllowAll;

#[async_trait]
impl PermissionPrompt for AllowAll {
    async fn request(&self, _: PermissionRequest) -> PermissionDecision {
        PermissionDecision::AllowOnce
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Allowlist {
    /// Rule strings, e.g. `write_file` or `run_command:git`.
    #[serde(default)]
    allowed: BTreeSet<String>,
}

/// Both layers, behind one lock.
///
/// One lock rather than two because every read consults both: taking them
/// separately would let a grant land between the two checks and be missed.
#[derive(Debug, Default)]
struct Allowlists {
    global: Allowlist,
    workspace: Allowlist,
}

impl Allowlists {
    fn permits(&self, rule: &str) -> bool {
        self.global.allowed.contains(rule) || self.workspace.allowed.contains(rule)
    }

    fn layer(&mut self, scope: Scope) -> &mut Allowlist {
        match scope {
            Scope::Global => &mut self.global,
            Scope::Workspace => &mut self.workspace,
        }
    }
}

pub struct PermissionEngine {
    workspace: PathBuf,
    /// The config home, which holds the global `permissions.json`.
    global: PathBuf,
    prompt: Box<dyn PermissionPrompt>,
    allowlists: Mutex<Allowlists>,
}

impl PermissionEngine {
    pub fn new(
        workspace: impl Into<PathBuf>,
        global: impl Into<PathBuf>,
        prompt: Box<dyn PermissionPrompt>,
    ) -> Self {
        let workspace = workspace.into();
        let global = global.into();
        let allowlists = Allowlists {
            global: read_allowlist(&global_allowlist_file(&global)),
            workspace: read_allowlist(&workspace_allowlist_file(&workspace)),
        };
        Self {
            workspace,
            global,
            prompt,
            allowlists: Mutex::new(allowlists),
        }
    }

    /// Decides whether `tool` may run with `input`, prompting if needed.
    pub async fn check(&self, tool: &dyn Tool, input: &serde_json::Value) -> Result<(), ToolError> {
        if tool.effect() == Effect::Read {
            return Ok(());
        }

        let rule = rule_for(tool, input);
        if self.allowlists.lock().await.permits(&rule) {
            return Ok(());
        }

        let offer_global = global_offerable(tool.effect());
        let request = PermissionRequest {
            id: Uuid::new_v4().to_string(),
            tool: tool.name().to_string(),
            effect: tool.effect(),
            preview: tool.preview(input),
            always_scope: describe_rule(&rule, tool.effect(), Scope::Workspace),
            always_global_scope: offer_global
                .then(|| describe_rule(&rule, tool.effect(), Scope::Global)),
            input: input.clone(),
        };

        let scope = match self.prompt.request(request).await {
            PermissionDecision::AllowOnce => return Ok(()),
            PermissionDecision::Deny => return Err(ToolError::Denied),
            PermissionDecision::AllowAlways => Scope::Workspace,
            PermissionDecision::AllowAlwaysGlobal if offer_global => Scope::Global,
            // A frontend that offered a global grant where one was not on
            // offer gets the narrower decision honored, never the wider one.
            PermissionDecision::AllowAlwaysGlobal => Scope::Workspace,
        };

        let mut lists = self.allowlists.lock().await;
        lists.layer(scope).allowed.insert(rule);
        self.save(scope, lists.layer(scope));
        Ok(())
    }

    pub async fn allowed_rules(&self) -> Vec<AllowedRule> {
        let lists = self.allowlists.lock().await;
        [
            (Scope::Global, &lists.global),
            (Scope::Workspace, &lists.workspace),
        ]
        .into_iter()
        .flat_map(|(scope, list)| {
            list.allowed.iter().map(move |rule| AllowedRule {
                rule: rule.clone(),
                scope,
            })
        })
        .collect()
    }

    /// Removes a rule from one layer.
    ///
    /// Scoped rather than "wherever it is": a rule can be granted in both, and
    /// revoking the workspace copy must not silently drop a global grant the
    /// user made for every project.
    pub async fn revoke(&self, rule: &str, scope: Scope) {
        let mut lists = self.allowlists.lock().await;
        lists.layer(scope).allowed.remove(rule);
        self.save(scope, lists.layer(scope));
    }

    fn save(&self, scope: Scope, list: &Allowlist) {
        let path = match scope {
            Scope::Global => global_allowlist_file(&self.global),
            Scope::Workspace => workspace_allowlist_file(&self.workspace),
        };
        write_allowlist(&path, list);
    }
}

/// The unit an "always" decision grants.
///
/// Shell commands are keyed by their leading word so approving `git` does not
/// also approve `rm`. A call that names a URL is keyed by that URL's host, for
/// the same reason: "always allow fetching docs.rs" is a decision about a site
/// the user trusts, and the tool-wide version of it is a standing grant to
/// reach anywhere on the internet.
///
/// Everything else is keyed by name: the user is saying "this tool may write in
/// this workspace", which is the granularity they actually reason about.
fn rule_for(tool: &dyn Tool, input: &serde_json::Value) -> String {
    if tool.effect() == Effect::Execute {
        if let Some(program) = input
            .get("command")
            .and_then(|c| c.as_str())
            .and_then(leading_word)
        {
            return format!("{}:{program}", tool.name());
        }
    }
    if tool.effect() == Effect::Network {
        if let Some(host) = input.get("url").and_then(|u| u.as_str()).and_then(host_of) {
            return format!("{}:{host}", tool.name());
        }
    }
    tool.name().to_string()
}

fn describe_rule(rule: &str, effect: Effect, scope: Scope) -> String {
    let where_ = match scope {
        Scope::Global => "in every workspace",
        Scope::Workspace => "in this workspace",
    };
    match rule.split_once(':') {
        Some((tool, qualifier)) if effect == Effect::Network => {
            format!("Always allow `{tool}` to reach {qualifier} {where_}")
        }
        Some((tool, program)) => {
            format!("Always allow `{tool}` to run `{program}` commands {where_}")
        }
        None => format!("Always allow `{rule}` {where_}"),
    }
}

/// Host of an absolute URL, without scheme, port, path, or credentials.
///
/// Deliberately not a URL parser: what a grant is keyed by has to be something
/// the user can read back out of `permissions.json` and recognize, and anything
/// this rejects simply falls back to a per-call prompt.
///
/// It does have to agree with the parser the request will actually use about
/// where the authority *ends*, though, or the two disagree about which host is
/// being granted. WHATWG — which is what `url`, and so `reqwest`, implements —
/// ends it at a backslash as readily as a slash, so `https://a.test\@b.test/`
/// reaches `a.test` while a naive split would key it under `b.test`. Getting
/// that wrong spends a grant for one host on a request to another, so the
/// terminator set here matches theirs.
fn host_of(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://")?.1;
    let authority = after_scheme
        .split(['/', '\\', '?', '#'])
        .next()
        .filter(|a| !a.is_empty())?;
    // `user:pass@host` — the credential is not part of what is being granted.
    let host = authority.rsplit('@').next()?;
    let host = match host.rfind(':') {
        // A colon inside brackets is an IPv6 address, not a port.
        Some(i) if !host.ends_with(']') => &host[..i],
        _ => host,
    };
    // Hosts are case-insensitive, so `DOCS.RS` must not bank a second grant
    // alongside the `docs.rs` the user already approved.
    Some(host)
        .filter(|h| !h.is_empty())
        .map(|h| h.to_ascii_lowercase())
}

/// First bare word of a command line, ignoring `VAR=value` prefixes.
fn leading_word(command: &str) -> Option<&str> {
    command
        .split_whitespace()
        .find(|w| !w.contains('='))
        .map(|w| w.trim_start_matches(['(', '{']))
        .filter(|w| !w.is_empty())
}

fn workspace_allowlist_file(workspace: &Path) -> PathBuf {
    workspace.join(ALLOWLIST_PATH)
}

fn global_allowlist_file(home: &Path) -> PathBuf {
    home.join(GLOBAL_ALLOWLIST_FILE)
}

fn read_allowlist(path: &Path) -> Allowlist {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_allowlist(path: &Path, list: &Allowlist) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(list) {
        // A failure to persist must not fail the tool call the user just
        // approved; they simply get asked again next time.
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ToolContext, ToolResult};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Fake {
        name: &'static str,
        effect: Effect,
    }

    #[async_trait]
    impl Tool for Fake {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "fake"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn effect(&self) -> Effect {
            self.effect
        }
        async fn execute(&self, _: serde_json::Value, _: &ToolContext) -> ToolResult {
            Ok(String::new())
        }
    }

    struct Counting {
        decision: PermissionDecision,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl PermissionPrompt for Counting {
        async fn request(&self, _: PermissionRequest) -> PermissionDecision {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.decision
        }
    }

    /// Keeps the request it was asked, so a test can assert on what the user
    /// would have been offered.
    struct Recording {
        seen: Arc<Mutex<Option<Option<String>>>>,
        decision: PermissionDecision,
    }

    #[async_trait]
    impl PermissionPrompt for Recording {
        async fn request(&self, request: PermissionRequest) -> PermissionDecision {
            *self.seen.lock().await = Some(request.always_global_scope);
            self.decision
        }
    }

    /// A workspace and a config home, kept apart so a test can tell which
    /// layer a decision landed in.
    struct Homes {
        workspace: tempfile::TempDir,
        global: tempfile::TempDir,
    }

    impl Homes {
        fn new() -> Self {
            Self {
                workspace: tempfile::tempdir().unwrap(),
                global: tempfile::tempdir().unwrap(),
            }
        }

        fn engine(&self, prompt: Box<dyn PermissionPrompt>) -> PermissionEngine {
            PermissionEngine::new(self.workspace.path(), self.global.path(), prompt)
        }

        fn wrote_workspace(&self) -> bool {
            workspace_allowlist_file(self.workspace.path()).is_file()
        }

        fn wrote_global(&self) -> bool {
            global_allowlist_file(self.global.path()).is_file()
        }
    }

    fn engine(decision: PermissionDecision) -> (PermissionEngine, Arc<AtomicUsize>, Homes) {
        let homes = Homes::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let engine = homes.engine(Box::new(Counting {
            decision,
            calls: calls.clone(),
        }));
        (engine, calls, homes)
    }

    #[tokio::test]
    async fn reads_never_prompt() {
        let (engine, calls, _dir) = engine(PermissionDecision::Deny);
        let tool = Fake {
            name: "read_file",
            effect: Effect::Read,
        };
        assert!(engine.check(&tool, &serde_json::json!({})).await.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn writes_prompt_and_can_be_denied() {
        let (engine, calls, _dir) = engine(PermissionDecision::Deny);
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };
        assert!(matches!(
            engine.check(&tool, &serde_json::json!({})).await,
            Err(ToolError::Denied)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn allow_once_does_not_persist() {
        let (engine, calls, _dir) = engine(PermissionDecision::AllowOnce);
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };
        for _ in 0..2 {
            engine.check(&tool, &serde_json::json!({})).await.unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn allow_always_suppresses_later_prompts() {
        let (engine, calls, _dir) = engine(PermissionDecision::AllowAlways);
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };
        for _ in 0..3 {
            engine.check(&tool, &serde_json::json!({})).await.unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn allow_always_survives_a_restart() {
        let homes = Homes::new();
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };
        let first = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowAlways,
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        first.check(&tool, &serde_json::json!({})).await.unwrap();

        // A fresh engine over the same workspace must honor the stored rule
        // even though its prompt denies everything.
        let second = homes.engine(Box::new(DenyAll));
        assert!(second.check(&tool, &serde_json::json!({})).await.is_ok());
    }

    #[tokio::test]
    async fn a_workspace_grant_stays_in_the_workspace() {
        let (engine, _, homes) = engine(PermissionDecision::AllowAlways);
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };
        engine.check(&tool, &serde_json::json!({})).await.unwrap();

        assert!(homes.wrote_workspace());
        assert!(
            !homes.wrote_global(),
            "a workspace decision must not touch the global layer"
        );
    }

    #[tokio::test]
    async fn a_global_grant_carries_into_a_workspace_that_never_saw_it() {
        let global = tempfile::tempdir().unwrap();
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };

        let first = tempfile::tempdir().unwrap();
        let engine = PermissionEngine::new(
            first.path(),
            global.path(),
            Box::new(Counting {
                decision: PermissionDecision::AllowAlwaysGlobal,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
        );
        engine.check(&tool, &serde_json::json!({})).await.unwrap();
        assert!(
            !workspace_allowlist_file(first.path()).is_file(),
            "a global decision must not also be written to the workspace"
        );

        // A different workspace, sharing only the config home, and a prompt
        // that would refuse. The stored global rule has to carry it.
        let second = tempfile::tempdir().unwrap();
        let elsewhere = PermissionEngine::new(second.path(), global.path(), Box::new(DenyAll));
        assert!(elsewhere.check(&tool, &serde_json::json!({})).await.is_ok());
    }

    #[tokio::test]
    async fn running_commands_is_never_offered_globally() {
        let homes = Homes::new();
        let tool = Fake {
            name: "run_command",
            effect: Effect::Execute,
        };

        // What the request offers, and what the engine does if a frontend
        // answers with the wider grant regardless.
        let offered = Arc::new(Mutex::new(None));
        let recorder = Recording {
            seen: offered.clone(),
            decision: PermissionDecision::AllowAlwaysGlobal,
        };
        let engine = homes.engine(Box::new(recorder));
        engine
            .check(&tool, &serde_json::json!({"command": "git status"}))
            .await
            .unwrap();

        assert!(
            offered.lock().await.as_ref().unwrap().is_none(),
            "execution must not offer an everywhere grant"
        );
        assert!(
            !homes.wrote_global(),
            "an unofferable global grant must be narrowed, not honored"
        );
        assert!(homes.wrote_workspace());
    }

    #[tokio::test]
    async fn revoking_one_layer_leaves_the_other_granted() {
        let homes = Homes::new();
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };

        // The same rule granted in both layers, as happens when someone
        // approves it per project and later approves it everywhere.
        let engine = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowAlways,
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        engine.check(&tool, &serde_json::json!({})).await.unwrap();
        engine.revoke("write_file", Scope::Workspace).await;

        let global = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowAlwaysGlobal,
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        global.check(&tool, &serde_json::json!({})).await.unwrap();

        global.revoke("write_file", Scope::Workspace).await;
        assert!(
            global.check(&tool, &serde_json::json!({})).await.is_ok(),
            "revoking the workspace copy dropped the global grant"
        );

        global.revoke("write_file", Scope::Global).await;
        let fresh = homes.engine(Box::new(DenyAll));
        assert!(matches!(
            fresh.check(&tool, &serde_json::json!({})).await,
            Err(ToolError::Denied)
        ));
    }

    #[tokio::test]
    async fn listed_rules_say_which_layer_they_came_from() {
        let homes = Homes::new();
        let write = Fake {
            name: "write_file",
            effect: Effect::Write,
        };
        let net = Fake {
            name: "fetch",
            effect: Effect::Network,
        };

        let engine = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowAlways,
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        engine.check(&write, &serde_json::json!({})).await.unwrap();

        let global = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowAlwaysGlobal,
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        global.check(&net, &serde_json::json!({})).await.unwrap();

        let rules = global.allowed_rules().await;
        assert!(rules.contains(&AllowedRule {
            rule: "fetch".into(),
            scope: Scope::Global,
        }));
        assert!(rules.contains(&AllowedRule {
            rule: "write_file".into(),
            scope: Scope::Workspace,
        }));
    }

    #[test]
    fn a_url_is_keyed_by_its_host_and_nothing_else() {
        assert_eq!(
            host_of("https://docs.rs/serde/latest").as_deref(),
            Some("docs.rs")
        );
        assert_eq!(
            host_of("http://localhost:8888/search?q=x").as_deref(),
            Some("localhost")
        );
        assert_eq!(
            host_of("https://user:pw@private.test/x").as_deref(),
            Some("private.test")
        );
        assert_eq!(host_of("https://[::1]:9000/x").as_deref(), Some("[::1]"));
        assert_eq!(
            host_of("https://example.test").as_deref(),
            Some("example.test")
        );
    }

    #[test]
    fn something_that_is_not_a_url_keys_nothing_and_so_keeps_prompting() {
        assert_eq!(host_of("not a url"), None);
        assert_eq!(host_of("https://"), None);
    }

    #[test]
    fn a_backslash_ends_the_authority_the_way_the_url_parser_says_it_does() {
        // WHATWG treats `\` like `/`, so this reaches evil.test. Keying it
        // under docs.rs would spend a grant for one host on another.
        assert_eq!(
            host_of("https://evil.test\\x@docs.rs/serde").as_deref(),
            Some("evil.test")
        );
    }

    #[test]
    fn host_case_does_not_bank_a_second_grant() {
        assert_eq!(host_of("https://DOCS.RS/serde").as_deref(), Some("docs.rs"));
    }

    #[tokio::test]
    async fn a_network_grant_covers_one_host_rather_than_the_whole_internet() {
        let homes = Homes::new();
        let fetch = Fake {
            name: "fetch_url",
            effect: Effect::Network,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let engine = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowAlways,
            calls: calls.clone(),
        }));

        engine
            .check(&fetch, &serde_json::json!({"url": "https://docs.rs/serde"}))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A second page on the granted host does not ask again.
        engine
            .check(&fetch, &serde_json::json!({"url": "https://docs.rs/tokio"}))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A different host is a different decision.
        engine
            .check(&fetch, &serde_json::json!({"url": "https://evil.test/x"}))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        assert!(engine.allowed_rules().await.contains(&AllowedRule {
            rule: "fetch_url:docs.rs".into(),
            scope: Scope::Workspace,
        }));
    }

    #[test]
    fn a_network_grant_is_described_as_reaching_a_host_not_running_a_command() {
        let described = describe_rule("fetch_url:docs.rs", Effect::Network, Scope::Workspace);
        assert!(described.contains("reach docs.rs"), "{described}");
        assert!(!described.contains("commands"), "{described}");
    }

    #[tokio::test]
    async fn a_search_without_a_url_is_still_keyed_by_tool_name() {
        let homes = Homes::new();
        let search = Fake {
            name: "web_search",
            effect: Effect::Network,
        };
        let engine = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowAlways,
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        engine
            .check(&search, &serde_json::json!({"query": "rust"}))
            .await
            .unwrap();
        assert!(engine.allowed_rules().await.contains(&AllowedRule {
            rule: "web_search".into(),
            scope: Scope::Workspace,
        }));
    }

    #[tokio::test]
    async fn a_hand_written_global_command_rule_is_still_honored() {
        // The restriction on global execute grants is about what the harness
        // will create, not what it will obey. Someone who edits the global
        // file by hand has made an explicit choice, and quietly ignoring it
        // would be its own surprise. Pinned so the distinction stays a
        // decision rather than drifting either way by accident.
        let homes = Homes::new();
        std::fs::write(
            global_allowlist_file(homes.global.path()),
            r#"{"allowed": ["run_command:git"]}"#,
        )
        .unwrap();

        let engine = homes.engine(Box::new(DenyAll));
        assert!(engine
            .check(
                &Fake {
                    name: "run_command",
                    effect: Effect::Execute,
                },
                &serde_json::json!({"command": "git status"}),
            )
            .await
            .is_ok());
    }

    #[test]
    fn the_two_scopes_are_described_differently() {
        assert!(
            describe_rule("write_file", Effect::Write, Scope::Workspace).contains("this workspace")
        );
        assert!(
            describe_rule("write_file", Effect::Write, Scope::Global).contains("every workspace")
        );
    }

    #[tokio::test]
    async fn approving_one_command_does_not_approve_another() {
        let (engine, calls, _dir) = engine(PermissionDecision::AllowAlways);
        let tool = Fake {
            name: "run_command",
            effect: Effect::Execute,
        };
        engine
            .check(&tool, &serde_json::json!({"command": "git status"}))
            .await
            .unwrap();
        engine
            .check(&tool, &serde_json::json!({"command": "git log"}))
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "second git call reused the rule"
        );

        engine
            .check(&tool, &serde_json::json!({"command": "rm -rf /"}))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "rm must prompt separately");
    }

    #[test]
    fn leading_word_skips_env_assignments() {
        assert_eq!(leading_word("FOO=1 BAR=2 git status"), Some("git"));
        assert_eq!(leading_word("  ls -la"), Some("ls"));
        assert_eq!(leading_word(""), None);
    }
}
