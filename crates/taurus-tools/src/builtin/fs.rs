//! Filesystem tools.

use std::path::Path;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::budget::OutputBudget;
use crate::diff::FileDiff;
use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};

/// Guards against a single read blowing the model's context window.
///
/// A bound on what one call *answers with*, not on how far into a file it
/// may look. Those were the same thing while a read always started at the
/// first byte, and a file past this size had a tail nothing could reach: the
/// model could be handed a line number and have no way to go to it. The
/// window is taken around the offset instead, so the cap costs a large file
/// more calls and never costs it a region.
const READ_SHARE: f32 = 0.32;
/// Below this a read stops being able to answer with a region of a file.
const MIN_READ_BYTES: usize = 8 * 1024;
/// The size this cap was before it was a share, and still the most any single
/// read answers with however much room the window has.
const MAX_READ_BYTES: usize = 2 * 1024 * 1024;

/// Lines returned when the caller does not ask for a range.
///
/// A file read is usually the largest thing a turn puts into the context
/// window, and it stays there for every later iteration of that turn. Returning
/// a window by default makes the common case — a long file the model needs one
/// region of — cost what the region costs rather than what the file costs.
///
/// How large that window should be is a fact about the model, not about files,
/// so it is a share of the window against a source line costing about forty
/// bytes. At [`OutputBudget::ANCHOR_WINDOW`] it is the 2000 lines it was.
const READ_LINES_SHARE: f32 = 0.10;
const LINE_BYTES: usize = 40;
/// Fewer lines than this and a default read cannot show a function with its
/// imports, which is the thing it is for.
const MIN_READ_LINES: usize = 200;
/// More than this and the model asked for a file rather than a region, and
/// should say so with an explicit `limit`.
const MAX_READ_LINES: usize = 10_000;

/// How many lines an unqualified `read_file` answers with.
///
/// A function rather than a constant because the number is a fact about the
/// model, and the caller that has to know it in a test is the same caller that
/// has to know it here.
fn default_read_lines(budget: OutputBudget) -> usize {
    budget.count(READ_LINES_SHARE, LINE_BYTES, MIN_READ_LINES, MAX_READ_LINES)
}

/// The most one `read_file` call answers with, whatever range it was asked for.
fn read_answer_cap(budget: OutputBudget) -> usize {
    budget.bytes(READ_SHARE, MIN_READ_BYTES, MAX_READ_BYTES)
}

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

        if bytes.is_empty() {
            return Ok(format!("{} is empty.", ctx.display(&path)).into());
        }

        let start = input.offset.unwrap_or(1).max(1) - 1;
        let limit = input.limit.unwrap_or(default_read_lines(ctx.budget)).max(1);
        let answer_cap = read_answer_cap(ctx.budget);
        // Located in the bytes rather than by decoding the file: only the
        // window is turned into text, so a read near the end of something
        // large costs the window instead of the file.
        let (window, total) = line_window(&bytes, start, limit);

        // An offset past the end is a mistake worth naming, not an empty
        // result: the model asked for a region that does not exist and needs
        // the file's length to correct itself. That length is now always
        // known, because finding the window counts every line on the way.
        let Some((from, to)) = window else {
            return Err(ToolError::InvalidInput(format!(
                "{} has {} lines; offset {} is past the end",
                ctx.display(&path),
                total,
                start + 1
            )));
        };

        let text = String::from_utf8_lossy(&bytes[from..to]);
        let mut out = String::with_capacity((to - from) + (to - from) / 8);
        let mut shown = 0usize;
        let mut clipped = false;
        for line in text.lines() {
            if out.len() + line.len() + LINE_NUMBER_WIDTH > answer_cap {
                if shown == 0 {
                    // One line longer than the whole budget — a minified
                    // bundle, a JSON log written without newlines. Cutting it
                    // beats returning nothing, which would read as an empty
                    // file rather than as a line that did not fit.
                    let cut = floor_char_boundary(line, answer_cap);
                    out.push_str(&format!("{:>5}\t{}\n", start + 1, &line[..cut]));
                    shown = 1;
                }
                clipped = true;
                break;
            }
            // Numbered by absolute position, not by position in the window, so
            // a line number from a windowed read still means what it says.
            out.push_str(&format!("{:>5}\t{line}\n", start + shown + 1));
            shown += 1;
        }

        let last = start + shown;
        if clipped || start > 0 || last < total {
            out.push_str(&range_note(start + 1, last, total, clipped, answer_cap));
        }
        Ok(out.into())
    }
}

/// Tells the model what it just got and how to get the rest.
///
/// Stated every time the answer is partial, because a window that does not say
/// it is a window is indistinguishable from a short file, and a model that
/// believes it has read the whole thing will act on what is missing.
fn range_note(
    first: usize,
    last: usize,
    available: usize,
    clipped: bool,
    answer_cap: usize,
) -> String {
    let mut note = format!("\n[showing lines {first}-{last} of {available}");
    if clipped {
        note.push_str(&format!(
            "; the window stopped early because a single read answers with at most {} KB",
            answer_cap / 1024
        ));
    }
    if last < available {
        note.push_str(&format!("; read again with offset {} for more", last + 1));
    }
    note.push_str("]\n");
    note
}

/// Room left for the number and tab each line is rendered with.
const LINE_NUMBER_WIDTH: usize = 8;

/// The byte range of `limit` lines starting at line `start`, and how many
/// lines the whole thing has.
///
/// `None` when `start` is past the end. Counting to the end regardless is what
/// lets an offset past it be answered with the file's real length rather than
/// with whatever a prefix happened to hold.
///
/// Lines are counted the way [`str::lines`] splits them, so a number from here
/// means the same thing as a number from a grep hit: on `\n`, with a final
/// line needing no terminator and a trailing one adding no empty line.
fn line_window(bytes: &[u8], start: usize, limit: usize) -> (Option<(usize, usize)>, usize) {
    let wanted_end = start.saturating_add(limit);
    let mut total = 0usize;
    let mut from: Option<usize> = None;
    let mut to: Option<usize> = None;
    let mut line_start = 0usize;

    for (i, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        if total == start {
            from = Some(line_start);
        }
        if total == wanted_end {
            to = Some(line_start);
        }
        total += 1;
        line_start = i + 1;
    }
    // A last line that the file did not terminate.
    if line_start < bytes.len() {
        if total == start {
            from = Some(line_start);
        }
        if total == wanted_end {
            to = Some(line_start);
        }
        total += 1;
    }

    (from.map(|f| (f, to.unwrap_or(bytes.len()))), total)
}

/// The largest index at or below `at` that `str` may be split on.
fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
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
    NotFound(Miss),
    Ambiguous(usize),
}

/// What the file can say about why the text was not there.
///
/// "Not found, read it again" is true and costs a round trip to act on, and
/// the reread usually ends in the same call with one space changed. The file
/// already holds the answer at the moment the match fails, so this works it
/// out then and puts it in the error.
enum Miss {
    /// The file holds text that is nearly `old_string`.
    Near(NearMiss),
    /// `new_string` is in the file already, and nothing resembles
    /// `old_string` — the shape of an edit being made twice.
    AlreadyApplied,
    /// Nothing to say beyond that it is not there.
    Nothing,
}

/// The stretch of the file that came closest, in the file's own bytes.
struct NearMiss {
    /// 1-based, inclusive, as the file's own line numbers.
    first: usize,
    last: usize,
    /// The file's text for those lines, unnumbered so it can be copied
    /// straight back into `old_string`. Cut at [`MAX_NEAR_MISS_LINES`], in
    /// which case `shown` is short of `last - first + 1`.
    text: String,
    shown: usize,
    /// Every line of `old_string` matched, once the indentation was set aside.
    whitespace_only: bool,
    /// How many places tied for closest. More than one and none of them can be
    /// pointed at as *the* answer.
    ties: usize,
}

/// How much of the file a near miss may quote.
///
/// Enough to hold a small function, and short of the point where the error is
/// itself the reread it exists to save.
const MAX_NEAR_MISS_LINES: usize = 12;

impl EditProblem {
    fn explain(self, display: &str) -> ToolError {
        ToolError::InvalidInput(match self {
            Self::NotFound(Miss::Nothing) => format!(
                "old_string was not found in {display}. Read the file again and match its exact \
                 current text, including whitespace."
            ),
            Self::NotFound(Miss::AlreadyApplied) => format!(
                "old_string was not found in {display}, but new_string is already there — this \
                 edit looks like it has been made once already. Read the file before making it \
                 again."
            ),
            Self::NotFound(Miss::Near(near)) => near.explain(display),
            Self::Ambiguous(n) => format!(
                "old_string appears {n} times in {display}. Include surrounding context to make \
                 it unique, or set replace_all."
            ),
        })
    }
}

impl NearMiss {
    fn explain(&self, display: &str) -> String {
        let (where_, matches, is) = if self.first == self.last {
            (format!("Line {}", self.first), "matches", "is")
        } else {
            (
                format!("Lines {}-{}", self.first, self.last),
                "match",
                "are",
            )
        };
        let how = if self.whitespace_only {
            format!("{matches} it apart from whitespace")
        } else {
            format!("{is} the closest text in the file")
        };
        let mut message = format!("old_string was not found in {display}. {where_} {how}");
        if self.ties > 1 {
            message.push_str(&format!(
                ", and {} other places match it equally well, so include more \
                 surrounding lines to say which one you mean",
                self.ties - 1
            ));
        }
        message.push_str(". Copy this text exactly:\n\n--- ");
        message.push_str(display);
        message.push_str(", as it stands ---\n");
        message.push_str(&self.text);
        message.push_str("\n--- end ---");
        if self.shown < self.last - self.first + 1 {
            message.push_str(&format!(
                "\n\n[the first {} of those lines; read {display} from line {} for the rest]",
                self.shown,
                self.first + self.shown
            ));
        }
        message
    }
}

/// Finds the stretch of `original` that came closest to `old`.
///
/// Compared with the indentation set aside, because that is what the misses
/// are: a model reproducing a line it read correctly and its leading spaces
/// approximately. The anchor is `old`'s first line with anything on it, and a
/// candidate's score is how many lines from there keep matching — so a
/// one-line miss and a twenty-line one are both found, and the twenty-line one
/// reports where the two stopped agreeing.
fn near_miss(original: &str, old: &str) -> Option<NearMiss> {
    let want: Vec<&str> = old.lines().collect();
    let have: Vec<&str> = original.lines().collect();
    let (anchor_at, anchor) = want
        .iter()
        .enumerate()
        .find(|(_, l)| !l.trim().is_empty())?;

    let mut best = 0usize;
    let mut best_start = 0usize;
    let mut ties = 0usize;
    for (i, line) in have.iter().enumerate() {
        if line.trim() != anchor.trim() {
            continue;
        }
        let Some(start) = i.checked_sub(anchor_at) else {
            continue;
        };
        let score = want
            .iter()
            .zip(have[start..].iter())
            .take_while(|(w, h)| w.trim() == h.trim())
            .count();
        if score > best {
            best = score;
            best_start = start;
            ties = 1;
        } else if score == best {
            ties += 1;
        }
    }
    if best == 0 {
        return None;
    }

    // The whole of what `old_string` asked for, so the model sees the lines it
    // got wrong and not only the ones it got right.
    let last = (best_start + want.len()).min(have.len());
    let shown = (last - best_start).min(MAX_NEAR_MISS_LINES);
    Some(NearMiss {
        first: best_start + 1,
        last,
        text: have[best_start..best_start + shown].join("\n"),
        shown,
        whitespace_only: best == want.len(),
        ties,
    })
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
        0 => Err(EditProblem::NotFound(miss(original, &old, &new))),
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

/// What to tell the model when `old_string` is not in the file.
///
/// A near miss first: it carries the file's own text, which answers the
/// already-applied case as well by showing what is there instead. The second
/// branch is for when nothing resembles `old_string` at all — the shape left
/// behind when an edit has already replaced it outright.
fn miss(original: &str, old: &str, new: &str) -> Miss {
    if let Some(near) = near_miss(original, old) {
        return Miss::Near(near);
    }
    if !new.trim().is_empty() && original.contains(new) {
        return Miss::AlreadyApplied;
    }
    Miss::Nothing
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

    /// The window the answer has to fit in is the model's, not this file's.
    ///
    /// Both directions matter. The 2000 lines this returned unconditionally is
    /// about 20,000 tokens — more than an 8k local model holds at all, so the
    /// answer could not be obeyed and survive the request carrying it — and two
    /// percent of a million-token window, where the model pages through a file
    /// it could have been handed.
    #[tokio::test]
    async fn the_default_window_follows_the_model_it_has_to_fit() {
        let (ctx, dir) = test_ctx();
        lines_file(dir.path(), "big.txt", 12_000);

        let small = ReadFile
            .execute(
                serde_json::json!({"path": "big.txt"}),
                &ctx.clone().with_budget(OutputBudget::for_window(8_192)),
            )
            .await
            .unwrap();
        let large = ReadFile
            .execute(
                serde_json::json!({"path": "big.txt"}),
                &ctx.clone().with_budget(OutputBudget::for_window(1_000_000)),
            )
            .await
            .unwrap();

        let lines = |out: &taurus_provider::ToolOutput| out.to_text().lines().count();
        assert!(
            lines(&small) < lines(&large),
            "a small window got as much as a large one: {} vs {}",
            lines(&small),
            lines(&large)
        );
        // Both still say what they left out, which is the property no window
        // size is allowed to cost.
        assert!(
            small.to_text().contains("read again with offset"),
            "{small}"
        );
        assert!(
            large.to_text().contains("read again with offset"),
            "{large}"
        );
        // And a tiny window is still handed something worth having.
        assert!(
            lines(&small) >= MIN_READ_LINES,
            "{} lines is below the floor",
            lines(&small)
        );
    }

    #[tokio::test]
    async fn a_long_file_stops_at_the_default_window_and_says_how_to_continue() {
        let (ctx, dir) = test_ctx();
        let default_lines = default_read_lines(OutputBudget::unknown());
        lines_file(dir.path(), "big.txt", default_lines + 50);
        let out = ReadFile
            .execute(serde_json::json!({"path": "big.txt"}), &ctx)
            .await
            .unwrap();
        assert!(out
            .to_text()
            .contains(&format!("{:>5}\tline {}", default_lines, default_lines)));
        assert!(!out
            .to_text()
            .contains(&format!("line {}", default_lines + 1)));
        assert!(
            out.to_text()
                .contains(&format!("read again with offset {}", default_lines + 1)),
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

    /// The reason the window follows the offset. This file is comfortably past
    /// the 256 KB a read answers with, and the region asked for is past it
    /// too — which used to be unreachable, so a model handed a line number
    /// from a grep hit had no way to go and look at it.
    #[tokio::test]
    async fn a_region_past_the_read_limit_opens_at_the_offset_it_was_given() {
        let (ctx, dir) = test_ctx();
        lines_file(dir.path(), "big.txt", 40_000);
        let out = ReadFile
            .execute(
                serde_json::json!({"path": "big.txt", "offset": 39_000, "limit": 2}),
                &ctx,
            )
            .await
            .unwrap();
        let text = out.to_text();
        assert!(text.contains("39000\tline 39000"), "{text}");
        assert!(text.contains("39001\tline 39001"), "{text}");
        assert!(
            text.contains("showing lines 39000-39001 of 40000"),
            "{text}"
        );
    }

    /// And the length in that sentence is the file's, which it now always is:
    /// finding the window counts every line on the way past it.
    #[tokio::test]
    async fn an_offset_past_a_large_file_reports_its_real_length() {
        let (ctx, dir) = test_ctx();
        lines_file(dir.path(), "big.txt", 40_000);
        let err = ReadFile
            .execute(
                serde_json::json!({"path": "big.txt", "offset": 50_000}),
                &ctx,
            )
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("has 40000 lines"), "{message}");
        // The old wording sent the model to grep because the tail was out of
        // reach. It is not, so nothing here should say so.
        assert!(!message.contains("grep"), "{message}");
    }

    /// The cap did not go away, it moved: it bounds the answer rather than the
    /// file, so a window too large to return stops early and says where to
    /// pick it up.
    #[tokio::test]
    async fn a_window_larger_than_one_read_stops_early_and_says_how_to_continue() {
        let (ctx, dir) = test_ctx();
        lines_file(dir.path(), "big.txt", 40_000);
        let out = ReadFile
            .execute(
                serde_json::json!({"path": "big.txt", "offset": 1, "limit": 40_000}),
                &ctx,
            )
            .await
            .unwrap();
        let text = out.to_text();
        let cap = read_answer_cap(OutputBudget::unknown());
        assert!(text.len() <= cap + 200, "{} bytes", text.len());
        assert!(text.contains("the window stopped early"), "{text}");
        assert!(text.contains("read again with offset"), "{text}");
    }

    /// A minified bundle is one line and no window can be smaller. Returning
    /// nothing would read as an empty file rather than as a line that did not
    /// fit.
    #[tokio::test]
    async fn a_single_line_too_long_to_return_comes_back_cut_rather_than_empty() {
        let (ctx, dir) = test_ctx();
        let body = format!(
            "{}\nsecond line\n",
            "x".repeat(read_answer_cap(OutputBudget::unknown()) * 2)
        );
        std::fs::write(dir.path().join("min.js"), body).unwrap();
        let out = ReadFile
            .execute(serde_json::json!({"path": "min.js"}), &ctx)
            .await
            .unwrap();
        let text = out.to_text();
        assert!(text.starts_with("    1\txxx"), "{}", &text[..40]);
        assert!(
            text.contains("showing lines 1-1 of 2"),
            "{}",
            &text[text.len() - 200..]
        );
        assert!(
            text.contains("the window stopped early"),
            "{}",
            &text[text.len() - 200..]
        );
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
    async fn edit_file_points_at_text_that_differs_only_in_whitespace() {
        let (ctx, dir) = test_ctx();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn main() {\n        let x = 1;\n}\n",
        )
        .unwrap();
        let err = EditFile
            .execute(
                serde_json::json!({
                    "path": "a.rs",
                    // The model's copy has four spaces where the file has eight.
                    "old_string": "fn main() {\n    let x = 1;\n}",
                    "new_string": "fn main() {\n    let x = 2;\n}",
                }),
                &ctx,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Lines 1-3"), "{err}");
        assert!(err.contains("apart from whitespace"), "{err}");
        // Quoted unnumbered, so it can go straight back into old_string.
        assert!(err.contains("\n        let x = 1;\n"), "{err}");
    }

    #[tokio::test]
    async fn edit_file_shows_where_a_partial_match_diverges() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.rs"), "fn main() {\n    let x = 9;\n}\n").unwrap();
        let err = EditFile
            .execute(
                serde_json::json!({
                    "path": "a.rs",
                    "old_string": "fn main() {\n    let x = 1;\n}",
                    "new_string": "fn main() {}",
                }),
                &ctx,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("closest text in the file"), "{err}");
        assert!(err.contains("let x = 9;"), "{err}");
    }

    #[tokio::test]
    async fn edit_file_says_when_the_edit_looks_already_made() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("a.txt"), "after\n").unwrap();
        let err = EditFile
            .execute(
                serde_json::json!({
                    "path": "a.txt",
                    "old_string": "before",
                    "new_string": "after",
                }),
                &ctx,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("already"), "{err}");
    }

    #[tokio::test]
    async fn edit_file_names_the_other_places_that_match_equally_well() {
        let (ctx, dir) = test_ctx();
        // Tabs in the file, spaces in the model's copy: not a literal match
        // anywhere, and an equally good one in three places.
        std::fs::write(dir.path().join("a.txt"), "\tcall()\n\tcall()\n\tcall()\n").unwrap();
        let err = EditFile
            .execute(
                serde_json::json!({
                    "path": "a.txt",
                    "old_string": "    call()",
                    "new_string": "    call(1)",
                }),
                &ctx,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("2 other places"), "{err}");
    }

    #[tokio::test]
    async fn edit_file_caps_how_much_of_the_file_it_quotes() {
        let (ctx, dir) = test_ctx();
        let file: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.path().join("a.txt"), file).unwrap();
        let wanted: String = (1..=40)
            .map(|i| format!("  line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let err = EditFile
            .execute(
                serde_json::json!({"path": "a.txt", "old_string": wanted, "new_string": "x"}),
                &ctx,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("the first 12 of those lines"), "{err}");
        assert!(err.contains("from line 13"), "{err}");
        assert!(!err.contains("line 20"), "{err}");
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
