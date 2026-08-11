//! The tool registry: name resolution, schema advertisement, and dispatch.
//!
//! Built-ins, skill tools, and MCP tools all live here, so the agent loop has
//! one lookup path and the model sees one flat namespace.

use std::collections::BTreeMap;
use std::sync::Arc;

use taurus_provider::ToolDef;
use tracing::{debug, warn};

use crate::tool::{Tool, ToolContext, ToolError, ToolResult};

#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every tool the harness ships with.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(crate::builtin::fs::ReadFile));
        registry.register(Arc::new(crate::builtin::fs::WriteFile));
        registry.register(Arc::new(crate::builtin::fs::EditFile));
        registry.register(Arc::new(crate::builtin::fs::ListDir));
        registry.register(Arc::new(crate::builtin::search::Glob));
        registry.register(Arc::new(crate::builtin::search::Grep));
        registry.register(Arc::new(crate::builtin::shell::RunCommand));
        registry
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        if self.tools.insert(name.clone(), tool).is_some() {
            // Later registration wins, but a silent shadow would be a very
            // confusing bug to chase later.
            warn!(tool = %name, "tool was re-registered and now shadows the previous one");
        }
    }

    /// A copy of this registry with one tool removed.
    ///
    /// Used to build a sub-agent's registry without the spawn tool, which caps
    /// delegation depth structurally rather than by a counter.
    pub fn without(&self, name: &str) -> Self {
        let mut clone = self.clone();
        clone.tools.remove(name);
        clone
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Tool definitions to advertise to the model.
    pub fn definitions(&self) -> Vec<ToolDef> {
        self.tools
            .values()
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }

    /// Definitions restricted to `allowed`, for sub-agents and skills that
    /// declare a narrower tool set.
    pub fn definitions_for(&self, allowed: &[String]) -> Vec<ToolDef> {
        self.definitions()
            .into_iter()
            .filter(|d| allowed.iter().any(|a| a == &d.name))
            .collect()
    }

    /// Resolves, permission-checks, and runs a tool call.
    ///
    /// Every failure here is expected to be reported back to the model as a
    /// tool result rather than aborting the turn: a wrong tool name or bad
    /// arguments is something the model can fix on the next iteration.
    pub async fn execute(
        &self,
        name: &str,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> ToolResult {
        let Some(tool) = self.get(name) else {
            return Err(ToolError::NotFound(name.to_string()));
        };

        if ctx.cancel.is_cancelled() {
            return Err(ToolError::Canceled);
        }

        // Small models stringify scalars (`"replace_all": "false"`). Fixing that
        // here rather than in each tool means every tool benefits, including
        // ones registered by skills and MCP servers.
        let input = crate::coerce::coerce(input, &tool.input_schema());

        ctx.permissions.check(tool.as_ref(), &input).await?;

        // After the permission check, so a denied call leaves no trace, and
        // before execution, so what is recorded is what was there first.
        if let Some(recorder) = &ctx.checkpoints {
            for candidate in tool.touches(&input) {
                // An unresolvable path is left to the tool to reject with its
                // own message; there is nothing to snapshot either way.
                if let Ok(path) = ctx.resolve(&candidate) {
                    recorder.capture(&path).await;
                }
            }
        }

        debug!(tool = name, "executing");
        tool.execute(input, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_ctx;

    #[test]
    fn builtins_are_registered_under_stable_names() {
        let registry = ToolRegistry::with_builtins();
        for expected in [
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "glob",
            "grep",
            "run_command",
        ] {
            assert!(registry.get(expected).is_some(), "missing {expected}");
        }
    }

    #[test]
    fn every_definition_carries_a_description_and_object_schema() {
        for def in ToolRegistry::with_builtins().definitions() {
            assert!(
                !def.description.trim().is_empty(),
                "{} has no description",
                def.name
            );
            assert_eq!(
                def.input_schema.get("type").and_then(|t| t.as_str()),
                Some("object"),
                "{} schema is not an object",
                def.name
            );
        }
    }

    #[test]
    fn definitions_for_filters_to_the_allowed_set() {
        let registry = ToolRegistry::with_builtins();
        let defs = registry.definitions_for(&["read_file".into(), "glob".into()]);
        let names: Vec<_> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["glob", "read_file"]);
    }

    #[tokio::test]
    async fn an_unknown_tool_is_a_recoverable_error() {
        let (ctx, _dir) = test_ctx();
        let err = ToolRegistry::with_builtins()
            .execute("nope", serde_json::json!({}), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_model_message().contains("No tool named 'nope'"));
    }

    #[tokio::test]
    async fn a_denied_write_never_touches_the_disk() {
        let (ctx, dir) = crate::test_support::test_ctx_denying();
        let registry = ToolRegistry::with_builtins();
        let err = registry
            .execute(
                "write_file",
                serde_json::json!({"path": "x.txt", "content": "nope"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Denied));
        assert!(!dir.path().join("x.txt").exists());
    }

    #[tokio::test]
    async fn stringified_scalars_are_coerced_before_the_tool_sees_them() {
        // Without coercion this is the "invalid type: string, expected a
        // boolean" bounce that small models get stuck in.
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "x x").unwrap();
        let out = ToolRegistry::with_builtins()
            .execute(
                "edit_file",
                serde_json::json!({
                    "path": "a.txt",
                    "old_string": "x",
                    "new_string": "y",
                    "replace_all": "true"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("2 replacements"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "y y"
        );
    }

    #[tokio::test]
    async fn a_write_dispatched_through_the_registry_can_be_undone() {
        let (ctx, dir) = test_ctx();
        let root = ctx.workspace.clone();
        std::fs::write(root.join("a.txt"), "original").unwrap();

        let logs = tempfile::TempDir::new().unwrap();
        let store = crate::CheckpointStore::new(logs.path());
        let ctx = ctx.with_checkpoints(store.begin_turn("s1", &root, "replace a.txt"));

        ToolRegistry::with_builtins()
            .execute(
                "write_file",
                serde_json::json!({"path": "a.txt", "content": "the model's version"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "the model's version"
        );

        store.rewind("s1", &root, 1, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "original"
        );
        drop(dir);
    }

    #[tokio::test]
    async fn a_denied_write_leaves_nothing_to_rewind() {
        // The snapshot sits behind the permission gate, so a refused call does
        // not litter the log with turns that changed nothing.
        let (ctx, _dir) = crate::test_support::test_ctx_denying();
        let root = ctx.workspace.clone();
        let logs = tempfile::TempDir::new().unwrap();
        let store = crate::CheckpointStore::new(logs.path());
        let ctx = ctx.with_checkpoints(store.begin_turn("s1", &root, "try to write"));

        let _ = ToolRegistry::with_builtins()
            .execute(
                "write_file",
                serde_json::json!({"path": "x.txt", "content": "nope"}),
                &ctx,
            )
            .await;

        assert!(store.turns("s1").unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_command_declares_nothing_it_will_touch() {
        // Asserted rather than only documented: a shell command's reach is not
        // knowable in advance, and the day someone makes this tool guess, the
        // guess needs to fail loudly here.
        let registry = ToolRegistry::with_builtins();
        assert!(registry
            .get("run_command")
            .unwrap()
            .touches(&serde_json::json!({"command": "rm -rf src"}))
            .is_empty());
        assert_eq!(
            registry
                .get("edit_file")
                .unwrap()
                .touches(&serde_json::json!({"path": "src/main.rs"})),
            vec!["src/main.rs".to_string()]
        );
    }

    #[tokio::test]
    async fn reads_are_allowed_without_a_prompt_even_when_prompts_deny() {
        let (ctx, dir) = crate::test_support::test_ctx_denying();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        let out = ToolRegistry::with_builtins()
            .execute("read_file", serde_json::json!({"path": "a.txt"}), &ctx)
            .await
            .unwrap();
        assert!(out.contains("hi"));
    }
}
