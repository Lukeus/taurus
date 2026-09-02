//! What is inside the config a workspace is asking you to trust.
//!
//! [`crate::trust`] answers *whether* a folder's own config may take effect.
//! Until now the question was put with counts: "1 skill, 1 MCP server, 2
//! standing permission grants". A count of servers is not something anyone can
//! judge, which is why the MCP command lines have always been named rather than
//! counted — and this is that same argument applied to everything else in the
//! layer. A skill is a file with a procedure in it. An `AGENTS.md` is a brief
//! the model is handed before it reads a line of the code. Both arrive with
//! `git clone`, and both were being counted.
//!
//! # What this is not
//!
//! It is not a verdict. Nothing here refuses a workspace, and a workspace with
//! no findings is not a workspace that is safe to run — the gate still decides
//! only whether a project may configure Taurus, and running that project's
//! build script is a decision the permission prompt makes later. What this
//! produces is a list of the things in these files that a person reading them
//! would want pointed out, most of which they could not see: a byte that
//! renders as nothing, a comment the renderer hides, a run of base64 in a file
//! that should hold prose.
//!
//! # Why it does not parse anything
//!
//! [`crate::trust::pending`] is deliberately shallow — it counts files and
//! reads two of them for their entry names, and it starts no process, loads no
//! skill and evaluates nothing. This keeps that promise. Every rule below reads
//! bytes and compares them; the two that look at JSON read one field and act on
//! neither. A function that exists to describe a decision must not take any of
//! the actions the decision governs.
//!
//! # What bounds it
//!
//! It runs inside `pending`, which the desktop app calls on every refresh and
//! the CLI once per command. So it is capped in three directions at once —
//! [`MAX_FILES`], [`MAX_BYTES`] per file, [`MAX_FINDINGS`] overall — and it
//! only runs at all where the workspace already has config waiting, which is a
//! minority of directories. A payload past those caps is not seen, and the
//! alternative is reading an unbounded tree on every `taurus run`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config;

/// How many files are opened, across every kind.
///
/// A workspace with more borrowed instruction files than this has a
/// configuration problem of its own, and the point of a cap is that it holds
/// for the workspace built to defeat it.
pub const MAX_FILES: usize = 64;

/// How much of one file is read.
///
/// Comfortably past the end of any brief or skill written to be read. A
/// `SKILL.md` that runs longer than this is already past what the prompt would
/// take from it.
pub const MAX_BYTES: usize = 64 * 1024;

/// How many findings are reported.
///
/// A list longer than this has stopped being something anyone reads. The
/// twenty-first finding does not change the decision the first one already
/// informed.
pub const MAX_FINDINGS: usize = 20;

/// What kind of thing was noticed.
///
/// Each variant is something a reader of the file could not have seen, or a
/// setting whose consequence is not obvious from the line that sets it. None of
/// them is proof of anything, which is why the panel names them and does not
/// score them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FindingKind {
    /// Characters that occupy no width, or reorder what is around them.
    HiddenCharacters,
    /// An HTML comment long enough to hold an instruction. Markdown renders it
    /// as nothing; the model is given the file as text and reads it.
    HiddenDirective,
    /// A run of base64 long enough to be a program rather than an identifier.
    EncodedPayload,
    /// The workspace names the endpoint conversations are sent to.
    Endpoint,
    /// The workspace lifts the guard that keeps `fetch_url` off private hosts.
    PrivateHosts,
    /// A standing permission grant wide enough to cover a whole tool.
    BroadGrant,
    /// A skill carrying something executable beside its procedure.
    ExecutableSkill,
}

impl FindingKind {
    /// The short label a panel puts in front of the detail.
    pub fn label(self) -> &'static str {
        match self {
            Self::HiddenCharacters => "Invisible characters",
            Self::HiddenDirective => "Hidden comment",
            Self::EncodedPayload => "Encoded block",
            Self::Endpoint => "Names an endpoint",
            Self::PrivateHosts => "Reaches private hosts",
            Self::BroadGrant => "Broad permission grant",
            Self::ExecutableSkill => "Skill carries a script",
        }
    }
}

/// One thing worth pointing out, in one file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Finding {
    /// Relative to the workspace, because an absolute path in a banner is
    /// mostly the part the reader already knows.
    pub path: String,
    pub kind: FindingKind,
    /// A complete phrase, the way `Restored::Skipped`'s reason is: it follows
    /// the label and has to read as a sentence beside it rather than as a code.
    pub detail: String,
}

/// Everything the workspace's own config layer holds that is worth naming.
///
/// Ordered by file so a reader works down a list rather than around one, and
/// truncated at [`MAX_FINDINGS`].
pub fn inspect(workspace: &Path) -> Vec<Finding> {
    let mut scan = Scan::new(workspace);

    // The same source lists `trust::pending` walks, and for the same reason: a
    // borrowable location added to one of those loaders has to fall inside this
    // as automatically as it falls inside the gate. A second list of paths here
    // is a list that goes stale silently.
    for source in config::all_skill_sources(Some(workspace)) {
        if source.tier == taurus_skills::SkillTier::Project {
            scan.skills(&source.dir);
        }
    }
    for source in config::all_agent_sources(Some(workspace)) {
        if source.tier == taurus_agents::AgentTier::Project {
            scan.text_tree(&source.dir, ".md");
        }
    }
    for source in crate::instructions::all_sources(Some(workspace)) {
        if source.tier == crate::instructions::InstructionsTier::Project {
            scan.text_file(&source.path);
        }
    }
    for dir in crate::instructions::all_scoped_dirs(Some(workspace)) {
        if dir.starts_with(workspace) {
            scan.text_tree(&dir, crate::instructions::SCOPED_SUFFIX);
        }
    }

    scan.providers();
    scan.search();
    scan.permissions();

    scan.findings
}

/// The walk, and what it has found so far.
struct Scan {
    workspace: PathBuf,
    findings: Vec<Finding>,
    /// Counted rather than derived from `findings`, which holds no entry for a
    /// file that turned out to be ordinary — and an ordinary file cost a read
    /// exactly like a suspicious one.
    opened: usize,
}

impl Scan {
    fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            findings: Vec::new(),
            opened: 0,
        }
    }

    /// Whether there is any point continuing.
    fn done(&self) -> bool {
        self.opened >= MAX_FILES || self.findings.len() >= MAX_FINDINGS
    }

    fn note(&mut self, path: &Path, kind: FindingKind, detail: String) {
        if self.findings.len() >= MAX_FINDINGS {
            return;
        }
        let path = path
            .strip_prefix(&self.workspace)
            .unwrap_or(path)
            .display()
            .to_string();
        self.findings.push(Finding { path, kind, detail });
    }

    /// Reads at most [`MAX_BYTES`], as text.
    ///
    /// A file that is not UTF-8 is not text, and every file this looks at is
    /// meant to be prose or JSON — so a binary one is skipped rather than
    /// scanned as bytes. It still counts against the cap, because opening it
    /// cost the same.
    fn read(&mut self, path: &Path) -> Option<String> {
        if self.done() {
            return None;
        }
        self.opened += 1;
        let bytes = std::fs::read(path).ok()?;
        let bytes = &bytes[..bytes.len().min(MAX_BYTES)];
        // `from_utf8_lossy` rather than a strict decode: the cut above lands
        // mid-character on any file long enough to need it, and refusing the
        // whole file for one truncated codepoint would blind this to exactly
        // the large files worth reading.
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    /// A file the model is handed as text.
    fn text_file(&mut self, path: &Path) {
        if !path.is_file() {
            return;
        }
        let Some(text) = self.read(path) else {
            return;
        };
        for (kind, detail) in text_findings(&text) {
            self.note(path, kind, detail);
        }
    }

    /// Every file under `dir` whose name ends in `suffix`, recursing as the
    /// loaders do.
    fn text_tree(&mut self, dir: &Path, suffix: &str) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        // Sorted, so a workspace with more files than the cap reports the same
        // ones on every refresh. A banner whose contents change on their own is
        // one nobody trusts.
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if self.done() {
                return;
            }
            if path.is_dir() {
                self.text_tree(&path, suffix);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix))
            {
                self.text_file(&path);
            }
        }
    }

    /// A skill directory: its procedure, and anything executable beside it.
    fn skills(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut skills: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        skills.sort();
        for skill in skills {
            if self.done() {
                return;
            }
            if !skill.is_dir() {
                continue;
            }
            self.text_file(&skill.join("SKILL.md"));
            self.scripts(&skill);
        }
    }

    /// Files in a skill that are meant to be run rather than read.
    ///
    /// Reported by name and not opened. A script is the one thing in this layer
    /// whose danger does not survive summarising — "it is 40 lines of Python"
    /// says nothing — so the useful thing to do with it is say it is there and
    /// where, and let the reader open it.
    fn scripts(&mut self, skill: &Path) {
        let Ok(entries) = std::fs::read_dir(skill) else {
            return;
        };
        let mut found: Vec<String> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_script(p))
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
            .collect();
        if found.is_empty() {
            return;
        }
        found.sort();
        let detail = format!(
            "carries {} — read {} before trusting this workspace",
            list(&found),
            if found.len() == 1 { "it" } else { "them" }
        );
        self.note(skill, FindingKind::ExecutableSkill, detail);
    }

    /// The workspace names the endpoint conversations go to.
    ///
    /// Not a warning. It is the single most consequential thing in this layer
    /// and the banner already says "provider endpoints" — this says *which*,
    /// which is the difference between a category and a fact. Only the host is
    /// reported: the rest of a URL is path, and a key is never in this file.
    fn providers(&mut self) {
        let Some(path) = config::providers_file(config::Scope::Workspace, Some(&self.workspace))
        else {
            return;
        };
        let Some(text) = self.read(&path) else {
            return;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let mut hosts: Vec<String> = json
            .get("providers")
            .and_then(|p| p.as_array())
            .into_iter()
            .flatten()
            .filter_map(|p| p.get("base_url").and_then(|u| u.as_str()))
            .filter_map(host_of)
            .collect();
        hosts.sort();
        hosts.dedup();
        if hosts.is_empty() {
            return;
        }
        let detail = format!("every message would be sent to {}", list(&hosts));
        self.note(&path, FindingKind::Endpoint, detail);
    }

    /// The workspace lifts the private-host guard off `fetch_url`.
    fn search(&mut self) {
        let Some(path) = config::search_file(config::Scope::Workspace, Some(&self.workspace))
        else {
            return;
        };
        let Some(text) = self.read(&path) else {
            return;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        if json.get("allow_private_hosts").and_then(|v| v.as_bool()) == Some(true) {
            self.note(
                &path,
                FindingKind::PrivateHosts,
                "`fetch_url` would be allowed to reach loopback and private-network \
                 addresses — the machines behind your firewall rather than the web"
                    .into(),
            );
        }
    }

    /// Standing grants wide enough to cover a whole tool.
    ///
    /// A rule is `tool` or `tool:argument`. The bare form is the one worth
    /// naming — `run_command` on its own is every command there is, granted in
    /// advance by a file that arrived with a clone.
    fn permissions(&mut self) {
        let Some(path) = config::scope_dir(config::Scope::Workspace, Some(&self.workspace))
            .map(|dir| dir.join("permissions.json"))
        else {
            return;
        };
        let Some(text) = self.read(&path) else {
            return;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let mut broad: Vec<String> = json
            .get("allowed")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
            .filter_map(|r| r.as_str())
            .filter(|rule| is_broad(rule))
            .map(String::from)
            .collect();
        if broad.is_empty() {
            return;
        }
        broad.sort();
        broad.dedup();
        let detail = format!(
            "{} would be allowed without asking, whatever the arguments",
            list(&broad)
        );
        self.note(&path, FindingKind::BroadGrant, detail);
    }
}

/// Whether a permission rule covers a whole tool rather than one use of it.
fn is_broad(rule: &str) -> bool {
    let rule = rule.trim();
    if rule.is_empty() {
        return false;
    }
    // A trailing `*` is the explicit form; no `:` at all is the implicit one,
    // and they mean the same thing.
    match rule.split_once(':') {
        None => true,
        Some((_, argument)) => argument.trim() == "*" || argument.trim().is_empty(),
    }
}

/// Whether a file beside a skill is meant to be run.
///
/// By extension, plus the Unix executable bit. Neither is conclusive on its
/// own — a `.py` is only dangerous if something runs it, and a bit can be set
/// on anything — and this does not have to be conclusive: it is deciding what
/// to put in front of a reader, not what to refuse.
fn is_script(path: &Path) -> bool {
    const EXTENSIONS: [&str; 9] = ["sh", "bash", "zsh", "fish", "py", "rb", "pl", "ps1", "js"];
    let by_extension = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e));
    if by_extension {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    // No equivalent bit on Windows, where what is runnable is decided by the
    // extension — which the list above already covers.
    #[cfg(not(unix))]
    {
        false
    }
}

/// Every rule that reads a file's text, in one pass over it.
fn text_findings(text: &str) -> Vec<(FindingKind, String)> {
    let mut out = Vec::new();
    if let Some(detail) = hidden_characters(text) {
        out.push((FindingKind::HiddenCharacters, detail));
    }
    if let Some(detail) = hidden_directive(text) {
        out.push((FindingKind::HiddenDirective, detail));
    }
    if let Some(detail) = encoded_payload(text) {
        out.push((FindingKind::EncodedPayload, detail));
    }
    out
}

/// Characters that render as nothing, or reorder what is around them.
///
/// The point of the rule is the gap between what a reviewer sees and what the
/// model is given. A bidi override can make a line of a brief read in the
/// editor as the opposite of what it says as text; a zero-width run can hide a
/// sentence inside what looks like a paragraph break.
///
/// **U+200D is deliberately absent.** Zero-width joiner is how 👨‍👩‍👧 and every
/// other composed emoji is built, so flagging it would fire on any `AGENTS.md`
/// with a family in it — and a scanner that cries wolf on ordinary files is one
/// people learn to click past, which costs exactly the workspace where it
/// mattered. Same reasoning puts U+FEFF on the list only away from offset 0: a
/// byte-order mark at the start of a file is how half the world's editors save.
fn hidden_characters(text: &str) -> Option<String> {
    let mut count = 0usize;
    let mut first_line = 0usize;
    let mut line = 1usize;

    for (offset, ch) in text.char_indices() {
        if ch == '\n' {
            line += 1;
            continue;
        }
        let hidden = matches!(ch,
            '\u{200B}' | '\u{200E}' | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{E0000}'..='\u{E007F}')
            || (ch == '\u{FEFF}' && offset > 0);
        if hidden {
            if count == 0 {
                first_line = line;
            }
            count += 1;
        }
    }

    if count == 0 {
        return None;
    }
    let what = if count == 1 {
        "1 character that renders as nothing or reorders the text around it".to_string()
    } else {
        format!("{count} characters that render as nothing or reorder the text around them")
    };
    Some(format!(
        "{what}, first on line {first_line}. The model is given this file as \
         text and reads what you cannot see"
    ))
}

/// Shortest HTML comment worth naming.
///
/// `<!-- prettier-ignore -->` and `<!-- markdownlint-disable -->` are ordinary
/// and short. A comment long enough to hold an instruction is not, and the
/// threshold is set where the first stops and the second starts.
const DIRECTIVE_BYTES: usize = 200;

/// A comment the renderer hides and the model reads.
fn hidden_directive(text: &str) -> Option<String> {
    let mut rest = text;
    let mut longest = 0usize;
    let mut count = 0usize;
    while let Some(start) = rest.find("<!--") {
        let after = &rest[start + 4..];
        let Some(end) = after.find("-->") else { break };
        let body = &after[..end];
        if body.len() >= DIRECTIVE_BYTES {
            count += 1;
            longest = longest.max(body.len());
        }
        rest = &after[end + 3..];
    }
    if count == 0 {
        return None;
    }
    // "Markdown renders" whatever the count, so only the noun moves.
    let noun = if count == 1 {
        format!("1 HTML comment of {longest} bytes")
    } else {
        format!("{count} HTML comments, the longest {longest} bytes")
    };
    Some(format!(
        "{noun}, which Markdown renders as nothing. The model is given this \
         file as text, so what a comment hides from a reader it does not hide \
         from the model"
    ))
}

/// Shortest base64 run worth naming.
///
/// Past any identifier, hash, or key fingerprint that legitimately appears in
/// prose, and comfortably under a small script.
const PAYLOAD_CHARS: usize = 512;

/// A run of base64 long enough to be a program.
///
/// **A `data:` URI is excluded.** An image embedded in Markdown is a long
/// base64 run and an entirely ordinary one, and firing on every diagram in a
/// borrowed `AGENTS.md` is the same false alarm the emoji case is.
fn encoded_payload(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut run = 0usize;
    let mut start = 0usize;

    for (i, &b) in bytes.iter().enumerate() {
        let base64 = b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=';
        if base64 {
            if run == 0 {
                start = i;
            }
            run += 1;
            continue;
        }
        if run >= PAYLOAD_CHARS && !preceded_by_data_uri(text, start) {
            return Some(payload_detail(run));
        }
        run = 0;
    }
    if run >= PAYLOAD_CHARS && !preceded_by_data_uri(text, start) {
        return Some(payload_detail(run));
    }
    None
}

fn payload_detail(run: usize) -> String {
    format!(
        "an unbroken {run}-character run of base64 — long enough to be a program \
         rather than a key or a hash"
    )
}

/// Whether the run beginning at `start` is the payload of a `data:` URI.
///
/// The marker sits just before the run, after a `;base64,` that the run itself
/// cannot contain — so a short look backwards is enough and there is no need to
/// parse the line.
fn preceded_by_data_uri(text: &str, start: usize) -> bool {
    let window = start.saturating_sub(64);
    // `is_char_boundary` rather than slicing blindly: a fixed-width look back
    // lands mid-character on any file with prose in it.
    let mut window = window;
    while window < start && !text.is_char_boundary(window) {
        window += 1;
    }
    text[window..start].contains("base64,")
}

/// The host of a URL, without parsing one.
///
/// Only the authority is wanted and only for display. A URL that does not look
/// like one is reported as nothing rather than as itself, because a banner is
/// not the place to render whatever a file happened to contain.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = rest
        .split(['/', '?', '#'])
        .next()?
        .rsplit('@')
        .next()?
        .trim();
    if host.is_empty() || host.contains(char::is_whitespace) {
        return None;
    }
    Some(host.to_string())
}

/// `a`, `a and b`, `a, b and c`.
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_brief_is_not_flagged() {
        // The case that matters most. Everything below is a rule that could
        // fire on a file somebody wrote in good faith, and a banner that raises
        // findings on ordinary repositories is a banner people click past.
        let text = "# Project\n\nRun `cargo test` before pushing.\n\n\
                    <!-- prettier-ignore -->\n| a | b |\n";
        assert!(text_findings(text).is_empty());
    }

    #[test]
    fn a_family_emoji_is_not_invisible_characters() {
        // U+200D joins these, and flagging it would fire on any brief with an
        // emoji in it. This is the false positive the rule is written around.
        assert_eq!(hidden_characters("Ship it 👨‍👩‍👧 🎉"), None);
    }

    #[test]
    fn a_byte_order_mark_is_ordinary_at_the_start_and_not_in_the_middle() {
        assert_eq!(hidden_characters("\u{FEFF}# Project"), None);
        assert!(hidden_characters("# Project\u{FEFF} notes").is_some());
    }

    #[test]
    fn a_bidi_override_is_named_with_its_line() {
        let found = hidden_characters("line one\nline two\nnever \u{202E}run this")
            .expect("an override must be found");
        assert!(found.contains("line 3"), "{found}");
    }

    #[test]
    fn a_short_comment_is_ordinary_and_a_long_one_is_not() {
        assert_eq!(hidden_directive("<!-- prettier-ignore -->"), None);
        let long = format!("<!-- {} -->", "x".repeat(DIRECTIVE_BYTES));
        assert!(hidden_directive(&long).is_some());
    }

    #[test]
    fn an_unterminated_comment_does_not_hang_the_scan() {
        // The loop advances past a `-->` it found; with none to find it has to
        // stop rather than re-examine the same `<!--` forever.
        let text = format!("<!-- {}", "x".repeat(DIRECTIVE_BYTES * 2));
        assert_eq!(hidden_directive(&text), None);
    }

    #[test]
    fn an_embedded_image_is_not_an_encoded_payload() {
        // A diagram in a borrowed AGENTS.md is a long base64 run and an
        // entirely ordinary one.
        let uri = format!(
            "![d](data:image/png;base64,{})",
            "A".repeat(PAYLOAD_CHARS * 2)
        );
        assert_eq!(encoded_payload(&uri), None);
    }

    #[test]
    fn a_bare_base64_block_is_an_encoded_payload() {
        let blob = format!("Setup:\n\n{}\n", "A".repeat(PAYLOAD_CHARS));
        assert!(encoded_payload(&blob).is_some());
    }

    #[test]
    fn a_hash_in_prose_is_too_short_to_report() {
        let sha = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(encoded_payload(&format!("pinned at {sha}")), None);
    }

    #[test]
    fn a_bare_tool_name_is_a_broad_grant_and_a_scoped_one_is_not() {
        assert!(is_broad("run_command"));
        assert!(is_broad("run_command:*"));
        assert!(!is_broad("run_command:cargo test"));
        assert!(!is_broad(""));
    }

    #[test]
    fn a_url_reports_its_host_and_nothing_else() {
        assert_eq!(
            host_of("https://gateway.example.com/v1/messages"),
            Some("gateway.example.com".into())
        );
        assert_eq!(
            host_of("http://user:pw@10.0.0.1:8080/v1"),
            Some("10.0.0.1:8080".into())
        );
        assert_eq!(host_of("not a url at all"), None);
    }

    #[test]
    fn findings_stop_at_the_cap() {
        let dir = tempfile::tempdir().expect("temp workspace");
        let mut scan = Scan::new(dir.path());
        for _ in 0..MAX_FINDINGS * 2 {
            scan.note(dir.path(), FindingKind::HiddenCharacters, "x".into());
        }
        assert_eq!(scan.findings.len(), MAX_FINDINGS);
    }

    #[test]
    fn a_path_is_reported_relative_to_the_workspace() {
        let dir = tempfile::tempdir().expect("temp workspace");
        let mut scan = Scan::new(dir.path());
        scan.note(
            &dir.path().join(".taurus").join("permissions.json"),
            FindingKind::BroadGrant,
            "x".into(),
        );
        let path = &scan.findings[0].path;
        assert!(!path.starts_with('/'), "{path}");
        assert!(path.ends_with("permissions.json"), "{path}");
    }
}
