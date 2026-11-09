//! `compare_delegates`: reading what several delegates kept, side by side.
//!
//! Best-of-N runs one task as several delegates, each in its own copy of the
//! workspace and often each on a different model, and then somebody has to
//! choose. Their reports say what each *thinks* it did; this says what each
//! changed, as diffs, so the comparison is of the work and not of the prose.
//!
//! Read-only. Applying one is still `apply_delegate`, or the person's Apply.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, Isolation, Tool, ToolContext, ToolError, ToolResult};

pub const COMPARE_TOOL: &str = "compare_delegates";

/// Most runs one call compares. Past this a comparison is a reading list.
const MAX_RUNS: usize = 6;

/// Characters of diff shown across every run in one call, shared equally. The
/// `--stat` summary of each is always shown whole.
const TOTAL_DIFF: usize = 48_000;

#[derive(Deserialize, JsonSchema)]
pub struct CompareInput {
    /// The runs to compare, as their reports name them: "Kept as run <run>".
    pub runs: Vec<String>,
}

pub struct CompareDelegates {
    isolation: Arc<dyn Isolation>,
}

impl CompareDelegates {
    pub fn new(isolation: Arc<dyn Isolation>) -> Self {
        Self { isolation }
    }
}

#[async_trait]
impl Tool for CompareDelegates {
    fn name(&self) -> &str {
        COMPARE_TOOL
    }

    fn description(&self) -> &str {
        "Show what several delegates kept in their own copies of the workspace, as a diff per \
         run, so you can compare the work itself rather than their reports. Read-only: applying \
         one is apply_delegate."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<CompareInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let n = input
            .get("runs")
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len);
        format!("Compare what {n} delegate runs changed")
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: CompareInput = parse_input(input)?;
        let mut runs: Vec<String> = Vec::new();
        for run in input.runs.iter().map(|r| r.trim()) {
            if !run.is_empty() && !runs.iter().any(|r| r == run) {
                runs.push(run.to_string());
            }
        }
        if runs.is_empty() {
            return Err(ToolError::InvalidInput("name at least one run".into()));
        }
        if runs.len() > MAX_RUNS {
            return Err(ToolError::Rejected(format!(
                "{} runs is more than one comparison reads well; compare at most {MAX_RUNS} at \
                 a time",
                runs.len()
            )));
        }

        let share = TOTAL_DIFF / runs.len();
        let mut out = String::new();
        for run in &runs {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format!("## Run {run}\n\n"));
            match self.isolation.diff(run, ctx.session_id.as_deref()).await {
                Ok(diff) if diff.trim().is_empty() => out.push_str("(changed nothing)"),
                Ok(diff) => out.push_str(&cut(&diff, share)),
                Err(why) => out.push_str(&format!("(can't be shown: {why})")),
            }
        }
        Ok(out.into())
    }
}

/// `diff` cut to about `max` characters on a line boundary, saying how much
/// went. The `--stat` summary comes first in what [`Isolation::diff`] returns,
/// so what survives a cut is the summary and the start of the patch.
fn cut(diff: &str, max: usize) -> String {
    if diff.len() <= max {
        return diff.to_string();
    }
    let mut end = max;
    while !diff.is_char_boundary(end) {
        end -= 1;
    }
    let end = diff[..end].rfind('\n').map_or(end, |i| i + 1);
    let dropped = diff[end..].lines().count();
    format!(
        "{}… {dropped} more lines of this diff not shown. Read the files in question if the \
         rest decides it.",
        &diff[..end]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct Fixed;

    #[async_trait]
    impl Isolation for Fixed {
        async fn open(
            &self,
            _: &Path,
            _: &taurus_tools::Requester,
        ) -> Result<taurus_tools::Opened, String> {
            Err("none".into())
        }
        async fn close(&self, _: &str) -> Result<taurus_tools::Closed, String> {
            Err("none".into())
        }
        async fn apply(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<&taurus_tools::TurnRecorder>,
            _: bool,
        ) -> Result<taurus_tools::ApplyPlan, String> {
            Err("none".into())
        }
        async fn diff(&self, run: &str, _: Option<&str>) -> Result<String, String> {
            match run {
                "a" => Ok(" a.rs | 2 +-\n\n-old\n+new\n".into()),
                "empty" => Ok(String::new()),
                other => Err(format!("no run {other}")),
            }
        }
    }

    fn ctx() -> (ToolContext, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let permissions = Arc::new(taurus_tools::PermissionEngine::new(
            dir.path(),
            dir.path().join(".taurus"),
            Box::new(taurus_tools::AllowAll),
        ));
        let ctx = ToolContext::new(
            dir.path(),
            permissions,
            tokio_util::sync::CancellationToken::new(),
        );
        (ctx, dir)
    }

    #[tokio::test]
    async fn each_run_gets_its_own_section_and_failures_say_why() {
        let (ctx, _dir) = ctx();
        let tool = CompareDelegates::new(Arc::new(Fixed));
        let result = tool
            .execute(
                serde_json::json!({"runs": ["a", "empty", "gone", "a"]}),
                &ctx,
            )
            .await
            .unwrap();
        let out = result.to_text();
        assert_eq!(
            out,
            "## Run a\n\n a.rs | 2 +-\n\n-old\n+new\n\n\n## Run empty\n\n(changed nothing)\
             \n\n## Run gone\n\n(can't be shown: no run gone)"
        );
    }

    #[tokio::test]
    async fn too_many_runs_is_refused() {
        let (ctx, _dir) = ctx();
        let tool = CompareDelegates::new(Arc::new(Fixed));
        let runs: Vec<String> = (0..7).map(|n| n.to_string()).collect();
        assert!(tool
            .execute(serde_json::json!({ "runs": runs }), &ctx)
            .await
            .is_err());
    }

    #[test]
    fn a_long_diff_is_cut_on_a_line_and_says_so() {
        let diff = "line\n".repeat(100);
        let cut = cut(&diff, 22);
        assert!(cut.starts_with("line\nline\nline\nline\n…"), "{cut}");
        assert!(cut.contains("96 more lines"));
    }
}
