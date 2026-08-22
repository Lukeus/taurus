//! Filesystem tools.

use std::path::Path;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::diff::FileDiff;
use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};

/// Guards against a single read blowing the model's context window.
const MAX_READ_BYTES: usize = 256 * 1024;

/// Lines returned when the caller does not ask for a range.
///
/// A file read is usually the largest thing a turn puts into the context
/// window, and it stays there for every later iteration of that turn. Returning
/// a window by default makes the common case — a long file the model needs one
/// region of — cost what the region costs rather than what the file costs.
const DEFAULT_READ_LINES: usize = 2000;

/// The `path` argument, for the tools whose whole effect is on one file.
///
/// Reads the raw JSON rather than the parsed input struct because a checkpoint
/// is taken before the tool validates anything: a call that is about to be
/// rejected costs one `read_to_string`, and a call that is not must not have
/// gone unrecorded because the snapshot ran too late.
fn touched_path(input: &serde_json::Value) -> Vec<String> {
    input
        .get("path")
        .and_then(|p| p.as_str())
        .map(|p| vec![p.to_string()])
        .unwrap_or_default()
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadFileInput {
    /// Path to the file, relative to the workspace root or absolute within it.
    pub path: String,
    /// 1-based line to start at. Defaults to the start of the file.
    #[serde(default)]
    pub offset: Option<usize>,
    /// How many lines to return. Defaults to 2000.
    #[serde(default)]
    pub limit: Option<usize>,
}

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read a file's contents. Prefer this over running `cat`. Returns the text with 1-based \
         line numbers so you can reference specific lines. Long files come back one window at a \
         time; pass `offset` and `limit` to ask for the part you need instead of the whole file."
    }
    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ReadFileInput>()
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Read {}",
            input.get("path").and_then(|p| p.as_str()).unwrap_or("?")
        )
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: ReadFileInput = parse_input(input)?;
        let path = ctx.resolve_read(&input.path)?;

        if path.is_dir() {
            return Err(ToolError::InvalidInput(format!(
                "{} is a directory; use list_dir",
                input.path
            )));
        }

        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| ToolError::Failed(format!("cannot read {}: {e}", ctx.display(&path))))?;

        let truncated = bytes.len() > MAX_READ_BYTES;
        let slice = if truncated {
            // Cut on a char boundary so the output stays valid UTF-8.
            let mut end = MAX_READ_BYTES;
            while end > 0 && !bytes.is_char_boundary_at(end) {
                end -= 1;
            }
            &bytes[..end]
        } else {
            &bytes[..]
        };

        let text = String::from_utf8_lossy(slice);
        if text.is_empty() {
            return Ok(format!("{} is empty.", ctx.display(&path)).into());
        }

        let lines: Vec<&str> = text.lines().collect();
        let start = input.offset.unwrap_or(1).max(1) - 1;
        let limit = input.limit.unwrap_or(DEFAULT_READ_LINES).max(1);

        // An offset past the end is a mistake worth naming, not an empty
        // result: the model asked for a region that does not exist and needs
        // the file's actual length to correct itself. On a truncated read that
        // length is unknown — `lines` is only the readable prefix — and
        // reporting it as the file's length would send the model off to correct
        // an offset against a number that is not the file's.
        if start >= lines.len() {
            return Err(ToolError::InvalidInput(if truncated {
                format!(
                    "{} is larger than the {} KB read limit; offset {} is past its readable first \
                     {} lines, and how many more there are is not knowable this way — use grep to \
                     locate what you need",
                    ctx.display(&path),
                    MAX_READ_BYTES / 1024,
                    start + 1,
                    lines.len()
                )
            } else {
                format!(
                    "{} has {} lines; offset {} is past the end",
                    ctx.display(&path),
                    lines.len(),
                    start + 1
                )
            }));
        }
        let end = start.saturating_add(limit).min(lines.len());

        let window = &lines[start..end];
        let mut out =
            String::with_capacity(window.iter().map(|l| l.len()).sum::<usize>() + window.len() * 8);
        for (i, line) in window.iter().enumerate() {
            // Numbered by absolute position, not by position in the window, so
            // a line number from a windowed read still means what it says.
            out.push_str(&format!("{:>5}\t{line}\n", start + i + 1));
        }
        if truncated || start > 0 || end < lines.len() {
            out.push_str(&range_note(start + 1, end, lines.len(), truncated));
        }
        Ok(out.into())
    }
}

/// Tells the model what it just got and how to get the rest.
///
/// Stated every time the answer is partial, because a window that does not say
/// it is a window is indistinguishable from a short file, and a model that
/// believes it has read the whole thing will act on what is missing.
fn range_note(first: usize, last: usize, available: usize, truncated: bool) -> String {
    let mut note = format!("\n[showing lines {first}-{last} of {available}");
    if truncated {
        note.push_str(&format!(
            " readable; the file is larger than the {} KB read limit, so the rest is not \
             reachable this way — use grep to locate it",
            MAX_READ_BYTES / 1024
        ));
    }
    if last < available {
        note.push_str(&format!("; read again with offset {} for more", last + 1));
    }
    note.push_str("]\n");
    note
}

/// `str::is_char_boundary` for a byte slice we are about to lossy-decode.
trait CharBoundary {
    fn is_char_boundary_at(&self, index: usize) -> bool;
}

impl CharBoundary for Vec<u8> {
    fn is_char_boundary_at(&self, index: usize) -> bool {
        index >= self.len() || (self[index] & 0xC0) != 0x80
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteFileInput {
    /// Path to write, relative to the workspace root.
    pub path: String,
    /// Full contents of the file. Any existing file is replaced.
    pub content: String,
}

pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Create a file or replace its entire contents. To change part of an existing file, use \
         edit_file instead so the rest is preserved."
    }
    fn input_schema(&self) -> serde_json::Value {
        schema_for::<WriteFileInput>()
    }
    fn effect(&self) -> Effect {
        Effect::Write
    }
    fn preview(&self, input: &serde_json::Value) -> String {
        let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("?");
        let bytes = input
            .get("content")
            .and_then(|c| c.as_str())
            .map_or(0, str::len);
        format!("Write {path} ({bytes} bytes)")
    }
    fn touches(&self, input: &serde_json::Value) -> Vec<String> {
        touched_path(input)
    }

    /// The line above says which file and how many bytes. This says what the
    /// bytes are — which for an overwrite is the whole decision.
    async fn diff(&self, input: &serde_json::Value, workspace: &Path) -> Option<FileDiff> {
        let path = input.get("path")?.as_str()?;
        let content = input.get("content")?.as_str()?;
        // Resolved through the same guard the call itself will use, so a path
        // the write would refuse is never read to draw a picture of it.
        let resolved = crate::path_guard::resolve(workspace, path).ok()?;
        crate::diff::against_disk(workspace, &resolved, content)
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: WriteFileInput = parse_input(input)?;
        let path = ctx.resolve(&input.path)?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolError::Failed(format!("cannot create {}: {e}", ctx.display(parent)))
            })?;
        }

        // Match the file's existing convention rather than imposing LF, so a
        // write into a CRLF repository does not show up as a whole-file diff.
        let existing = tokio::fs::read_to_string(&path).await.ok();
        let content = match existing.as_deref() {
            Some(prior) if prior.contains("\r\n") => to_crlf(&input.content),
            _ => input.content,
        };

        let bytes = content.len();
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| ToolError::Failed(format!("cannot write {}: {e}", ctx.display(&path))))?;
        Ok(format!("Wrote {bytes} bytes to {}", ctx.display(&path)).into())
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct EditFileInput {
    pub path: String,
    /// Exact text to find. Must appear in the file, and must be unique unless
    /// `replace_all` is set.
    pub old_string: String,
    /// Text to put in its place.
    pub new_string: String,
    /// Replace every occurrence instead of requiring a unique match.
    #[serde(default)]
    pub replace_all: bool,
}

pub struct EditFile;

#[async_trait]
impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }
    fn description(&self) -> &str {
        "Replace exact text in a file. Read the file first so old_string matches byte for byte, \
         including indentation. old_string must be unique in the file unless replace_all is true."
    }
    fn input_schema(&self) -> serde_json::Value {
        schema_for::<EditFileInput>()
    }
    fn effect(&self) -> Effect {
        Effect::Write
    }
    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Edit {}",
            input.get("path").and_then(|p| p.as_str()).unwrap_or("?")
        )
    }
    fn touches(&self, input: &serde_json::Value) -> Vec<String> {
        touched_path(input)
    }

    /// Computed by running the same replacement the call will run.
    ///
    /// Through [`apply_edit`] rather than a second implementation of it: a
    /// dialog that shows one change while the tool makes another is worse than
    /// showing nothing, because it is the thing the user believed when they
    /// approved it.
    async fn diff(&self, input: &serde_json::Value, workspace: &Path) -> Option<FileDiff> {
        let input: EditFileInput = serde_json::from_value(input.clone()).ok()?;
        let path = crate::path_guard::resolve(workspace, &input.path).ok()?;
        let original = std::fs::read_to_string(&path).ok()?;
        let (updated, _) = apply_edit(&original, &input).ok()?;
        Some(crate::diff::between(
            crate::path_guard::display(workspace, &path),
            &original,
            &updated,
        ))
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: EditFileInput = parse_input(input)?;
        if input.old_string == input.new_string {
            return Err(ToolError::InvalidInput(
                "old_string and new_string are identical".into(),
            ));
        }
        let path = ctx.resolve(&input.path)?;
        let original = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::Failed(format!("cannot read {}: {e}", ctx.display(&path))))?;

        let display = ctx.display(&path);
        let (updated, count) = apply_edit(&original, &input).map_err(|e| e.explain(&display))?;

        tokio::fs::write(&path, updated)
            .await
            .map_err(|e| ToolError::Failed(format!("cannot write {display}: {e}")))?;
        Ok(match count {
            1 => format!("Edited {display}").into(),
            n => format!("Edited {display} ({n} replacements)").into(),
        })
    }
}

/// Why a replacement could not be made, without the file's name in it.
///
/// Separate from the message so [`apply_edit`] can be called where there is
/// nothing to name — the permission prompt resolves a path the user has not
/// approved yet — while `execute` still produces the wording the model reads.
enum EditProblem {
    NotFound,
    Ambiguous(usize),
}

impl EditProblem {
    fn explain(self, display: &str) -> ToolError {
        ToolError::InvalidInput(match self {
            Self::NotFound => format!(
                "old_string was not found in {display}. Read the file again and match its exact \
                 current text, including whitespace."
            ),
            Self::Ambiguous(n) => format!(
                "old_string appears {n} times in {display}. Include surrounding context to make \
                 it unique, or set replace_all."
            ),
        })
    }
}

/// Applies an edit and reports how many occurrences it replaced.
///
/// The one place the replacement is worked out, so the diff the user approves
/// and the bytes that get written cannot disagree.
fn apply_edit(original: &str, input: &EditFileInput) -> Result<(String, usize), EditProblem> {
    // The model reasons in LF because that is how read_file presented the
    // file; translate its strings into the file's own convention.
    let crlf = original.contains("\r\n");
    let old = if crlf {
        to_crlf(&input.old_string)
    } else {
        input.old_string.clone()
    };
    let new = if crlf {
        to_crlf(&input.new_string)
    } else {
        input.new_string.clone()
    };

    match original.matches(&old).count() {
        0 => Err(EditProblem::NotFound),
        n if n > 1 && !input.replace_all => Err(EditProblem::Ambiguous(n)),
        n => Ok((
            if input.replace_all {
                original.replace(&old, &new)
            } else {
                original.replacen(&old, &new, 1)
            },
            n,
        )),
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ListDirInput {
    /// Directory to list. Defaults to the workspace root.
    #[serde(default)]
    pub path: Option<String>,
}

pub struct ListDir;

#[async_trait]
impl Tool for ListDir {
    fn name(&self) -> &str {
        "list_dir"
    }
    fn description(&self) -> &str {
        "List the entries of a directory. Directories are marked with a trailing slash."
    }
    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ListDirInput>()
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: ListDirInput = parse_input(input)?;
        let path = ctx.resolve_read(input.path.as_deref().unwrap_or("."))?;

        let mut entries = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| ToolError::Failed(format!("cannot list {}: {e}", ctx.display(&path))))?;

        let mut rows = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            rows.push(if is_dir { format!("{name}/") } else { name });
        }

        if rows.is_empty() {
            return Ok(format!("{} is empty.", ctx.display(&path)).into());
        }
        // Directories first, then alphabetical, so the listing reads the same
        // on every platform regardless of readdir order.
        rows.sort_by(|a, b| {
            let key = |s: &String| (!s.ends_with('/'), s.to_lowercase());
            key(a).cmp(&key(b))
        });
        Ok(rows.join("\n").into())
    }
}

fn to_crlf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_ctx;

    /// The claim the whole feature rests on: the diff the user approves and the
    /// bytes that get written are computed by the same code, so they cannot
    /// disagree. A dialog showing one change while the tool makes another is
    /// worse than showing none, because it is what the user believed.
    #[tokio::test]
    async fn the_edit_diff_is_what_the_edit_actually_does() {
        let (ctx, dir) = test_ctx();
        let root = ctx.workspace.clone();
        std::fs::write(dir.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
        let input = serde_json::json!({
            "path": "a.rs", "old_string": "two", "new_string": "TWO",
        });

        let diff = EditFile.diff(&input, &root).await.expect("a diff");
        assert_eq!((diff.added, diff.removed), (1, 1));
        assert!(!diff.created);

        EditFile.execute(input, &ctx).await.unwrap();
        let after = std::fs::read_to_string(dir.path().join("a.rs")).unwrap();

        // Replay the diff's added lines and check they are the file that exists.
        let added: Vec<&str> = diff
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| l.kind != crate::diff::DiffLineKind::Removed)
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(added.join("\n") + "\n", after);
    }

    #[tokio::test]
    async fn an_edit_that_cannot_apply_offers_no_diff_and_still_prompts() {
        // A diff is evidence offered with the decision, never a precondition
        // for making one — the prompt still has to appear.
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.rs"), "one\n").unwrap();
        let input = serde_json::json!({
            "path": "a.rs", "old_string": "nowhere", "new_string": "x",
        });
        assert!(EditFile.diff(&input, &ctx.workspace).await.is_none());
    }

    #[tokio::test]
    async fn writing_a_new_file_reads_as_a_creation() {
        let (ctx, _dir) = test_ctx();
        let diff = WriteFile
            .diff(
                &serde_json::json!({"path": "new.txt", "content": "hello\n"}),
                &ctx.workspace,
            )
            .await
            .expect("a diff");
        assert!(diff.created);
        assert_eq!(diff.removed, 0);
    }

    #[tokio::test]
    async fn overwriting_shows_what_is_being_destroyed() {
        // The case the byte count could not speak to.
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "keep\nlose\n").unwrap();
        let diff = WriteFile
            .diff(
                &serde_json::json!({"path": "a.txt", "content": "keep\n"}),
                &ctx.workspace,
            )
            .await
            .expect("a diff");
        assert!(!diff.created);
        assert_eq!(diff.removed, 1);
        let removed: Vec<&str> = diff.hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == crate::diff::DiffLineKind::Removed)
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(removed, ["lose"]);
    }

    #[tokio::test]
    async fn a_path_outside_the_workspace_is_not_read_to_draw_a_picture_of_it() {
        // Resolved through the same guard the write will use, so the prompt
        // cannot become a way to read a file the tool would refuse to touch.
        let (ctx, _dir) = test_ctx();
        let escaped = serde_json::json!({"path": "../outside.txt", "content": "x"});
        assert!(WriteFile.diff(&escaped, &ctx.workspace).await.is_none());
    }

    #[tokio::test]
    async fn read_file_numbers_lines() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo").unwrap();
        let out = ReadFile
            .execute(serde_json::json!({"path": "a.txt"}), &ctx)
            .await
            .unwrap();
        assert!(out.to_text().contains("    1\tone"));
        assert!(out.to_text().contains("    2\ttwo"));
    }

    /// A file of `n` numbered lines, for exercising windowed reads.
    fn lines_file(dir: &std::path::Path, name: &str, n: usize) {
        let body: String = (1..=n).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[tokio::test]
    async fn a_short_file_reads_whole_and_says_nothing_about_ranges() {
        let (ctx, dir) = test_ctx();
        lines_file(dir.path(), "a.txt", 3);
        let out = ReadFile
            .execute(serde_json::json!({"path": "a.txt"}), &ctx)
            .await
            .unwrap();
        assert!(out.to_text().contains("    3\tline 3"));
        assert!(!out.to_text().contains("showing lines"), "{out}");
    }

    #[tokio::test]
    async fn a_long_file_stops_at_the_default_window_and_says_how_to_continue() {
        let (ctx, dir) = test_ctx();
        lines_file(dir.path(), "big.txt", DEFAULT_READ_LINES + 50);
        let out = ReadFile
            .execute(serde_json::json!({"path": "big.txt"}), &ctx)
            .await
            .unwrap();
        assert!(out.to_text().contains(&format!(
            "{:>5}\tline {}",
            DEFAULT_READ_LINES, DEFAULT_READ_LINES
        )));
        assert!(!out
            .to_text()
            .contains(&format!("line {}", DEFAULT_READ_LINES + 1)));
        assert!(
            out.to_text().contains(&format!(
                "read again with offset {}",
                DEFAULT_READ_LINES + 1
            )),
            "{out}"
        );
    }

    #[tokio::test]
    async fn a_windowed_read_numbers_lines_by_absolute_position() {
        let (ctx, dir) = test_ctx();
        lines_file(dir.path(), "a.txt", 100);
        let out = ReadFile
            .execute(
                serde_json::json!({"path": "a.txt", "offset": 40, "limit": 2}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("   40\tline 40"), "{out}");
        assert!(out.to_text().contains("   41\tline 41"), "{out}");
        assert!(!out.to_text().contains("line 42"), "{out}");
        assert!(
            out.to_text().contains("showing lines 40-41 of 100"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn a_window_reaching_the_end_does_not_invite_another_read() {
        let (ctx, dir) = test_ctx();
        lines_file(dir.path(), "a.txt", 10);
        let out = ReadFile
            .execute(
                serde_json::json!({"path": "a.txt", "offset": 9, "limit": 500}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("showing lines 9-10 of 10"), "{out}");
        assert!(!out.to_text().contains("read again"), "{out}");
    }

    #[tokio::test]
    async fn an_offset_past_the_end_reports_the_files_length() {
        let (ctx, dir) = test_ctx();
        lines_file(dir.path(), "a.txt", 5);
        let err = ReadFile
            .execute(serde_json::json!({"path": "a.txt", "offset": 99}), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("has 5 lines"), "{err}");
    }

    /// Past the read limit the line count is the prefix's, not the file's, and
    /// stating it as the file's would have the model correct its offset against
    /// a number that belongs to nothing.
    #[tokio::test]
    async fn an_offset_past_a_truncated_read_does_not_claim_to_know_the_length() {
        let (ctx, dir) = test_ctx();
        // Comfortably past the 256 KB limit at ~9 bytes a line.
        lines_file(dir.path(), "big.txt", 40_000);
        let err = ReadFile
            .execute(
                serde_json::json!({"path": "big.txt", "offset": 39_000}),
                &ctx,
            )
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("larger than the 256 KB read limit"),
            "{message}"
        );
        assert!(!message.contains("has 40000 lines"), "{message}");
        assert!(message.contains("readable first"), "{message}");
        assert!(message.contains("grep"), "{message}");
    }

    #[tokio::test]
    async fn read_file_on_a_directory_points_at_list_dir() {
        let (ctx, dir) = test_ctx();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        let err = ReadFile
            .execute(serde_json::json!({"path": "d"}), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("list_dir"));
    }

    #[tokio::test]
    async fn write_file_creates_parent_directories() {
        let (ctx, dir) = test_ctx();
        WriteFile
            .execute(
                serde_json::json!({"path": "deep/nested/a.txt", "content": "hi"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("deep/nested/a.txt")).unwrap(),
            "hi"
        );
    }

    #[tokio::test]
    async fn write_file_preserves_crlf_line_endings() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("w.txt"), "a\r\nb\r\n").unwrap();
        WriteFile
            .execute(
                serde_json::json!({"path": "w.txt", "content": "x\ny\n"}),
                &ctx,
            )
            .await
            .unwrap();
        let text = std::fs::read_to_string(dir.path().join("w.txt")).unwrap();
        assert_eq!(text, "x\r\ny\r\n");
    }

    #[tokio::test]
    async fn edit_file_replaces_a_unique_match() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "hello world").unwrap();
        EditFile
            .execute(
                serde_json::json!({"path": "a.txt", "old_string": "world", "new_string": "there"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello there"
        );
    }

    #[tokio::test]
    async fn edit_file_refuses_an_ambiguous_match() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "x x").unwrap();
        let err = EditFile
            .execute(
                serde_json::json!({"path": "a.txt", "old_string": "x", "new_string": "y"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("appears 2 times"));
    }

    #[tokio::test]
    async fn edit_file_replace_all_accepts_ambiguity() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "x x").unwrap();
        EditFile
            .execute(
                serde_json::json!({
                    "path": "a.txt", "old_string": "x", "new_string": "y", "replace_all": true
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "y y"
        );
    }

    #[tokio::test]
    async fn edit_file_matches_lf_input_against_a_crlf_file() {
        // The model only ever sees LF, because that is what read_file shows it.
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("c.txt"), "one\r\ntwo\r\n").unwrap();
        EditFile
            .execute(
                serde_json::json!({
                    "path": "c.txt", "old_string": "one\ntwo", "new_string": "1\n2"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("c.txt")).unwrap(),
            "1\r\n2\r\n"
        );
    }

    #[tokio::test]
    async fn edit_file_reports_a_missing_match_actionably() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let err = EditFile
            .execute(
                serde_json::json!({"path": "a.txt", "old_string": "nope", "new_string": "x"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn list_dir_sorts_directories_first() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("a_dir")).unwrap();
        let out = ListDir.execute(serde_json::json!({}), &ctx).await.unwrap();
        let out = out.to_text();
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(lines[0], "a_dir/");
        assert!(lines.contains(&"b.txt"));
    }

    #[tokio::test]
    async fn tools_refuse_paths_outside_the_workspace() {
        let (ctx, _dir) = test_ctx();
        let err = ReadFile
            .execute(serde_json::json!({"path": "../../etc/passwd"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::OutsideWorkspace { .. }));
    }
}
