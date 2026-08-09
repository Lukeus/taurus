//! Filesystem tools.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};

/// Guards against a single read blowing the model's context window.
const MAX_READ_BYTES: usize = 256 * 1024;

#[derive(Deserialize, JsonSchema)]
pub struct ReadFileInput {
    /// Path to the file, relative to the workspace root or absolute within it.
    pub path: String,
}

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read a file's contents. Prefer this over running `cat`. Returns the text with 1-based \
         line numbers so you can reference specific lines."
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
        let path = ctx.resolve(&input.path)?;

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
            return Ok(format!("{} is empty.", ctx.display(&path)));
        }

        let mut out = String::with_capacity(text.len() + text.lines().count() * 6);
        for (i, line) in text.lines().enumerate() {
            out.push_str(&format!("{:>5}\t{line}\n", i + 1));
        }
        if truncated {
            out.push_str("\n[truncated: file exceeds the read limit]\n");
        }
        Ok(out)
    }
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
        Ok(format!("Wrote {bytes} bytes to {}", ctx.display(&path)))
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

        let count = original.matches(&old).count();
        let display = ctx.display(&path);
        match count {
            0 => Err(ToolError::InvalidInput(format!(
                "old_string was not found in {display}. Read the file again and match its exact \
                 current text, including whitespace."
            ))),
            n if n > 1 && !input.replace_all => Err(ToolError::InvalidInput(format!(
                "old_string appears {n} times in {display}. Include surrounding context to make \
                 it unique, or set replace_all."
            ))),
            n => {
                let updated = if input.replace_all {
                    original.replace(&old, &new)
                } else {
                    original.replacen(&old, &new, 1)
                };
                tokio::fs::write(&path, updated)
                    .await
                    .map_err(|e| ToolError::Failed(format!("cannot write {display}: {e}")))?;
                Ok(match n {
                    1 => format!("Edited {display}"),
                    n => format!("Edited {display} ({n} replacements)"),
                })
            }
        }
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
        let path = ctx.resolve(input.path.as_deref().unwrap_or("."))?;

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
            return Ok(format!("{} is empty.", ctx.display(&path)));
        }
        // Directories first, then alphabetical, so the listing reads the same
        // on every platform regardless of readdir order.
        rows.sort_by(|a, b| {
            let key = |s: &String| (!s.ends_with('/'), s.to_lowercase());
            key(a).cmp(&key(b))
        });
        Ok(rows.join("\n"))
    }
}

fn to_crlf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_ctx;

    #[tokio::test]
    async fn read_file_numbers_lines() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo").unwrap();
        let out = ReadFile
            .execute(serde_json::json!({"path": "a.txt"}), &ctx)
            .await
            .unwrap();
        assert!(out.contains("    1\tone"));
        assert!(out.contains("    2\ttwo"));
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
