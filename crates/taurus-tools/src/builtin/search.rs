//! Search tools. Both respect `.gitignore`, which keeps `node_modules` and
//! `target` out of the model's context without the model having to know to
//! exclude them.

use async_trait::async_trait;
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};

const MAX_RESULTS: usize = 200;
/// Files above this size are almost always minified bundles or binaries.
const MAX_GREP_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// The traversal both search tools use.
///
/// Dotfiles are visible because agents legitimately need `.github/`,
/// `.env.example`, and friends, but `.git` itself is skipped: walking object
/// storage is slow and every hit is noise. `.gitignore` is honored even when
/// the workspace is not a git repository, which is the behavior users expect
/// from a file they wrote specifically to exclude things.
fn walker(root: &std::path::Path) -> ignore::Walk {
    WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
}

#[derive(Deserialize, JsonSchema)]
pub struct GlobInput {
    /// Glob pattern, e.g. `**/*.rs` or `src/**/test_*.py`.
    pub pattern: String,
    /// Directory to search under. Defaults to the workspace root.
    #[serde(default)]
    pub path: Option<String>,
}

pub struct Glob;

#[async_trait]
impl Tool for Glob {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "Find files by name pattern. Use this to locate files when you know roughly what they are \
         called. Ignored files (.gitignore) are skipped."
    }
    fn input_schema(&self) -> serde_json::Value {
        schema_for::<GlobInput>()
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: GlobInput = parse_input(input)?;
        let root = ctx.resolve(input.path.as_deref().unwrap_or("."))?;
        let matcher = globset::Glob::new(&input.pattern)
            .map_err(|e| ToolError::InvalidInput(format!("bad glob pattern: {e}")))?
            .compile_matcher();

        let ctx = ctx.clone();
        let hits = tokio::task::spawn_blocking(move || {
            let mut hits = Vec::new();
            for entry in walker(&root).flatten() {
                if hits.len() >= MAX_RESULTS {
                    break;
                }
                if entry.file_type().is_some_and(|t| t.is_file()) {
                    let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
                    if matcher.is_match(rel) || matcher.is_match(entry.path()) {
                        hits.push(ctx.display(entry.path()));
                    }
                }
            }
            hits
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?;

        Ok(format_hits(hits, "files"))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct GrepInput {
    /// Regular expression to search for.
    pub pattern: String,
    /// Directory to search under. Defaults to the workspace root.
    #[serde(default)]
    pub path: Option<String>,
    /// Only search files whose name matches this glob, e.g. `*.rs`.
    #[serde(default)]
    pub include: Option<String>,
}

pub struct Grep;

#[async_trait]
impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search file contents with a regular expression. Returns `path:line: text` for each match. \
         Prefer this over reading files one by one when looking for a symbol or string."
    }
    fn input_schema(&self) -> serde_json::Value {
        schema_for::<GrepInput>()
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: GrepInput = parse_input(input)?;
        let root = ctx.resolve(input.path.as_deref().unwrap_or("."))?;
        let regex = regex::Regex::new(&input.pattern)
            .map_err(|e| ToolError::InvalidInput(format!("bad regex: {e}")))?;
        let include = input
            .include
            .as_deref()
            .map(|g| {
                globset::Glob::new(g)
                    .map(|g| g.compile_matcher())
                    .map_err(|e| ToolError::InvalidInput(format!("bad include glob: {e}")))
            })
            .transpose()?;

        let ctx = ctx.clone();
        let hits = tokio::task::spawn_blocking(move || {
            let mut hits = Vec::new();
            'walk: for entry in walker(&root).flatten() {
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                let path = entry.path();
                if let Some(m) = &include {
                    let rel = path.strip_prefix(&root).unwrap_or(path);
                    if !m.is_match(rel) && !m.is_match(path) {
                        continue;
                    }
                }
                if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_GREP_FILE_BYTES {
                    continue;
                }
                // Binary files read as garbage; skipping them silently is right
                // because the model asked about text.
                let Ok(text) = std::fs::read_to_string(path) else {
                    continue;
                };
                for (i, line) in text.lines().enumerate() {
                    if regex.is_match(line) {
                        hits.push(format!(
                            "{}:{}: {}",
                            ctx.display(path),
                            i + 1,
                            line.trim_end()
                        ));
                        if hits.len() >= MAX_RESULTS {
                            break 'walk;
                        }
                    }
                }
            }
            hits
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?;

        Ok(format_hits(hits, "matches"))
    }
}

fn format_hits(mut hits: Vec<String>, noun: &str) -> String {
    if hits.is_empty() {
        return format!("No {noun} found.");
    }
    let capped = hits.len() >= MAX_RESULTS;
    hits.sort();
    let mut out = hits.join("\n");
    if capped {
        out.push_str(&format!(
            "\n\n[stopped at {MAX_RESULTS} {noun}; narrow the search to see the rest]"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_ctx;

    fn seed(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    let x = 1;\n}\n").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn helper() {}\n").unwrap();
        std::fs::write(dir.join("notes.md"), "some fn text\n").unwrap();
    }

    #[tokio::test]
    async fn glob_matches_by_extension() {
        let (ctx, dir) = test_ctx();
        seed(dir.path());
        let out = Glob
            .execute(serde_json::json!({"pattern": "**/*.rs"}), &ctx)
            .await
            .unwrap();
        assert!(out.contains("src/main.rs"));
        assert!(out.contains("src/lib.rs"));
        assert!(!out.contains("notes.md"));
    }

    #[tokio::test]
    async fn glob_reports_no_matches_clearly() {
        let (ctx, dir) = test_ctx();
        seed(dir.path());
        let out = Glob
            .execute(serde_json::json!({"pattern": "**/*.zig"}), &ctx)
            .await
            .unwrap();
        assert_eq!(out, "No files found.");
    }

    #[tokio::test]
    async fn glob_rejects_a_bad_pattern() {
        let (ctx, _dir) = test_ctx();
        let err = Glob
            .execute(serde_json::json!({"pattern": "["}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn grep_returns_path_line_and_text() {
        let (ctx, dir) = test_ctx();
        seed(dir.path());
        let out = Grep
            .execute(serde_json::json!({"pattern": "fn main"}), &ctx)
            .await
            .unwrap();
        assert!(out.contains("src/main.rs:1: fn main() {"));
    }

    #[tokio::test]
    async fn grep_include_narrows_by_file_type() {
        let (ctx, dir) = test_ctx();
        seed(dir.path());
        let out = Grep
            .execute(
                serde_json::json!({"pattern": "fn", "include": "*.md"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("notes.md"));
        assert!(!out.contains(".rs"));
    }

    #[tokio::test]
    async fn grep_respects_gitignore() {
        let (ctx, dir) = test_ctx();
        seed(dir.path());
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir(dir.path().join("ignored")).unwrap();
        std::fs::write(dir.path().join("ignored/secret.rs"), "fn hidden() {}\n").unwrap();
        let out = Grep
            .execute(serde_json::json!({"pattern": "hidden"}), &ctx)
            .await
            .unwrap();
        assert_eq!(out, "No matches found.");
    }

    #[tokio::test]
    async fn grep_rejects_a_bad_regex() {
        let (ctx, _dir) = test_ctx();
        let err = Grep
            .execute(serde_json::json!({"pattern": "("}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
