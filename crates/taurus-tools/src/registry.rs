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

    /// Removes a tool, reporting whether there was one to remove.
    ///
    /// Removal rather than hiding: a tool left registered but undeclared is
    /// still reachable by a skill or a sub-agent, which makes "turned off" mean
    /// something different depending on who is asking.
    pub fn remove(&mut self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }

    /// A copy of this registry with one tool removed.
    ///
    /// Used to build a sub-agent's registry without the spawn tool, which caps
    /// delegation depth structurally rather than by a counter.
    pub fn without(&self, name: &str) -> Self {
        let mut clone = self.clone();
        clone.remove(name);
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
    ///
    /// Schemas are slimmed on the way out. This is the one place every tool's
    /// definition passes through — built-in, skill, and MCP alike — so it is
    /// also the one place that has to know the difference between what a
    /// validator needs and what a model reads. Dispatch still sees the full
    /// schema: [`crate::coerce`] works from the real types.
    pub fn definitions(&self) -> Vec<ToolDef> {
        self.tools
            .values()
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: crate::schema::slim(&t.input_schema()),
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

        // A tool that changes files without being able to name them first is
        // covered by looking rather than by asking. That used to be the whole of
        // `run_command` sitting outside undo, and outside the changed-file list
        // the user reads to decide whether they need it.
        let sweep = match &ctx.checkpoints {
            Some(_) if tool.touches_unpredictably() => {
                Some(crate::sweep::Sweep::before(&ctx.workspace).await)
            }
            _ => None,
        };

        debug!(tool = name, "executing");
        let mut result = tool.execute(input, ctx).await;

        // Unconditionally: a command that failed, timed out, or was canceled
        // has still written whatever it got as far as writing, and that is
        // precisely the turn someone reaches for undo on.
        if let (Some(sweep), Some(recorder)) = (sweep, &ctx.checkpoints) {
            let change = sweep.after(&ctx.workspace, recorder).await;

            // What it *did* record needs no announcement: the changed-file
            // count in the header and the Changes drawer are both read straight
            // off the log, and they are where someone goes to look.
            //
            // What it could not record does. Believing a turn is undoable when
            // it is not is the failure this whole path exists to prevent, and a
            // progress line would not do — those are dropped from the card the
            // moment the call finishes, which for a command is immediately.
            if let Some(warning) = change.warning() {
                annotate(&mut result, &warning);
            }
        }

        result
    }
}

/// Adds a note about the call to whatever the call produced.
///
/// A timeout and a cancellation are `Err`, and they are exactly the outcomes a
/// warning about unrecorded changes matters most for — a command killed
/// part-way through wrote something, and nobody knows what. So the error text
/// carries it too. `Canceled` and the structured input errors are left alone:
/// they have no message of their own to extend, and a call that never ran
/// changed nothing.
fn annotate(result: &mut ToolResult, note: &str) {
    match result {
        Ok(output) => output.push_str(&format!("\n\n[taurus] {note}")),
        Err(ToolError::Failed(message)) => {
            message.push_str(&format!("\n\n[taurus] {note}"));
        }
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_ctx;
    use crate::tool::Effect;

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

    /// Writes a file, then exits non-zero.
    #[cfg(windows)]
    const WRITES_THEN_FAILS: &str = "echo half-done > out.txt & exit 1";
    #[cfg(not(windows))]
    const WRITES_THEN_FAILS: &str = "echo half-done > out.txt; exit 1";

    #[tokio::test]
    async fn a_command_that_changed_files_can_be_undone() {
        // The whole point of the sweep, through the path the agent actually
        // takes: no tool declared this file, and it is still recoverable.
        let (ctx, _dir) = test_ctx();
        let root = ctx.workspace.clone();
        std::fs::write(root.join("a.txt"), "original").unwrap();

        let logs = tempfile::TempDir::new().unwrap();
        let store = crate::CheckpointStore::new(logs.path());
        let ctx = ctx.with_checkpoints(store.begin_turn("s1", &root, "rewrite it"));

        ToolRegistry::with_builtins()
            .execute(
                "run_command",
                serde_json::json!({"command": "echo rewritten > a.txt"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_ne!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "original"
        );

        // And it reaches the list the user reads before deciding to undo.
        assert_eq!(store.turns("s1").unwrap()[0].files, vec!["a.txt"]);

        store.rewind("s1", &root, 1, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "original"
        );
    }

    #[tokio::test]
    async fn a_command_that_failed_still_has_its_changes_recorded() {
        // A command killed partway through has written whatever it got as far
        // as writing, and that is exactly when undo is wanted.
        let (ctx, _dir) = test_ctx();
        let root = ctx.workspace.clone();
        let logs = tempfile::TempDir::new().unwrap();
        let store = crate::CheckpointStore::new(logs.path());
        let ctx = ctx.with_checkpoints(store.begin_turn("s1", &root, "half a job"));

        let out = ToolRegistry::with_builtins()
            .execute(
                "run_command",
                serde_json::json!({"command": WRITES_THEN_FAILS}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("Exit code 1"), "{out}");

        assert_eq!(store.turns("s1").unwrap()[0].files, vec!["out.txt"]);
        store.rewind("s1", &root, 1, false).unwrap();
        assert!(!root.join("out.txt").exists());
    }

    #[tokio::test]
    async fn a_command_the_sweep_could_not_cover_says_so_on_the_result() {
        // Believing a turn is undoable when it is not is the failure this whole
        // path exists to prevent, so the one case that cannot be recorded has
        // to be said out loud — and on the result, which survives, rather than
        // as progress, which the card drops the moment the call ends.
        let (ctx, _dir) = test_ctx();
        let root = ctx.workspace.clone();
        std::fs::write(root.join(".gitignore"), "dist/\n").unwrap();
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::write(root.join("dist/old.txt"), "predates the turn").unwrap();

        let logs = tempfile::TempDir::new().unwrap();
        let store = crate::CheckpointStore::new(logs.path());
        let ctx = ctx.with_checkpoints(store.begin_turn("s1", &root, "unignore dist"));

        let out = ToolRegistry::with_builtins()
            .execute(
                "run_command",
                // Rewrites the rule so `dist/` stops being ignored, which makes
                // every file already in it look newly created.
                serde_json::json!({"command": "echo somethingelse > .gitignore"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(out.contains("[taurus]"), "no warning on the result: {out}");
        assert!(out.contains("ignore rule"), "{out}");

        // And the thing the warning is about actually held: the rewind leaves
        // the revealed file alone rather than deleting it.
        store.rewind("s1", &root, 1, false).unwrap();
        assert!(root.join("dist/old.txt").exists());
    }

    /// Stands in for an MCP tool: highest permission tier, because an external
    /// program is doing arbitrary work, but it never touches these files.
    struct RemoteTool;

    #[async_trait::async_trait]
    impl Tool for RemoteTool {
        fn name(&self) -> &str {
            "remote_thing"
        }
        fn description(&self) -> &str {
            "Calls a remote service."
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn effect(&self) -> Effect {
            Effect::Execute
        }
        async fn execute(&self, _: serde_json::Value, _: &ToolContext) -> ToolResult {
            Ok("{\"temperature\": 12}".into())
        }
    }

    #[tokio::test]
    async fn a_tool_that_only_reaches_outward_is_not_swept() {
        // `Effect::Execute` is a permission tier, not a claim about the
        // filesystem. Reading it as one made every call to a remote API index
        // the whole workspace twice and glued a note onto a JSON payload the
        // model then had to parse.
        let (ctx, _dir) = test_ctx();
        let root = ctx.workspace.clone();
        let logs = tempfile::TempDir::new().unwrap();
        let store = crate::CheckpointStore::new(logs.path());
        let ctx = ctx.with_checkpoints(store.begin_turn("s1", &root, "ask the weather"));

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(RemoteTool));
        let out = registry
            .execute("remote_thing", serde_json::json!({}), &ctx)
            .await
            .unwrap();

        assert_eq!(out, "{\"temperature\": 12}", "the payload was altered");
        assert!(store.turns("s1").unwrap().is_empty());
    }

    #[test]
    fn a_failure_carries_the_warning_too() {
        // A timeout is an `Err`, and it is the outcome the warning matters most
        // for: the command wrote something before it was killed, and nobody
        // knows what. Annotating only `Ok` left exactly that case silent.
        let mut result: ToolResult = Err(ToolError::Failed("Command timed out after 1s".into()));
        annotate(&mut result, "This one cannot be undone.");
        let message = result.unwrap_err().to_string();
        assert!(message.contains("timed out"), "{message}");
        assert!(
            message.contains("[taurus] This one cannot be undone."),
            "{message}"
        );
    }

    #[test]
    fn a_call_that_never_ran_is_left_alone() {
        // Nothing to extend, and a canceled call changed nothing to warn about.
        let mut result: ToolResult = Err(ToolError::Canceled);
        annotate(&mut result, "ignored");
        assert!(!result.unwrap_err().to_string().contains("ignored"));
    }

    #[tokio::test]
    async fn a_read_only_command_leaves_the_log_empty() {
        let (ctx, _dir) = test_ctx();
        let root = ctx.workspace.clone();
        std::fs::write(root.join("a.txt"), "untouched").unwrap();
        let logs = tempfile::TempDir::new().unwrap();
        let store = crate::CheckpointStore::new(logs.path());
        let ctx = ctx.with_checkpoints(store.begin_turn("s1", &root, "just look"));

        ToolRegistry::with_builtins()
            .execute("run_command", serde_json::json!({"command": "ls"}), &ctx)
            .await
            .unwrap();

        assert!(store.turns("s1").unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_denied_command_is_never_swept() {
        // The sweep sits behind the permission gate like every other capture,
        // so a refused command costs neither a walk nor a turn in the log.
        let (ctx, _dir) = crate::test_support::test_ctx_denying();
        let root = ctx.workspace.clone();
        let logs = tempfile::TempDir::new().unwrap();
        let store = crate::CheckpointStore::new(logs.path());
        let ctx = ctx.with_checkpoints(store.begin_turn("s1", &root, "try to run"));

        let _ = ToolRegistry::with_builtins()
            .execute(
                "run_command",
                serde_json::json!({"command": "echo nope > x.txt"}),
                &ctx,
            )
            .await;

        assert!(store.turns("s1").unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_command_declares_nothing_it_will_touch() {
        // Asserted rather than only documented: a shell command's reach is not
        // knowable in advance, and the day someone makes this tool guess, the
        // guess needs to fail loudly here. What covers it is `sweep`, which
        // looks instead of predicting.
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
