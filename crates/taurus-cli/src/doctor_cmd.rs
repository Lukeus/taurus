//! `taurus doctor` — everything this session depends on, checked in one go.
//!
//! The usual question behind a failed run is "which of the five things it
//! needed wasn't there": a key not set, a backend not running, an MCP server
//! not on the PATH, a config file that doesn't parse. Each has a place in the
//! app that shows it, and each of those places has to be opened to look. This
//! looks at all of them and says which are wrong, in the order a run would
//! trip over them.
//!
//! It reads and connects; it changes nothing. It does start every enabled MCP
//! server, which is the only honest way to say whether one starts, and asks
//! each provider for its models, which is a request but not a billed one.
//!
//! The exit status is a failure when something a run needs is broken: a
//! config file that won't read, the provider a run would use, an enabled MCP
//! server. A provider configured but not in use being unreachable is a
//! warning — an Ollama that isn't running is only a problem the day it's
//! picked.

use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::Duration;

use taurus_host::Host;

/// How long one provider gets to list its models.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    /// Not in the way of a run now, and worth knowing.
    Warn,
    /// Something a run needs is broken.
    Fail,
    /// Switched off or not installed, on purpose or harmlessly.
    Off,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
            Self::Off => "off",
        }
    }
}

#[derive(Debug)]
pub struct Check {
    pub level: Level,
    pub name: String,
    pub detail: String,
}

impl Check {
    fn new(level: Level, name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level,
            name: name.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub sections: Vec<(String, Vec<Check>)>,
}

impl Report {
    fn section(&mut self, title: &str, checks: Vec<Check>) {
        self.sections.push((title.to_string(), checks));
    }

    pub fn failed(&self) -> bool {
        self.sections
            .iter()
            .flat_map(|(_, checks)| checks)
            .any(|check| check.level == Level::Fail)
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for (title, checks) in &self.sections {
            let _ = writeln!(out, "\n{title}");
            if checks.is_empty() {
                let _ = writeln!(out, "  {:<5} nothing configured", "");
            }
            for check in checks {
                let _ = writeln!(
                    out,
                    "  {:<5} {:<28} {}",
                    check.level.label(),
                    check.name,
                    check.detail
                );
            }
        }
        let (fail, warn) =
            self.sections
                .iter()
                .flat_map(|(_, checks)| checks)
                .fold((0, 0), |(f, w), check| match check.level {
                    Level::Fail => (f + 1, w),
                    Level::Warn => (f, w + 1),
                    _ => (f, w),
                });
        let _ = writeln!(
            out,
            "\n{}",
            match (fail, warn) {
                (0, 0) => "Everything checked is working.".to_string(),
                (0, w) => format!("Nothing is broken; {w} warning(s) above."),
                (f, _) => format!("{f} problem(s) above will stop a run."),
            }
        );
        out
    }
}

pub async fn run(host: &Host, provider: Option<&str>) -> Result<ExitCode, String> {
    let workspace = host.workspace().await;
    println!("Checking Taurus for {}", workspace.display());

    let mut report = Report::default();
    report.section("Workspace", workspace_checks(&workspace));
    report.section("Config files", config_checks(host).await);
    report.section("Providers", provider_checks(host, provider).await);
    report.section("MCP servers", mcp_checks(host).await);
    report.section("Language servers", lsp_checks(host).await);

    print!("{}", report.render());
    Ok(if report.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn workspace_checks(workspace: &std::path::Path) -> Vec<Check> {
    vec![if taurus_host::trust::is_trusted(workspace) {
        Check::new(Level::Ok, "trusted", "its own .taurus config is read")
    } else {
        Check::new(
            Level::Warn,
            "not trusted",
            "its .taurus config, MCP servers and language servers aren't used. \
             `taurus trust` says what's waiting.",
        )
    }]
}

async fn config_checks(host: &Host) -> Vec<Check> {
    let problems = host.problems().await;
    if problems.is_empty() {
        return vec![Check::new(
            Level::Ok,
            "all read",
            "settings, providers, MCP, skills and agents",
        )];
    }
    problems
        .into_iter()
        .map(|problem| {
            Check::new(
                Level::Fail,
                format!("{:?}", problem.source),
                problem.message,
            )
        })
        .collect()
}

async fn provider_checks(host: &Host, asked: Option<&str>) -> Vec<Check> {
    let providers = host.providers().await;
    let resolved = host.resolve_model(asked, None).await;
    let in_use = resolved.as_ref().ok().map(|(id, _)| id.clone());
    // All at once: each can take its whole deadline, and they don't depend on
    // one another. Reported in the file's order, which `join_all` keeps.
    let checks = futures::future::join_all(
        providers
            .iter()
            .map(|config| provider_check(host, config, in_use.as_deref())),
    )
    .await;
    resolution_check(&resolved)
        .into_iter()
        .chain(checks)
        .collect()
}

/// A failure when no provider and model resolve at all. Without it, no
/// provider is the one in use, so none can fail, and `doctor` passed a setup
/// that every run refuses.
fn resolution_check(resolved: &Result<(String, String), String>) -> Option<Check> {
    resolved
        .as_ref()
        .err()
        .map(|e| Check::new(Level::Fail, "no model to run on", e.clone()))
}

async fn provider_check(
    host: &Host,
    config: &taurus_host::ProviderConfig,
    in_use: Option<&str>,
) -> Check {
    let using = in_use == Some(config.id.as_str());
    let name = if using {
        format!("{} (in use)", config.id)
    } else {
        config.id.clone()
    };
    // Only the one a run would use is a failure. See the module docs.
    let broken = if using { Level::Fail } else { Level::Warn };
    // Building one reads its key from the credential store, which can wait on
    // a dialog asking whether this build may read it: a debug build, or one
    // updated since the keychain was told. So the deadline covers that as
    // well as the request, and running out says so.
    let listed = tokio::time::timeout(PROVIDER_TIMEOUT, async {
        let provider = host.provider(&config.id).await?;
        provider
            .models()
            .await
            .map_err(|e| format!("{}: {e}", config.base_url))
    })
    .await;
    match listed {
        Ok(Ok(models)) => Check::new(
            Level::Ok,
            name,
            format!("{} answers, {} model(s)", config.base_url, models.len()),
        ),
        Ok(Err(e)) => Check::new(broken, name, e),
        Err(_) => Check::new(
            broken,
            name,
            format!(
                "no answer within {}s. Either {} didn't respond, or reading its key is \
                 waiting on a keychain prompt",
                PROVIDER_TIMEOUT.as_secs(),
                config.base_url
            ),
        ),
    }
}

async fn mcp_checks(host: &Host) -> Vec<Check> {
    host.mcp_servers()
        .await
        .into_iter()
        .map(|server| {
            if server.disabled {
                let cost = server
                    .last_seen_tokens
                    .map(|t| format!(", ~{t} tokens when on"))
                    .unwrap_or_default();
                return Check::new(Level::Off, server.name, format!("switched off{cost}"));
            }
            match server.status {
                Some(status) if status.connected => {
                    let mut detail = format!("{} tool(s)", status.tool_count);
                    if let Some(tokens) = server.schema_tokens {
                        let _ = write!(detail, ", ~{tokens} tokens of every request");
                    }
                    if !status.prompts.is_empty() {
                        let _ = write!(detail, ", {} prompt(s)", status.prompts.len());
                    }
                    Check::new(Level::Ok, server.name, detail)
                }
                Some(status) => Check::new(
                    Level::Fail,
                    server.name,
                    status.error.unwrap_or_else(|| "not connected".into()),
                ),
                None => Check::new(Level::Fail, server.name, "never started"),
            }
        })
        .collect()
}

async fn lsp_checks(host: &Host) -> Vec<Check> {
    let settings = host.settings().await.lsp;
    if !settings.enabled {
        return vec![Check::new(Level::Off, "all", "switched off in settings")];
    }
    settings
        .specs()
        .into_iter()
        .map(|spec| {
            let found = spec.commands.iter().find_map(|command| {
                let program = command.first()?;
                taurus_tools::login_path::which(program).map(|path| path.display().to_string())
            });
            let mut extensions: Vec<&str> = spec.languages.keys().map(String::as_str).collect();
            extensions.truncate(4);
            match found {
                Some(path) => Check::new(Level::Ok, spec.name, path),
                // Not a failure: a server is only looked for, and only used
                // for its own files.
                None => Check::new(
                    Level::Off,
                    spec.name,
                    format!("not installed (for .{})", extensions.join(", .")),
                ),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_anywhere_fails_the_run_and_a_warning_does_not() {
        let mut report = Report::default();
        report.section("A", vec![Check::new(Level::Ok, "x", "fine")]);
        report.section("B", vec![Check::new(Level::Warn, "y", "meh")]);
        assert!(!report.failed());
        assert!(report.render().contains("Nothing is broken; 1 warning(s)"));

        report.section("C", vec![Check::new(Level::Fail, "z", "broken")]);
        assert!(report.failed());
        let text = report.render();
        assert!(text.contains("  FAIL  z"), "{text}");
        assert!(
            text.contains("1 problem(s) above will stop a run."),
            "{text}"
        );
    }

    #[test]
    fn no_model_to_run_on_is_a_failure() {
        let check = resolution_check(&Err("no providers configured".into())).unwrap();
        assert_eq!(check.level, Level::Fail);
        assert!(resolution_check(&Ok(("ollama".into(), "llama3".into()))).is_none());
    }

    #[test]
    fn an_empty_section_says_so() {
        let mut report = Report::default();
        report.section("MCP servers", Vec::new());
        assert!(report.render().contains("nothing configured"));
    }
}
