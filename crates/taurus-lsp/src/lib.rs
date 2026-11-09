//! What the compiler says about a file the agent just wrote.
//!
//! A small model's edits fail in a particular way: each one is plausible, and
//! the third one in a row breaks the build in a way nothing tells it about
//! until it runs the tests, if it does. A language server already knows, about
//! a second after the write. [`Lsp`] asks it, and the file tools put the answer
//! in the result of the call that caused it, where the model reads it before
//! it makes the next edit.
//!
//! Deliberately narrow. This is diagnostics and nothing else: no completion,
//! no hover, no go-to-definition. Those are things an editor wants. An agent
//! that edits by text, reads by text and checks by running things needs to be
//! told when it has broken something, and that's the one question asked here.
//!
//! A server is found by a file's extension and started the first time a file
//! it covers is read or written. It's only ever a program on the PATH: nothing
//! is downloaded. Starting one runs the project's own code in some cases
//! (rust-analyzer runs build scripts and proc macros), so the host only builds
//! an [`Lsp`] for a workspace that's trusted.

mod client;
pub mod uri;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::watch;
use tracing::{info, warn};

use client::Server;

/// How long a check waits for more after the first publish. See
/// [`client::Server::check`].
const SETTLE: Duration = Duration::from_millis(400);

/// At most this many problems are listed for one file. A file that broke this
/// badly has one cause, nearly always near the top.
const MAX_LISTED: usize = 8;

/// A message is cut here. Compiler messages that run longer are explaining
/// the type system, not the mistake.
const MAX_MESSAGE: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// 1-based, as an editor shows it.
    pub line: u32,
    pub column: u32,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

/// A server, and the files it's for.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerSpec {
    /// What it's called in settings and in what the model reads.
    pub name: String,
    /// Command lines to try, in order. The first whose program is on the PATH
    /// is the one started.
    pub commands: Vec<Vec<String>>,
    /// Extension, without the dot, to the language id the protocol wants.
    pub languages: BTreeMap<String, String>,
    pub options: Option<Value>,
}

impl ServerSpec {
    fn new(name: &str, commands: &[&[&str]], languages: &[(&str, &str)]) -> Self {
        Self {
            name: name.into(),
            commands: commands
                .iter()
                .map(|c| c.iter().map(|s| s.to_string()).collect())
                .collect(),
            languages: languages
                .iter()
                .map(|(e, l)| (e.to_string(), l.to_string()))
                .collect(),
            options: None,
        }
    }

    /// A server someone configured by hand. Each extension's language id is
    /// `language_id` when given, and the extension itself when not, which is
    /// right for most languages (`go`, `rust` aside).
    pub fn custom(
        name: &str,
        command: Vec<String>,
        extensions: Vec<String>,
        language_id: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            commands: vec![command],
            languages: extensions
                .into_iter()
                .map(|e| {
                    let e = e.trim_start_matches('.').to_ascii_lowercase();
                    let id = language_id.clone().unwrap_or_else(|| e.clone());
                    (e, id)
                })
                .collect(),
            options: None,
        }
    }
}

/// The servers looked for when nothing is configured. Each is used only if
/// its program is already installed.
pub fn builtin_specs() -> Vec<ServerSpec> {
    vec![
        ServerSpec::new("rust-analyzer", &[&["rust-analyzer"]], &[("rs", "rust")]),
        ServerSpec::new(
            "typescript-language-server",
            &[&["typescript-language-server", "--stdio"]],
            &[
                ("ts", "typescript"),
                ("mts", "typescript"),
                ("cts", "typescript"),
                ("tsx", "typescriptreact"),
                ("js", "javascript"),
                ("mjs", "javascript"),
                ("cjs", "javascript"),
                ("jsx", "javascriptreact"),
            ],
        ),
        ServerSpec::new(
            "pyright",
            &[
                &["pyright-langserver", "--stdio"],
                &["basedpyright-langserver", "--stdio"],
            ],
            &[("py", "python"), ("pyi", "python")],
        ),
        ServerSpec::new("gopls", &[&["gopls"]], &[("go", "go")]),
        ServerSpec::new(
            "clangd",
            &[&["clangd"]],
            &[
                ("c", "c"),
                ("h", "c"),
                ("cc", "cpp"),
                ("cpp", "cpp"),
                ("cxx", "cpp"),
                ("hh", "cpp"),
                ("hpp", "cpp"),
            ],
        ),
    ]
}

/// What a check found out.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// No server covers this file, or it's outside the workspace.
    NotCovered,
    /// The server hasn't finished starting. The next check will find it.
    Starting { server: String },
    /// It couldn't be started, or it has exited.
    Unavailable { server: String, reason: String },
    /// It's running and published nothing about this file in time.
    NoAnswer { server: String },
    Checked {
        server: String,
        diagnostics: Vec<Diagnostic>,
    },
}

impl Outcome {
    /// What the model is told, appended to the tool's result. `None` when
    /// there's nothing worth the tokens.
    ///
    /// Errors are listed; warnings are counted. A model asked to fix a
    /// warning about an unused import while its edit doesn't compile will
    /// fix the import.
    pub fn render(&self, display: &str) -> Option<String> {
        match self {
            Outcome::NotCovered | Outcome::NoAnswer { .. } => None,
            Outcome::Starting { server } => Some(format!(
                "{server} is still starting, so this edit wasn't checked. Later edits will be."
            )),
            Outcome::Unavailable { server, reason } => Some(format!(
                "{server} isn't running ({reason}), so edits to files like this one aren't \
                 being checked."
            )),
            Outcome::Checked {
                server,
                diagnostics,
            } => {
                let errors: Vec<_> = diagnostics
                    .iter()
                    .filter(|d| d.severity == Severity::Error)
                    .collect();
                let warnings = diagnostics
                    .iter()
                    .filter(|d| d.severity == Severity::Warning)
                    .count();
                let warned = match warnings {
                    0 => String::new(),
                    1 => " and 1 warning".into(),
                    n => format!(" and {n} warnings"),
                };
                if errors.is_empty() {
                    return Some(format!("{server}: no errors in {display}{warned}."));
                }
                let mut out = format!(
                    "{server} reports {} in {display}{warned}:",
                    plural(errors.len(), "error")
                );
                for d in errors.iter().take(MAX_LISTED) {
                    out.push_str(&format!("\n- {}:{}: {}", d.line, d.column, one_line(d)));
                }
                if errors.len() > MAX_LISTED {
                    out.push_str(&format!("\n- …and {} more", errors.len() - MAX_LISTED));
                }
                Some(out)
            }
        }
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn one_line(d: &Diagnostic) -> String {
    let mut message: String = d.message.split_whitespace().collect::<Vec<_>>().join(" ");
    if message.chars().count() > MAX_MESSAGE {
        message = message.chars().take(MAX_MESSAGE).collect::<String>() + "…";
    }
    match &d.code {
        Some(code) => format!("{message} [{code}]"),
        None => message,
    }
}

#[derive(Clone)]
enum Slot {
    Starting,
    Ready(Arc<Server>),
    Failed(String),
}

/// The language servers for one workspace. See the crate docs.
pub struct Lsp {
    root: PathBuf,
    specs: Vec<ServerSpec>,
    budget: Duration,
    slots: Mutex<HashMap<String, watch::Receiver<Slot>>>,
    /// Servers whose failure the model has already been told about, so it's
    /// told once and not on every edit.
    reported: Mutex<HashSet<String>>,
}

impl Lsp {
    /// Servers for `root`, from `specs`, with `budget` the longest a check
    /// waits, a server's start included.
    pub fn new(root: &Path, specs: Vec<ServerSpec>, budget: Duration) -> Self {
        Self {
            root: dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
            specs,
            budget,
            slots: Mutex::default(),
            reported: Mutex::default(),
        }
    }

    /// The server for `path`, and the language id it goes by there.
    fn spec_for(&self, path: &Path) -> Option<(&ServerSpec, String)> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        self.specs
            .iter()
            .find_map(|s| s.languages.get(&ext).map(|id| (s, id.clone())))
    }

    /// The slot for `spec`, starting its server the first time it's asked
    /// for.
    fn slot(&self, spec: &ServerSpec) -> watch::Receiver<Slot> {
        let mut slots = self.slots.lock().unwrap();
        if let Some(slot) = slots.get(&spec.name) {
            return slot.clone();
        }
        let (tx, rx) = watch::channel(Slot::Starting);
        slots.insert(spec.name.clone(), rx.clone());
        let (spec, root) = (spec.clone(), self.root.clone());
        // Its own task, so a check that gives up waiting doesn't cancel the
        // start: the next check finds it ready.
        tokio::spawn(async move {
            let slot = match command_for(&spec) {
                None => Slot::Failed("not installed".into()),
                Some(command) => match Server::start(command, &root, spec.options.clone()).await {
                    Ok(server) => {
                        info!(server = spec.name, "language server started");
                        Slot::Ready(Arc::new(server))
                    }
                    Err(e) => {
                        warn!(server = spec.name, error = %e, "language server didn't start");
                        Slot::Failed(e)
                    }
                },
            };
            let _ = tx.send(slot);
        });
        rx
    }

    /// Starts the server for `path` if there is one, without waiting. Called
    /// when a file is read, so the server is up by the time it's edited.
    pub fn warm(&self, path: &Path) {
        if !path.starts_with(&self.root) {
            return;
        }
        if let Some((spec, _)) = self.spec_for(path) {
            if command_for(spec).is_some() {
                self.slot(spec);
            }
        }
    }

    /// What the server for `path` says about it, now that it holds `text`.
    pub async fn check(&self, path: &Path, text: &str) -> Outcome {
        let path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !path.starts_with(&self.root) {
            return Outcome::NotCovered;
        }
        let Some((spec, language_id)) = self.spec_for(&path) else {
            return Outcome::NotCovered;
        };
        let server = spec.name.clone();
        // A built-in that isn't installed is nobody's problem: not every
        // machine has clangd, and saying so on every C edit would be noise.
        if command_for(spec).is_none() {
            return Outcome::NotCovered;
        }
        // Set before anything waits, starting the server and any check
        // already running on it included: the budget is the longest this
        // write is held up, and a check that only counted its own time
        // would let five parallel writes wait five budgets.
        let deadline = std::time::Instant::now() + self.budget;
        let mut slot = self.slot(spec);
        // In one statement, so the borrow `wait_for` hands back is gone before
        // the next await: held across it, it would make this future unsendable.
        let ready = match tokio::time::timeout_at(
            deadline.into(),
            slot.wait_for(|s| !matches!(s, Slot::Starting)),
        )
        .await
        {
            Err(_) => return Outcome::Starting { server },
            Ok(Err(_)) => return self.unavailable(server, "it stopped starting".into()),
            Ok(Ok(state)) => state.clone(),
        };
        let handle = match ready {
            Slot::Ready(handle) if handle.is_alive() => handle,
            Slot::Ready(_) => return self.unavailable(server, "it exited".into()),
            Slot::Failed(reason) => return self.unavailable(server, reason),
            Slot::Starting => unreachable!("waited for"),
        };
        match handle
            .check(&path, &language_id, text, deadline, SETTLE)
            .await
        {
            Ok(Some(diagnostics)) => Outcome::Checked {
                server,
                diagnostics,
            },
            Ok(None) => Outcome::NoAnswer { server },
            Err(reason) => self.unavailable(server, reason),
        }
    }

    /// [`Outcome::Unavailable`] the first time for a server, and
    /// [`Outcome::NotCovered`] after that.
    fn unavailable(&self, server: String, reason: String) -> Outcome {
        if self.reported.lock().unwrap().insert(server.clone()) {
            Outcome::Unavailable { server, reason }
        } else {
            Outcome::NotCovered
        }
    }

    /// Ends every server this started.
    pub async fn shutdown(&self) {
        let slots: Vec<_> = self.slots.lock().unwrap().drain().collect();
        for (_, slot) in slots {
            let state = slot.borrow().clone();
            if let Slot::Ready(server) = state {
                server.shutdown().await;
            }
        }
    }
}

/// The command for the first of `spec`'s command lines whose program is on
/// the PATH.
fn command_for(spec: &ServerSpec) -> Option<tokio::process::Command> {
    spec.commands.iter().find_map(|line| {
        let (program, args) = line.split_first()?;
        let found = which(program)?;
        Some(command(&found, args))
    })
}

#[cfg(windows)]
fn command(program: &Path, args: &[String]) -> tokio::process::Command {
    // `npm i -g` installs a `.cmd`, which `CreateProcess` can't run directly.
    let batch = matches!(
        program
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("cmd") | Some("bat")
    );
    let mut c = if batch {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        tokio::process::Command::new(program)
    };
    c.args(args);
    c
}

#[cfg(not(windows))]
fn command(program: &Path, args: &[String]) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(program);
    c.args(args);
    c
}

/// Where `program` is on the PATH every child is given.
fn which(program: &str) -> Option<PathBuf> {
    let direct = Path::new(program);
    if direct.components().count() > 1 {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    let suffixes: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    std::env::split_paths(&taurus_process::path::current()).find_map(|dir| {
        suffixes.iter().find_map(|suffix| {
            let candidate = dir.join(format!("{program}{suffix}"));
            candidate.is_file().then_some(candidate)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(severity: Severity, line: u32, message: &str) -> Diagnostic {
        Diagnostic {
            line,
            column: 1,
            severity,
            message: message.into(),
            source: None,
            code: None,
        }
    }

    #[test]
    fn errors_are_listed_and_warnings_counted() {
        let outcome = Outcome::Checked {
            server: "rust-analyzer".into(),
            diagnostics: vec![
                d(Severity::Error, 3, "mismatched types:\n  expected u32"),
                d(Severity::Warning, 1, "unused import"),
                d(Severity::Hint, 2, "consider this"),
            ],
        };
        assert_eq!(
            outcome.render("src/main.rs").unwrap(),
            "rust-analyzer reports 1 error in src/main.rs and 1 warning:\n\
             - 3:1: mismatched types: expected u32"
        );
    }

    #[test]
    fn a_clean_file_says_so_briefly() {
        let outcome = Outcome::Checked {
            server: "gopls".into(),
            diagnostics: vec![],
        };
        assert_eq!(
            outcome.render("main.go").unwrap(),
            "gopls: no errors in main.go."
        );
    }

    #[test]
    fn a_long_list_is_cut() {
        let outcome = Outcome::Checked {
            server: "pyright".into(),
            diagnostics: (1..=11).map(|n| d(Severity::Error, n, "bad")).collect(),
        };
        let text = outcome.render("a.py").unwrap();
        assert_eq!(text.lines().count(), 1 + MAX_LISTED + 1);
        assert!(text.ends_with("…and 3 more"));
    }

    #[test]
    fn nothing_to_say_says_nothing() {
        assert_eq!(Outcome::NotCovered.render("x"), None);
        assert_eq!(
            Outcome::NoAnswer {
                server: "clangd".into()
            }
            .render("x"),
            None
        );
    }

    #[test]
    fn a_custom_server_takes_its_extensions_as_language_ids() {
        let spec = ServerSpec::custom(
            "zls",
            vec!["zls".into()],
            vec![".zig".into(), "ZON".into()],
            None,
        );
        assert_eq!(spec.languages.get("zig").map(String::as_str), Some("zig"));
        assert_eq!(spec.languages.get("zon").map(String::as_str), Some("zon"));
    }

    #[tokio::test]
    async fn a_file_no_server_covers_is_not_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "hello").unwrap();
        let lsp = Lsp::new(dir.path(), builtin_specs(), Duration::from_secs(1));
        assert_eq!(lsp.check(&path, "hello").await, Outcome::NotCovered);
    }

    #[tokio::test]
    async fn a_file_outside_the_workspace_is_not_checked() {
        let workspace = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let path = elsewhere.path().join("main.rs");
        std::fs::write(&path, "fn main() {}").unwrap();
        let lsp = Lsp::new(workspace.path(), builtin_specs(), Duration::from_secs(1));
        assert_eq!(lsp.check(&path, "fn main() {}").await, Outcome::NotCovered);
    }
}
