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

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use ts_rs::TS;
use uuid::Uuid;

use crate::diff::FileDiff;
use taurus_process::Programs;

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
///
/// `JsonSchema` as well as `TS`, because `read_note` takes one: which notebook a
/// note is in is the same distinction, and a second enum spelling it for the
/// model would be a third place for these two words to drift.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
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
    /// What the call would do to a file, when that is knowable before it runs.
    ///
    /// Set for `write_file` and `edit_file`, and `None` everywhere else — a
    /// command line and a URL have no before-and-after to show. Also `None`
    /// when the target is not text or cannot be read: a diff is evidence
    /// offered with the decision, never a precondition for making one.
    pub diff: Option<FileDiff>,
    /// What "always allow" would grant, in words, so the user knows the scope
    /// of the broader decision they are being offered.
    pub always_scope: String,
    /// The same, for granting it in every workspace. `None` when that is not
    /// on offer for this call, which is how a frontend knows not to show it.
    pub always_global_scope: Option<String>,
    /// Whether the workspace-wide grant is on offer at all.
    ///
    /// False in an untrusted workspace, which has no layer to keep a standing
    /// decision in. A frontend that shows the button anyway does no harm — the
    /// engine narrows the decision to this call — but it would be offering a
    /// permanence it cannot deliver, so it is told not to.
    #[serde(default = "offered_by_default")]
    pub offer_always: bool,
    #[ts(type = "unknown")]
    pub input: serde_json::Value,
    /// Which conversation is asking.
    ///
    /// A window shows one conversation at a time and a turn keeps running in
    /// the ones it does not, so a prompt can arrive from somewhere other than
    /// what is on screen. Without this the dialog names the call and nothing
    /// else, and two turns running at once are indistinguishable at the one
    /// moment that matters.
    ///
    /// `None` where there is no conversation — a tool run directly, an
    /// example, a test.
    #[ts(optional)]
    pub session: Option<String>,
}

/// Who is being asked, and whether anybody is there.
///
/// Both are facts about the turn rather than about the call, and the gate has
/// no other way to learn either: a [`PermissionEngine`] is shared by every
/// conversation in a process.
#[derive(Clone, Debug, Default)]
pub struct Asking {
    /// The conversation, carried onto the request. See
    /// [`PermissionRequest::session`].
    pub session: Option<String>,
    /// Whether a call that needs a decision is refused instead of waiting for
    /// one.
    ///
    /// The default is to wait, and waiting is right when somebody is there:
    /// the turn keeps its place, and answering it hours later carries on from
    /// exactly where it stopped. It is wrong when nobody is, which is the case
    /// this exists for — a run left going overnight should spend the night
    /// working and report in the morning what it could not do, rather than
    /// stopping on the first question at ten past midnight.
    ///
    /// It never widens anything. What a standing grant already permits does
    /// not reach a prompt at all, so this only decides what happens to the
    /// calls that would have been asked about, and the only answer it gives is
    /// no.
    pub unattended: bool,
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
/// Older payloads, and every hand-built request in a test, mean the workspace
/// grant to be on offer. Only the untrusted case turns it off.
fn offered_by_default() -> bool {
    true
}

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
    /// The file was there and could not be read as a list.
    ///
    /// Such a layer grants nothing it held, and is never written back. Reading
    /// a stray comma as "no grants" and then saving the next one would put a
    /// list of one over every rule the file held, and the user would find out
    /// by being asked about everything again with nothing to say why. A grant
    /// made while the file is broken holds for the rest of the session.
    #[serde(skip)]
    unreadable: bool,
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
    /// Whether this workspace's own `permissions.json` counts.
    ///
    /// False for a workspace the user has not trusted. A committed allowlist is
    /// the one file in a repository that hands over capability with no prompt
    /// at all — `{"allowed": ["run_command:git"]}` in a clone is a standing
    /// grant nobody made — so it is read only once the workspace is trusted.
    /// See `taurus_host::trust`.
    ///
    /// It governs writing too. With no workspace layer to write into, "always
    /// allow in this workspace" would persist a decision nothing would ever
    /// read back, so the offer is withdrawn and the grant narrows to this call.
    workspace_rules: bool,
    /// What each conversation has allowed once, by the conversation's id.
    ///
    /// Kept for the moment somebody leaves. "Allow once" is a decision about
    /// the call in front of them, and the rules it would have taken to make
    /// that decision standing are exactly what an unattended run is refused
    /// next. So the switch that sets a conversation to unattended offers them
    /// back, and this is where it finds them. Never consulted by [`check`]:
    /// remembering a rule here allows nothing by itself.
    ///
    /// [`check`]: Self::check
    once: Mutex<HashMap<String, BTreeSet<String>>>,
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
            workspace_rules: true,
            once: Mutex::new(HashMap::new()),
        }
    }

    /// Whether this workspace's layer is honored, defaulting to yes.
    ///
    /// Applied after construction rather than as a fourth argument because the
    /// caller that knows the answer is the host, and the answer it gives is
    /// about trust rather than about permissions — see `taurus_host::trust`.
    /// Turning it off discards the layer that was just read, so no rule from an
    /// untrusted workspace is ever consulted, not even the one already in hand.
    #[must_use]
    pub fn with_workspace_rules(mut self, allowed: bool) -> Self {
        self.workspace_rules = allowed;
        if !allowed {
            self.allowlists.get_mut().workspace = Allowlist::default();
        }
        self
    }

    /// Decides whether `tool` may run with `input`, prompting if needed.
    ///
    /// `asking` is the turn's half of the question: which conversation wants
    /// this, and whether there is anybody to answer. See [`Asking`].
    pub async fn check(
        &self,
        tool: &dyn Tool,
        input: &serde_json::Value,
        asking: &Asking,
    ) -> Result<(), ToolError> {
        if tool.effect() == Effect::Read {
            return Ok(());
        }

        let keys = keys_for(tool, input);
        // A chained command is covered when every program it runs is, and the
        // prompt offers only the ones that are not. A command that does
        // something no program name can stand for — substitutes a command
        // into itself, writes into a file, hands itself to a shell — is never
        // covered by a standing grant, and is never offered one. See
        // [`Programs`].
        let missing: Vec<String> = {
            let lists = self.allowlists.lock().await;
            keys.rules
                .iter()
                .filter(|rule| !lists.permits(rule))
                .cloned()
                .collect()
        };
        if keys.beyond.is_none() && missing.is_empty() {
            return Ok(());
        }

        let offer_global = keys.beyond.is_none() && global_offerable(tool.effect());
        let offer_workspace = keys.beyond.is_none() && self.workspace_rules;
        let request = PermissionRequest {
            id: Uuid::new_v4().to_string(),
            tool: tool.name().to_string(),
            effect: tool.effect(),
            preview: tool.preview(input),
            diff: tool.diff(input, &self.workspace).await,
            always_scope: describe_rules(&missing, tool.effect(), Scope::Workspace),
            offer_always: offer_workspace,
            always_global_scope: offer_global
                .then(|| describe_rules(&missing, tool.effect(), Scope::Global)),
            input: input.clone(),
            session: asking.session.clone(),
        };

        // Refused rather than asked, and only ever refused: everything a
        // standing grant permits was answered above without reaching here, so
        // running unattended takes nothing away from what was already allowed
        // and adds nothing to it. See [`Asking::unattended`].
        //
        // The preview goes into the message because this is the one refusal
        // the model is expected to work around rather than abandon, and it
        // cannot do that without knowing what was refused.
        if asking.unattended {
            return Err(ToolError::Rejected(format!(
                "nobody is there to allow this. The conversation is running unattended, so a \
                 call that needs a decision is refused rather than left waiting for one. This \
                 one was: {}. Carry on without it where you can, and say plainly what you could \
                 not do.",
                request.preview
            )));
        }

        let scope = match self.prompt.request(request).await {
            PermissionDecision::AllowOnce => {
                // Remembered only where "always" was on offer: what could not
                // have been made standing now cannot be made standing later.
                if let (true, Some(session)) = (offer_workspace, &asking.session) {
                    self.once
                        .lock()
                        .await
                        .entry(session.clone())
                        .or_default()
                        .extend(missing);
                }
                return Ok(());
            }
            PermissionDecision::Deny => return Err(ToolError::Denied),
            // A workspace grant with no workspace layer to keep it in is
            // honored for this call and not written down. Narrowing rather than
            // refusing: the user said yes to the call in front of them, and
            // only the standing part of it has nowhere to go.
            PermissionDecision::AllowAlways if !offer_workspace => return Ok(()),
            PermissionDecision::AllowAlways => Scope::Workspace,
            PermissionDecision::AllowAlwaysGlobal if offer_global => Scope::Global,
            // A frontend that offered a global grant where one was not on
            // offer gets the narrower decision honored, never the wider one.
            PermissionDecision::AllowAlwaysGlobal if offer_workspace => Scope::Workspace,
            PermissionDecision::AllowAlwaysGlobal => return Ok(()),
        };

        let mut lists = self.allowlists.lock().await;
        lists.layer(scope).allowed.extend(missing);
        self.save(scope, lists.layer(scope));
        Ok(())
    }

    /// What `session` has allowed once and could make standing: every rule it
    /// said yes to for one call that no layer permits yet, in rule order.
    pub async fn allowed_once(&self, session: &str) -> Vec<String> {
        let once = self.once.lock().await;
        let Some(rules) = once.get(session) else {
            return Vec::new();
        };
        let lists = self.allowlists.lock().await;
        rules
            .iter()
            .filter(|rule| !lists.permits(rule))
            .cloned()
            .collect()
    }

    /// Makes `rules` standing in this workspace, as "always" on each of their
    /// prompts would have.
    ///
    /// Workspace only, because this is the pill's half of "allow always", and
    /// the pill never offers "everywhere": running commands is never offered
    /// globally, and a decision made on the way out the door is not the moment
    /// for the widest grant there is. Nothing in a workspace whose own rules
    /// are not read, for the same reason `revoke` does nothing there.
    pub async fn grant(&self, rules: &[String]) {
        if !self.workspace_rules || rules.is_empty() {
            return;
        }
        let mut lists = self.allowlists.lock().await;
        lists.workspace.allowed.extend(rules.iter().cloned());
        self.save(Scope::Workspace, &lists.workspace);
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
        if scope == Scope::Workspace && !self.workspace_rules {
            return;
        }
        let mut lists = self.allowlists.lock().await;
        lists.layer(scope).allowed.remove(rule);
        self.save(scope, lists.layer(scope));
    }

    fn save(&self, scope: Scope, list: &Allowlist) {
        let path = match scope {
            Scope::Global => global_allowlist_file(&self.global),
            Scope::Workspace => workspace_allowlist_file(&self.workspace),
        };
        if list.unreadable {
            // See `Allowlist::unreadable`. Held in memory, and said, but not
            // written over a file somebody can still fix.
            tracing::warn!(
                path = %path.display(),
                "not saving a permission grant over a permissions file that does not parse; \
                 it holds for this session, and fixing the file keeps the rules it had"
            );
            return;
        }
        write_allowlist(&path, list);
    }
}

/// The units an "always" decision grants, for one call.
struct Keys {
    /// Every rule the call needs. All of them have to be permitted.
    rules: Vec<String>,
    /// Why no set of rules can cover this call, when none can. The call is
    /// then asked about every time and never offered a standing grant.
    beyond: Option<&'static str>,
}

/// What a call is keyed by.
///
/// A shell command is keyed by each program it runs, so approving `git` does
/// not also approve `rm`, and `cd x && cargo test | tail` needs `cd`, `cargo`
/// and `tail`. A call that names a URL is keyed by that URL's host, for the
/// same reason: "always allow fetching docs.rs" is a decision about a site the
/// user trusts, and the tool-wide version of it is a standing grant to reach
/// anywhere on the internet.
///
/// Everything else is keyed by name: the user is saying "this tool may write in
/// this workspace", which is the granularity they actually reason about.
fn keys_for(tool: &dyn Tool, input: &serde_json::Value) -> Keys {
    if tool.effect() == Effect::Execute {
        if let Some(command) = input.get("command").and_then(|c| c.as_str()) {
            let programs = Programs::of(command);
            return Keys {
                rules: programs
                    .names
                    .iter()
                    .map(|program| format!("{}:{program}", tool.name()))
                    .collect(),
                beyond: programs.beyond,
            };
        }
    }
    if tool.effect() == Effect::Network {
        if let Some(host) = input.get("url").and_then(|u| u.as_str()).and_then(host_of) {
            return Keys {
                rules: vec![format!("{}:{host}", tool.name())],
                beyond: None,
            };
        }
    }
    Keys {
        rules: vec![tool.name().to_string()],
        beyond: None,
    }
}

/// What granting `rules` would mean, in a sentence the prompt shows.
///
/// `rules` are the ones the call still needs, so a chained command whose `cd`
/// is already granted describes only the `cargo` it is asking for.
fn describe_rules(rules: &[String], effect: Effect, scope: Scope) -> String {
    let where_ = match scope {
        Scope::Global => "in every workspace",
        Scope::Workspace => "in this workspace",
    };
    let Some(first) = rules.first() else {
        return format!("Always allow this {where_}");
    };
    match first.split_once(':') {
        Some((tool, host)) if effect == Effect::Network => {
            format!("Always allow `{tool}` to reach {host} {where_}")
        }
        Some((tool, _)) => {
            let programs: Vec<String> = rules
                .iter()
                .map(|rule| format!("`{}`", rule.split_once(':').map_or(rule.as_str(), |r| r.1)))
                .collect();
            format!(
                "Always allow `{tool}` to run {} commands {where_}",
                list(&programs)
            )
        }
        None => format!("Always allow `{first}` {where_}"),
    }
}

/// `a`, `a and b`, `a, b and c`.
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
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

fn workspace_allowlist_file(workspace: &Path) -> PathBuf {
    workspace.join(ALLOWLIST_PATH)
}

fn global_allowlist_file(home: &Path) -> PathBuf {
    home.join(GLOBAL_ALLOWLIST_FILE)
}

/// One layer's rules, as the file at `path` has them.
///
/// No file is the empty list, which is every workspace's first state. A file
/// that is there and will not read is named in the log and marked, so it grants
/// nothing and is never written over. See [`Allowlist::unreadable`].
fn read_allowlist(path: &Path) -> Allowlist {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Allowlist::default(),
        Err(e) => return unreadable(path, &e.to_string()),
    };
    serde_json::from_str(&text).unwrap_or_else(|e| unreadable(path, &e.to_string()))
}

fn unreadable(path: &Path, error: &str) -> Allowlist {
    tracing::warn!(
        path = %path.display(),
        error,
        "a permissions file could not be read, so none of its grants apply until it is fixed"
    );
    Allowlist {
        unreadable: true,
        ..Allowlist::default()
    }
}

/// Puts `list` at `path` without ever leaving it partly written.
///
/// A plain overwrite truncates first, and a crash in between leaves a file that
/// no longer parses — which, read back, grants nothing until somebody fixes a
/// file they never touched.
fn write_allowlist(path: &Path, list: &Allowlist) {
    // A failure to persist must not fail the tool call the user just approved;
    // the grant holds for this session, and they are asked again next time.
    if let Err(e) = replace(path, list) {
        tracing::warn!(path = %path.display(), error = %e, "could not save a permission grant");
    }
}

/// Through a temporary file in the same directory, since a rename is atomic only
/// within one filesystem, flushed before it is renamed over the old one.
fn replace(path: &Path, list: &Allowlist) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let json = serde_json::to_string_pretty(list).map_err(std::io::Error::other)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(json.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
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
            Ok(String::new().into())
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

    /// Keeps the conversation the request named.
    struct Session {
        asked: Arc<Mutex<Option<Option<String>>>>,
        decision: PermissionDecision,
    }

    #[async_trait]
    impl PermissionPrompt for Session {
        async fn request(&self, request: PermissionRequest) -> PermissionDecision {
            *self.asked.lock().await = Some(request.session);
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
        assert!(engine
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// The whole of what running unattended does: a call that would have been
    /// asked about is refused instead of waiting for somebody who is not there.
    #[tokio::test]
    async fn unattended_refuses_what_it_would_have_asked_about() {
        let (engine, calls, _dir) = engine(PermissionDecision::AllowOnce);
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };

        let refused = engine
            .check(
                &tool,
                &serde_json::json!({}),
                &Asking {
                    session: None,
                    unattended: true,
                },
            )
            .await
            .expect_err("a call that needs a decision has nobody to make it");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "the prompt is not put to a window nobody is looking at"
        );
        // `Rejected` rather than `Denied`: nobody denied it, and the advice
        // that follows the two is different — see `ToolError::to_model_message`.
        let ToolError::Rejected(why) = refused else {
            panic!("expected a refusal on the merits, got {refused:?}");
        };
        assert!(why.contains("unattended"), "{why}");
        // The model reads this, and so does anyone reading the transcript.
        // It once carried runs of indentation from a broken line continuation.
        assert!(!why.contains("  "), "no runs of spaces: {why}");
    }

    /// It narrows and never widens. What a standing grant covers is answered
    /// before a prompt is ever reached, so leaving changes nothing about it.
    #[tokio::test]
    async fn unattended_still_runs_what_was_already_granted() {
        let (engine, _calls, _dir) = engine(PermissionDecision::AllowAlways);
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };

        // Granted while somebody was there to grant it.
        engine
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .unwrap();

        // And then left to run.
        engine
            .check(
                &tool,
                &serde_json::json!({}),
                &Asking {
                    session: None,
                    unattended: true,
                },
            )
            .await
            .expect("a standing grant is not a decision anybody has to be there for");
    }

    /// Two turns can run at once, so a window has to be able to say which one
    /// it is being asked about.
    #[tokio::test]
    async fn a_request_says_which_conversation_is_asking() {
        let homes = Homes::new();
        let asked: Arc<Mutex<Option<Option<String>>>> = Arc::new(Mutex::new(None));
        let engine = homes.engine(Box::new(Session {
            asked: asked.clone(),
            decision: PermissionDecision::Deny,
        }));

        let _ = engine
            .check(
                &Fake {
                    name: "write_file",
                    effect: Effect::Write,
                },
                &serde_json::json!({}),
                &Asking {
                    session: Some("conversation-7".into()),
                    unattended: false,
                },
            )
            .await;

        assert_eq!(
            asked.lock().await.clone(),
            Some(Some("conversation-7".into()))
        );
    }

    #[tokio::test]
    async fn writes_prompt_and_can_be_denied() {
        let (engine, calls, _dir) = engine(PermissionDecision::Deny);
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };
        assert!(matches!(
            engine
                .check(&tool, &serde_json::json!({}), &Asking::default())
                .await,
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
            engine
                .check(&tool, &serde_json::json!({}), &Asking::default())
                .await
                .unwrap();
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
            engine
                .check(&tool, &serde_json::json!({}), &Asking::default())
                .await
                .unwrap();
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
        first
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .unwrap();

        // A fresh engine over the same workspace must honor the stored rule
        // even though its prompt denies everything.
        let second = homes.engine(Box::new(DenyAll));
        assert!(second
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn a_workspace_grant_stays_in_the_workspace() {
        let (engine, _, homes) = engine(PermissionDecision::AllowAlways);
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };
        engine
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .unwrap();

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
        engine
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .unwrap();
        assert!(
            !workspace_allowlist_file(first.path()).is_file(),
            "a global decision must not also be written to the workspace"
        );

        // A different workspace, sharing only the config home, and a prompt
        // that would refuse. The stored global rule has to carry it.
        let second = tempfile::tempdir().unwrap();
        let elsewhere = PermissionEngine::new(second.path(), global.path(), Box::new(DenyAll));
        assert!(elsewhere
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .is_ok());
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
            .check(
                &tool,
                &serde_json::json!({"command": "git status"}),
                &Asking::default(),
            )
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
        engine
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .unwrap();
        engine.revoke("write_file", Scope::Workspace).await;

        let global = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowAlwaysGlobal,
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        global
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .unwrap();

        global.revoke("write_file", Scope::Workspace).await;
        assert!(
            global
                .check(&tool, &serde_json::json!({}), &Asking::default())
                .await
                .is_ok(),
            "revoking the workspace copy dropped the global grant"
        );

        global.revoke("write_file", Scope::Global).await;
        let fresh = homes.engine(Box::new(DenyAll));
        assert!(matches!(
            fresh
                .check(&tool, &serde_json::json!({}), &Asking::default())
                .await,
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
        engine
            .check(&write, &serde_json::json!({}), &Asking::default())
            .await
            .unwrap();

        let global = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowAlwaysGlobal,
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        global
            .check(&net, &serde_json::json!({}), &Asking::default())
            .await
            .unwrap();

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

    #[tokio::test]
    async fn a_permissions_file_that_does_not_parse_is_not_written_over() {
        // A stray comma must not read as "no grants" and then let the next
        // "always" put a list of one over every rule the file held.
        let homes = Homes::new();
        let file = workspace_allowlist_file(homes.workspace.path());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let broken = r#"{"allowed": ["write_file", "run_command:git",]}"#;
        std::fs::write(&file, broken).unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let engine = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowAlways,
            calls: calls.clone(),
        }));
        let tool = Fake {
            name: "fetch",
            effect: Effect::Network,
        };
        engine
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            broken,
            "the broken file was written over"
        );

        // The grant still holds for the rest of the session.
        engine
            .check(&tool, &serde_json::json!({}), &Asking::default())
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_standing_grant_does_not_cover_a_command_chained_onto_it() {
        // `git status; rm -rf ~` runs `rm` as well, and only `git` is granted.
        let homes = Homes::new();
        let file = workspace_allowlist_file(homes.workspace.path());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"allowed": ["run_command:git"]}"#).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let engine = homes.engine(Box::new(Counting {
            decision: PermissionDecision::AllowOnce,
            calls: calls.clone(),
        }));
        let run = Fake {
            name: "run_command",
            effect: Effect::Execute,
        };

        engine
            .check(
                &run,
                &serde_json::json!({"command": "git status"}),
                &Asking::default(),
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0, "the grant covers git");

        engine
            .check(
                &run,
                &serde_json::json!({"command": "git status; rm -rf ~"}),
                &Asking::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "what rides after git was let through unasked"
        );
    }

    #[tokio::test]
    async fn a_chained_command_is_granted_by_its_parts() {
        // One "always" on `cd x && cargo test | tail` grants the three programs
        // it runs, and nothing more: `rm` still asks.
        let (engine, calls, homes) = engine(PermissionDecision::AllowAlways);
        let run = Fake {
            name: "run_command",
            effect: Effect::Execute,
        };
        engine
            .check(
                &run,
                &serde_json::json!({"command": "cd crates/x && cargo test 2>&1 | tail -20"}),
                &Asking::default(),
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let rules: Vec<String> = engine
            .allowed_rules()
            .await
            .into_iter()
            .map(|r| r.rule)
            .collect();
        assert_eq!(
            rules,
            ["run_command:cargo", "run_command:cd", "run_command:tail"]
        );
        assert!(homes.wrote_workspace());

        for covered in [
            "cargo build",
            "cd crates/y && cargo test",
            "cargo test | tail -5",
        ] {
            engine
                .check(
                    &run,
                    &serde_json::json!({"command": covered}),
                    &Asking::default(),
                )
                .await
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "every part was granted");

        engine
            .check(
                &run,
                &serde_json::json!({"command": "cargo clean && rm -rf target"}),
                &Asking::default(),
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "rm was never granted");
    }

    #[tokio::test]
    async fn the_offer_names_only_the_programs_still_missing() {
        let homes = Homes::new();
        let file = workspace_allowlist_file(homes.workspace.path());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"allowed": ["run_command:cd"]}"#).unwrap();
        let seen = Arc::new(Mutex::new(None));
        let engine = homes.engine(Box::new(Offered {
            seen: seen.clone(),
            decision: PermissionDecision::AllowOnce,
        }));
        let run = Fake {
            name: "run_command",
            effect: Effect::Execute,
        };
        engine
            .check(
                &run,
                &serde_json::json!({"command": "cd x && cargo test | tail"}),
                &Asking::default(),
            )
            .await
            .unwrap();
        let (offered, scope) = seen.lock().await.clone().unwrap();
        assert!(offered);
        assert_eq!(
            scope,
            "Always allow `run_command` to run `cargo` and `tail` commands in this workspace"
        );
    }

    #[tokio::test]
    async fn a_command_beyond_any_grant_is_asked_every_time() {
        // Every program is granted, and the line still does something no
        // program name stands for. Asked, never offered "always", and an
        // answer of "always" regardless is narrowed to this one call.
        let homes = Homes::new();
        let file = workspace_allowlist_file(homes.workspace.path());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            r#"{"allowed": ["run_command:git", "run_command:sh"]}"#,
        )
        .unwrap();
        let seen = Arc::new(Mutex::new(None));
        let engine = homes.engine(Box::new(Offered {
            seen: seen.clone(),
            decision: PermissionDecision::AllowAlways,
        }));
        let run = Fake {
            name: "run_command",
            effect: Effect::Execute,
        };
        let before = std::fs::read_to_string(&file).unwrap();
        for line in [
            "git show > ~/.bashrc",
            "git log $(rm -rf ~)",
            "sh -c 'git status'",
            "git apply <<EOF\npatch\nEOF",
        ] {
            *seen.lock().await = None;
            engine
                .check(
                    &run,
                    &serde_json::json!({"command": line}),
                    &Asking::default(),
                )
                .await
                .unwrap();
            let (offered, _) = seen.lock().await.clone().unwrap_or_else(|| {
                panic!("{line:?} was let through on the strength of its programs")
            });
            assert!(!offered, "{line:?} was offered a standing grant");
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
    }

    #[tokio::test]
    async fn what_was_allowed_once_is_remembered_for_the_conversation() {
        let (engine, calls, homes) = engine(PermissionDecision::AllowOnce);
        let run = Fake {
            name: "run_command",
            effect: Effect::Execute,
        };
        let here = Asking {
            session: Some("here".into()),
            unattended: false,
        };
        engine
            .check(
                &run,
                &serde_json::json!({"command": "cd x && cargo test"}),
                &here,
            )
            .await
            .unwrap();
        // Beyond any grant, so never offered "always", so not remembered.
        engine
            .check(
                &run,
                &serde_json::json!({"command": "git show > out.txt"}),
                &here,
            )
            .await
            .unwrap();
        assert_eq!(
            engine.allowed_once("here").await,
            ["run_command:cargo", "run_command:cd"]
        );
        assert!(engine.allowed_once("elsewhere").await.is_empty());
        assert!(!homes.wrote_workspace(), "allowing once wrote nothing");

        engine.grant(&engine.allowed_once("here").await).await;
        assert!(engine.allowed_once("here").await.is_empty());
        assert!(homes.wrote_workspace());
        let asked = calls.load(Ordering::SeqCst);
        engine
            .check(&run, &serde_json::json!({"command": "cargo build"}), &here)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), asked, "the grant is standing");
    }

    #[tokio::test]
    async fn a_grant_goes_nowhere_in_a_workspace_whose_rules_are_not_read() {
        let homes = Homes::new();
        let engine = homes.engine(Box::new(DenyAll)).with_workspace_rules(false);
        engine.grant(&["run_command:cargo".to_string()]).await;
        assert!(!homes.wrote_workspace());
        assert!(engine.allowed_rules().await.is_empty());
    }

    /// Keeps whether a standing grant was offered and how it was described.
    struct Offered {
        seen: Arc<Mutex<Option<(bool, String)>>>,
        decision: PermissionDecision,
    }

    #[async_trait]
    impl PermissionPrompt for Offered {
        async fn request(&self, request: PermissionRequest) -> PermissionDecision {
            *self.seen.lock().await = Some((request.offer_always, request.always_scope));
            self.decision
        }
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
            .check(
                &fetch,
                &serde_json::json!({"url": "https://docs.rs/serde"}),
                &Asking::default(),
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A second page on the granted host does not ask again.
        engine
            .check(
                &fetch,
                &serde_json::json!({"url": "https://docs.rs/tokio"}),
                &Asking::default(),
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A different host is a different decision.
        engine
            .check(
                &fetch,
                &serde_json::json!({"url": "https://evil.test/x"}),
                &Asking::default(),
            )
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
        let described = describe_rules(
            &["fetch_url:docs.rs".to_string()],
            Effect::Network,
            Scope::Workspace,
        );
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
            .check(
                &search,
                &serde_json::json!({"query": "rust"}),
                &Asking::default(),
            )
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
                &Asking::default(),
            )
            .await
            .is_ok());
    }

    #[test]
    fn the_two_scopes_are_described_differently() {
        assert!(
            describe_rules(&["write_file".to_string()], Effect::Write, Scope::Workspace)
                .contains("this workspace")
        );
        assert!(
            describe_rules(&["write_file".to_string()], Effect::Write, Scope::Global)
                .contains("every workspace")
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
            .check(
                &tool,
                &serde_json::json!({"command": "git status"}),
                &Asking::default(),
            )
            .await
            .unwrap();
        engine
            .check(
                &tool,
                &serde_json::json!({"command": "git log"}),
                &Asking::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "second git call reused the rule"
        );

        engine
            .check(
                &tool,
                &serde_json::json!({"command": "rm -rf /"}),
                &Asking::default(),
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "rm must prompt separately");
    }
}
