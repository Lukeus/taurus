//! Search tools. Both respect `.gitignore`, which keeps `node_modules` and
//! `target` out of the model's context without the model having to know to
//! exclude them.

use async_trait::async_trait;
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::budget::OutputBudget;
use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};

/// What a search may return, as a share of what has to hold it.
///
/// Anchored so [`OutputBudget::ANCHOR_WINDOW`] still gets the 200 this was,
/// against a matched line costing about eighty bytes with its path on the
/// front.
const RESULT_SHARE: f32 = 0.02;
const RESULT_BYTES: usize = 80;
/// Fewer than this and a search stops being able to answer the question it was
/// asked, whatever it is competing for room with.
const MIN_RESULTS: usize = 40;
/// More than this and the answer is a listing, which is a different tool.
const MAX_RESULTS: usize = 1_000;
/// Files above this size are almost always minified bundles or binaries.
const MAX_GREP_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// The most context [`Grep`] will put around a match.
///
/// Ten lines each side is a function; past that the model asked for a file and
/// should say so, where `read_file` can window it and say what it left out.
const MAX_CONTEXT: usize = 10;
/// What a search may return, however many matches it found.
///
/// A match cost one line before context existed, so the match cap was the
/// whole budget. With ten lines each side, two hundred matches is four
/// thousand lines, which is most of a small model's window spent on a search.
const GREP_OUTPUT_SHARE: f32 = 0.08;
const MIN_GREP_OUTPUT_BYTES: usize = 4 * 1024;
const MAX_GREP_OUTPUT_BYTES: usize = 512 * 1024;

/// The traversal both search tools use.
///
/// Dotfiles are visible because agents legitimately need `.github/`,
/// `.env.example`, and friends, but `.git` itself is skipped: walking object
/// storage is slow and every hit is noise. `.gitignore` is honored even when
/// the workspace is not a git repository, which is the behavior users expect
/// from a file they wrote specifically to exclude things.
///
/// Shared with [`crate::sweep`], which decides what a command changed. The two
/// have to agree: a file the agent cannot find with `grep` but can silently
/// destroy with `sed` would be the worst of both.
fn walker(root: &std::path::Path) -> ignore::Walk {
    walker_skipping(root, &[".git"])
}

/// The same traversal, with the skip list passed in.
///
/// [`crate::sweep`] skips one directory more than search does — see the list it
/// passes. Sharing the builder rather than copying it is what keeps the two
/// from drifting apart on everything else.
pub(crate) fn walker_skipping(
    root: &std::path::Path,
    skip: &'static [&'static str],
) -> ignore::Walk {
    WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(move |entry| !skip.iter().any(|name| entry.file_name() == *name))
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
        let root = ctx.resolve_read(input.path.as_deref().unwrap_or("."))?;
        let matcher = globset::Glob::new(&input.pattern)
            .map_err(|e| ToolError::InvalidInput(format!("bad glob pattern: {e}")))?
            .compile_matcher();

        let cap = result_cap(ctx);
        let ctx = ctx.clone();
        let hits = tokio::task::spawn_blocking(move || {
            let mut hits = Vec::new();
            for entry in walker(&root).flatten() {
                if hits.len() >= cap {
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

        Ok(format_hits(hits, "files", cap).into())
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
    /// Skip files whose name matches this glob, e.g. `*.min.js`.
    #[serde(default)]
    pub exclude: Option<String>,
    /// Match regardless of case.
    #[serde(default)]
    pub case_insensitive: bool,
    /// How many lines on each side of a match to show with it, up to 10.
    /// Defaults to none.
    #[serde(default)]
    pub context: Option<usize>,
    /// Return the paths of the files that matched, without the matching lines.
    #[serde(default)]
    pub files_only: bool,
    /// Return at most this many matches. Defaults to 200, which is also the
    /// most any search returns.
    #[serde(default)]
    pub limit: Option<usize>,
}

pub struct Grep;

#[async_trait]
impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search file contents with a regular expression. Returns `path:line: text` for each match. \
         Pass `context` to get the surrounding lines with it, as `path:line- text` — that is \
         cheaper than reading the whole file afterwards to see what a match sits in. Narrow with \
         `include` and `exclude` globs, set `files_only` when you only need to know which files \
         matched, and `limit` when a handful of matches would answer the question. Prefer this \
         over reading files one by one when looking for a symbol or string."
    }
    fn input_schema(&self) -> serde_json::Value {
        schema_for::<GrepInput>()
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: GrepInput = parse_input(input)?;
        let root = ctx.resolve_read(input.path.as_deref().unwrap_or("."))?;
        let regex = regex::RegexBuilder::new(&input.pattern)
            .case_insensitive(input.case_insensitive)
            .build()
            .map_err(|e| ToolError::InvalidInput(format!("bad regex: {e}")))?;
        let whole_file_first = !is_anchored(&input.pattern);
        let include = compile_glob(input.include.as_deref(), "include")?;
        let exclude = compile_glob(input.exclude.as_deref(), "exclude")?;
        let context = input.context.unwrap_or(0).min(MAX_CONTEXT);
        let cap = result_cap(ctx);
        let limit = input.limit.unwrap_or(cap).clamp(1, cap);
        let budget = ctx.budget;
        let files_only = input.files_only;

        let ctx = ctx.clone();
        let (files, capped) = tokio::task::spawn_blocking(move || {
            let mut files: Vec<FileMatches> = Vec::new();
            let mut found = 0usize;
            for entry in walker(&root).flatten() {
                if found >= limit {
                    break;
                }
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                let path = entry.path();
                let rel = path.strip_prefix(&root).unwrap_or(path);
                if let Some(m) = &include {
                    if !m.is_match(rel) && !m.is_match(path) {
                        continue;
                    }
                }
                if let Some(m) = &exclude {
                    if m.is_match(rel) || m.is_match(path) {
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
                // One pass over the file answers "is there anything here at
                // all", and in a repository most files are a no. Asking the
                // same question line by line pays the match machinery's setup
                // once per line to reach the same answer. Only safe for a
                // pattern that cannot tell a line from a file — see
                // [`is_anchored`].
                if whole_file_first && !regex.is_match(&text) {
                    continue;
                }
                if let Some(matched) =
                    matches_in(ctx.display(path), &text, &regex, context, limit - found)
                {
                    found += matched.count;
                    files.push(matched);
                }
            }
            (files, found >= limit)
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?;

        Ok(render(files, capped, files_only, limit, budget).into())
    }
}

/// One file's share of the answer, already cut down to the lines that will be
/// shown.
///
/// Cut here rather than at rendering time because the alternative is holding
/// every matching file's text until the walk finishes: two hundred matches can
/// be spread over two hundred files, and this tool will read a file of up to
/// [`MAX_GREP_FILE_BYTES`].
struct FileMatches {
    path: String,
    rows: Vec<Row>,
    /// Matching lines, which is what `limit` counts — not `rows`, which the
    /// context lines inflate.
    count: usize,
}

struct Row {
    /// 1-based, as the file's own line numbers.
    line: usize,
    matched: bool,
    text: String,
}

/// Finds up to `remaining` matching lines, and the context around them.
///
/// Returns `None` for a file with no matches so the caller can skip it without
/// a second emptiness check.
fn matches_in(
    path: String,
    text: &str,
    regex: &regex::Regex,
    context: usize,
    remaining: usize,
) -> Option<FileMatches> {
    let lines: Vec<&str> = text.lines().collect();
    let mut hits: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if regex.is_match(line) {
            hits.push(i);
            if hits.len() >= remaining {
                break;
            }
        }
    }
    if hits.is_empty() {
        return None;
    }

    let mut rows = Vec::new();
    // Where the next window may start, so two matches close enough for their
    // context to overlap produce one run of lines rather than the shared ones
    // twice.
    let mut next = 0usize;
    for &hit in &hits {
        let first = hit.saturating_sub(context).max(next);
        let last = (hit + context).min(lines.len() - 1);
        for (i, line) in lines.iter().enumerate().take(last + 1).skip(first) {
            rows.push(Row {
                line: i + 1,
                matched: hits.binary_search(&i).is_ok(),
                text: line.trim_end().to_string(),
            });
        }
        next = last + 1;
    }

    Some(FileMatches {
        path,
        rows,
        count: hits.len(),
    })
}

/// Turns the walk's findings into what the model reads.
///
/// Sorted by path, and within a path by line, because the walk's order is the
/// filesystem's. Sorting the formatted strings instead — which is what this
/// used to do — orders `foo.rs:100` before `foo.rs:9`, so a file's own matches
/// arrived shuffled.
fn render(
    mut files: Vec<FileMatches>,
    capped: bool,
    files_only: bool,
    limit: usize,
    budget: OutputBudget,
) -> String {
    let output_cap = budget.bytes(
        GREP_OUTPUT_SHARE,
        MIN_GREP_OUTPUT_BYTES,
        MAX_GREP_OUTPUT_BYTES,
    );
    if files.is_empty() {
        return "No matches found.".to_string();
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    if files_only {
        let mut out = files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if capped {
            out.push_str(&cap_note(limit));
        }
        return out;
    }

    let mut out = String::new();
    let mut truncated = false;
    'files: for file in &files {
        let mut previous: Option<usize> = None;
        for row in &file.rows {
            // A gap means the lines above and below it are not neighbours in
            // the file, and a reader who assumes they are is reading code that
            // does not exist.
            if previous.is_some_and(|p| row.line != p + 1) {
                out.push_str("--\n");
            }
            let separator = if row.matched { ':' } else { '-' };
            out.push_str(&format!(
                "{}:{}{} {}\n",
                file.path, row.line, separator, row.text
            ));
            previous = Some(row.line);
            if out.len() >= output_cap {
                truncated = true;
                break 'files;
            }
        }
    }
    while out.ends_with('\n') {
        out.pop();
    }
    if truncated {
        out.push_str(&format!(
            "\n\n[stopped at {} KB of output; search for less, or with less context]",
            output_cap / 1024
        ));
    } else if capped {
        out.push_str(&cap_note(limit));
    }
    out
}

fn cap_note(limit: usize) -> String {
    format!("\n\n[stopped at {limit} matches; narrow the search to see the rest]")
}

/// Whether the pattern can tell a line from the file it is in.
///
/// [`Grep`] matches line by line, so `^` is the start of every line. Run over
/// a whole file the same pattern means the start of the file, and the two
/// disagree — which the prefilter may only do in the direction of extra work.
/// An unanchored pattern cannot disagree at all: `.` does not cross a newline,
/// so a whole-file match lies inside one line, and that line matches on its
/// own. Anchored patterns skip the prefilter rather than lose matches to it.
fn is_anchored(pattern: &str) -> bool {
    pattern.contains('^')
        || pattern.contains('$')
        || pattern.contains("\\A")
        || pattern.contains("\\z")
        || pattern.contains("\\Z")
}

fn compile_glob(
    pattern: Option<&str>,
    field: &str,
) -> Result<Option<globset::GlobMatcher>, ToolError> {
    pattern
        .map(|g| {
            globset::Glob::new(g)
                .map(|g| g.compile_matcher())
                .map_err(|e| ToolError::InvalidInput(format!("bad {field} glob: {e}")))
        })
        .transpose()
}

/// How many results this model's window has room for.
fn result_cap(ctx: &ToolContext) -> usize {
    ctx.budget
        .count(RESULT_SHARE, RESULT_BYTES, MIN_RESULTS, MAX_RESULTS)
}

fn format_hits(mut hits: Vec<String>, noun: &str, cap: usize) -> String {
    if hits.is_empty() {
        return format!("No {noun} found.");
    }
    let capped = hits.len() >= cap;
    hits.sort();
    let mut out = hits.join("\n");
    if capped {
        out.push_str(&format!(
            "\n\n[stopped at {cap} {noun}; narrow the search to see the rest]"
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
        assert!(out.to_text().contains("src/main.rs"));
        assert!(out.to_text().contains("src/lib.rs"));
        assert!(!out.to_text().contains("notes.md"));
    }

    #[tokio::test]
    async fn glob_reports_no_matches_clearly() {
        let (ctx, dir) = test_ctx();
        seed(dir.path());
        let out = Glob
            .execute(serde_json::json!({"pattern": "**/*.zig"}), &ctx)
            .await
            .unwrap();
        assert_eq!(out.to_text(), "No files found.");
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
        assert!(out.to_text().contains("src/main.rs:1: fn main() {"));
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
        assert!(out.to_text().contains("notes.md"));
        assert!(!out.to_text().contains(".rs"));
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
        assert_eq!(out.to_text(), "No matches found.");
    }

    #[tokio::test]
    async fn grep_context_brings_the_surrounding_lines() {
        let (ctx, dir) = test_ctx();
        seed(dir.path());
        let out = Grep
            .execute(serde_json::json!({"pattern": "let x", "context": 1}), &ctx)
            .await
            .unwrap();
        let text = out.to_text();
        // The match keeps its colon; its neighbours are marked as context.
        assert!(text.contains("src/main.rs:2:     let x = 1;"), "{text}");
        assert!(text.contains("src/main.rs:1- fn main() {"), "{text}");
        assert!(text.contains("src/main.rs:3- }"), "{text}");
    }

    #[tokio::test]
    async fn grep_context_does_not_repeat_shared_lines() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "hit\nmiddle\nhit\n").unwrap();
        let out = Grep
            .execute(serde_json::json!({"pattern": "hit", "context": 2}), &ctx)
            .await
            .unwrap();
        let text = out.to_text();
        assert_eq!(text.lines().count(), 3, "{text}");
        assert_eq!(text.matches("a.txt:2").count(), 1, "{text}");
    }

    #[tokio::test]
    async fn grep_marks_a_gap_between_hunks() {
        let (ctx, dir) = test_ctx();
        let mut body = String::from("hit\n");
        body.push_str(&"filler\n".repeat(10));
        body.push_str("hit\n");
        std::fs::write(dir.path().join("a.txt"), body).unwrap();
        let out = Grep
            .execute(serde_json::json!({"pattern": "hit", "context": 1}), &ctx)
            .await
            .unwrap();
        assert!(out.to_text().contains("\n--\n"), "{}", out.to_text());
    }

    #[tokio::test]
    async fn grep_orders_a_file_by_line_number() {
        let (ctx, dir) = test_ctx();
        let body: String = (1..=120).map(|i| format!("hit {i}\n")).collect();
        std::fs::write(dir.path().join("a.txt"), body).unwrap();
        let out = Grep
            .execute(serde_json::json!({"pattern": "hit"}), &ctx)
            .await
            .unwrap();
        let text = out.to_text();
        let numbers: Vec<usize> = text
            .lines()
            .filter_map(|l| l.split(':').nth(1))
            .filter_map(|n| n.parse().ok())
            .collect();
        assert_eq!(numbers.len(), 120, "{text}");
        assert!(numbers.windows(2).all(|w| w[0] < w[1]), "{numbers:?}");
    }

    #[tokio::test]
    async fn grep_files_only_lists_paths_once() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "hit\nhit\n").unwrap();
        let out = Grep
            .execute(
                serde_json::json!({"pattern": "hit", "files_only": true}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.to_text(), "a.txt");
    }

    #[tokio::test]
    async fn grep_limit_stops_early_and_says_so() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "hit\nhit\nhit\n").unwrap();
        let out = Grep
            .execute(serde_json::json!({"pattern": "hit", "limit": 2}), &ctx)
            .await
            .unwrap();
        let text = out.to_text();
        assert_eq!(text.matches("a.txt:").count(), 2, "{text}");
        assert!(text.contains("stopped at 2 matches"), "{text}");
    }

    #[tokio::test]
    async fn grep_exclude_skips_matching_files() {
        let (ctx, dir) = test_ctx();
        seed(dir.path());
        let out = Grep
            .execute(
                serde_json::json!({"pattern": "fn", "exclude": "*.md"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!out.to_text().contains("notes.md"), "{}", out.to_text());
        assert!(out.to_text().contains("src/main.rs"));
    }

    #[tokio::test]
    async fn grep_can_ignore_case() {
        let (ctx, dir) = test_ctx();
        seed(dir.path());
        let out = Grep
            .execute(
                serde_json::json!({"pattern": "FN MAIN", "case_insensitive": true}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            out.to_text().contains("src/main.rs:1:"),
            "{}",
            out.to_text()
        );
    }

    /// The whole-file prefilter must not answer for an anchored pattern: `^`
    /// means the start of a line here, and a file's second line is not the
    /// start of the file.
    #[tokio::test]
    async fn grep_finds_an_anchored_match_below_the_first_line() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "first\nsecond\n").unwrap();
        let out = Grep
            .execute(serde_json::json!({"pattern": "^second"}), &ctx)
            .await
            .unwrap();
        assert_eq!(out.to_text(), "a.txt:2: second");
    }

    #[tokio::test]
    async fn grep_rejects_a_bad_exclude_glob() {
        let (ctx, _dir) = test_ctx();
        let err = Grep
            .execute(serde_json::json!({"pattern": "x", "exclude": "["}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(m) if m.contains("exclude")));
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
