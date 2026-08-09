//! The gate between a model's intent and the user's machine.
//!
//! Read-only work inside the workspace runs unattended, because prompting for
//! every file read makes the harness unusable. Anything that writes, executes,
//! or leaves the machine needs a decision, and "always" decisions persist so
//! the user grants each capability once rather than once per call.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use ts_rs::TS;
use uuid::Uuid;

use crate::tool::{Effect, Tool, ToolError};

/// Where persisted decisions live, relative to the workspace root.
const ALLOWLIST_PATH: &str = ".taurus/permissions.json";

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
    #[ts(type = "unknown")]
    pub input: serde_json::Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PermissionDecision {
    AllowOnce,
    AllowAlways,
    Deny,
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

pub struct PermissionEngine {
    workspace: PathBuf,
    prompt: Box<dyn PermissionPrompt>,
    allowlist: Mutex<Allowlist>,
}

impl PermissionEngine {
    pub fn new(workspace: impl Into<PathBuf>, prompt: Box<dyn PermissionPrompt>) -> Self {
        let workspace = workspace.into();
        let allowlist = load_allowlist(&workspace);
        Self {
            workspace,
            prompt,
            allowlist: Mutex::new(allowlist),
        }
    }

    /// Decides whether `tool` may run with `input`, prompting if needed.
    pub async fn check(&self, tool: &dyn Tool, input: &serde_json::Value) -> Result<(), ToolError> {
        if tool.effect() == Effect::Read {
            return Ok(());
        }

        let rule = rule_for(tool, input);
        if self.allowlist.lock().await.allowed.contains(&rule) {
            return Ok(());
        }

        let request = PermissionRequest {
            id: Uuid::new_v4().to_string(),
            tool: tool.name().to_string(),
            effect: tool.effect(),
            preview: tool.preview(input),
            always_scope: describe_rule(&rule),
            input: input.clone(),
        };

        match self.prompt.request(request).await {
            PermissionDecision::AllowOnce => Ok(()),
            PermissionDecision::AllowAlways => {
                let mut list = self.allowlist.lock().await;
                list.allowed.insert(rule);
                save_allowlist(&self.workspace, &list);
                Ok(())
            }
            PermissionDecision::Deny => Err(ToolError::Denied),
        }
    }

    pub async fn allowed_rules(&self) -> Vec<String> {
        self.allowlist
            .lock()
            .await
            .allowed
            .iter()
            .cloned()
            .collect()
    }

    pub async fn revoke(&self, rule: &str) {
        let mut list = self.allowlist.lock().await;
        list.allowed.remove(rule);
        save_allowlist(&self.workspace, &list);
    }
}

/// The unit an "always" decision grants.
///
/// Shell commands are keyed by their leading word so approving `git` does not
/// also approve `rm`. Every other tool is keyed by name: the user is saying
/// "this tool may write in this workspace", which is the granularity they
/// actually reason about.
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
    tool.name().to_string()
}

fn describe_rule(rule: &str) -> String {
    match rule.split_once(':') {
        Some((tool, program)) => {
            format!("Always allow `{tool}` to run `{program}` commands in this workspace")
        }
        None => format!("Always allow `{rule}` in this workspace"),
    }
}

/// First bare word of a command line, ignoring `VAR=value` prefixes.
fn leading_word(command: &str) -> Option<&str> {
    command
        .split_whitespace()
        .find(|w| !w.contains('='))
        .map(|w| w.trim_start_matches(['(', '{']))
        .filter(|w| !w.is_empty())
}

fn allowlist_file(workspace: &Path) -> PathBuf {
    workspace.join(ALLOWLIST_PATH)
}

fn load_allowlist(workspace: &Path) -> Allowlist {
    std::fs::read_to_string(allowlist_file(workspace))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_allowlist(workspace: &Path, list: &Allowlist) {
    let path = allowlist_file(workspace);
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

    fn engine(
        decision: PermissionDecision,
    ) -> (PermissionEngine, Arc<AtomicUsize>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let engine = PermissionEngine::new(
            dir.path(),
            Box::new(Counting {
                decision,
                calls: calls.clone(),
            }),
        );
        (engine, calls, dir)
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
        let dir = tempfile::tempdir().unwrap();
        let tool = Fake {
            name: "write_file",
            effect: Effect::Write,
        };
        let first = PermissionEngine::new(
            dir.path(),
            Box::new(Counting {
                decision: PermissionDecision::AllowAlways,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
        );
        first.check(&tool, &serde_json::json!({})).await.unwrap();

        // A fresh engine over the same workspace must honor the stored rule
        // even though its prompt denies everything.
        let second = PermissionEngine::new(dir.path(), Box::new(DenyAll));
        assert!(second.check(&tool, &serde_json::json!({})).await.is_ok());
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
