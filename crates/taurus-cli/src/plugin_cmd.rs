//! `taurus plugin` — installing, listing and switching plugins.
//!
//! A plugin bundles skills, sub-agents, MCP servers and hooks under one name,
//! in Claude Code's layout. See `taurus_host::plugins`, and
//! `docs/configuration.md#plugins`.

use std::path::Path;
use std::process::ExitCode;

use clap::Subcommand;
use taurus_host::{plugins, trust, Host, PluginSummary, Scope};

#[derive(Subcommand)]
pub enum PluginCommand {
    /// List installed plugins: what each brings, and whether it's on.
    List,
    /// Install a plugin from a folder or a git URL. It's on once it's in.
    Add {
        /// A folder, or a git URL (https://, ssh://, git@…, or ending in .git),
        /// or a GitHub or GitLab page for a folder in one (…/tree/<ref>/<path>).
        from: String,
        /// The branch or tag to clone. The default branch otherwise.
        #[arg(long = "ref", value_name = "REF")]
        git_ref: Option<String>,
        /// Install it in this project's .taurus/plugins/ rather than yours.
        #[arg(long)]
        project: bool,
    },
    /// Fetch a plugin again from where it was added from.
    Update {
        name: String,
        #[arg(long)]
        project: bool,
    },
    /// Delete an installed plugin.
    Remove {
        name: String,
        #[arg(long)]
        project: bool,
    },
    /// Switch a plugin on, in your settings or (with --project) this project's.
    Enable {
        name: String,
        #[arg(long)]
        project: bool,
    },
    /// Switch a plugin off, in your settings or (with --project) this project's.
    Disable {
        name: String,
        #[arg(long)]
        project: bool,
    },
    /// Check a plugin folder without installing it: what would load, and what
    /// wouldn't.
    Validate { path: String },
}

pub async fn run(host: &Host, command: PluginCommand) -> Result<ExitCode, String> {
    let workspace = host.workspace().await;
    let ws = Some(workspace.as_path());
    let scope = |project: bool| {
        if project {
            Scope::Workspace
        } else {
            Scope::Global
        }
    };

    let on = matches!(command, PluginCommand::Enable { .. });
    match command {
        PluginCommand::List => {
            let found = plugins::installed(ws);
            if found.is_empty() {
                println!("No plugins installed. Looked in:");
                for scope in [Scope::Global, Scope::Workspace] {
                    if let Some(dir) = plugins::dir(scope, ws) {
                        println!("  {}", dir.display());
                    }
                }
                println!("\nAdd one:  taurus plugin add <folder or git URL>");
                return Ok(ExitCode::SUCCESS);
            }
            let trusted = trust::is_trusted(&workspace);
            for (i, plugin) in found.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                print(&plugin.summary, trusted);
            }
            Ok(ExitCode::SUCCESS)
        }

        PluginCommand::Add {
            from,
            git_ref,
            project,
        } => {
            let summary = plugins::add(&from, git_ref.as_deref(), scope(project), ws)?;
            println!("Installed {} at {}.\n", summary.name, summary.root);
            print(&summary, trust::is_trusted(&workspace));
            println!("\n{}", next_step(&summary, &workspace));
            Ok(ExitCode::SUCCESS)
        }

        PluginCommand::Update { name, project } => {
            let summary = plugins::update(&name, scope(project), ws)?;
            let at = summary
                .source
                .as_ref()
                .and_then(|s| s.commit.as_deref())
                .map(|commit| format!(" at {}", short(commit)))
                .unwrap_or_default();
            println!("Updated {name}{at}.\n");
            print(&summary, trust::is_trusted(&workspace));
            Ok(ExitCode::SUCCESS)
        }

        PluginCommand::Remove { name, project } => {
            plugins::remove(&name, scope(project), ws)?;
            println!("Removed {name}.");
            Ok(ExitCode::SUCCESS)
        }

        PluginCommand::Enable { name, project } | PluginCommand::Disable { name, project } => {
            plugins::set_enabled(&name, scope(project), ws, on)?;
            let file = if project { "this project's" } else { "your" };
            println!(
                "{name} is {} in {file} settings. New conversations {} it.",
                if on { "on" } else { "off" },
                if on { "have" } else { "won't have" }
            );
            Ok(ExitCode::SUCCESS)
        }

        PluginCommand::Validate { path } => {
            let root = Path::new(&path)
                .canonicalize()
                .map_err(|e| format!("{path}: {e}"))?;
            let plugin = plugins::load(&root, Scope::Global, None);
            print(&plugin.summary, true);
            if plugin.summary.problems.is_empty() {
                println!("\nIt loads.");
                Ok(ExitCode::SUCCESS)
            } else {
                println!("\nIt doesn't load until the problems above are fixed.");
                Ok(ExitCode::FAILURE)
            }
        }
    }
}

/// One plugin, as a few lines.
fn print(plugin: &PluginSummary, trusted: bool) {
    let version = plugin
        .version
        .as_deref()
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    let whose = match plugin.scope {
        Scope::Global => "yours",
        Scope::Workspace => "this project's",
    };
    let state = if !plugin.problems.is_empty() {
        "doesn't load"
    } else if plugin.shadowed {
        "replaced by this project's plugin of the same name"
    } else if plugin.scope == Scope::Workspace && !trusted {
        "waiting for this project to be trusted"
    } else if plugin.enabled {
        "on"
    } else {
        "off"
    };
    println!("{}{version}  ({whose}, {state})", plugin.name);
    println!("  {}", plugin.root);
    if let Some(description) = &plugin.description {
        println!("  {description}");
    }
    let mut parts = Vec::new();
    let named = |label: &str, names: &[String]| {
        (!names.is_empty()).then(|| format!("{label}: {}", names.join(", ")))
    };
    parts.extend(named("skills", &plugin.skills));
    parts.extend(named("agents", &plugin.agents));
    parts.extend(named("MCP servers", &plugin.mcp_servers));
    parts.extend(named("hooks", &plugin.hooks));
    if parts.is_empty() {
        println!("  (nothing Taurus can run)");
    } else {
        println!("  {}", parts.join(" · "));
    }
    for problem in &plugin.problems {
        println!("  ! {problem}");
    }
    for part in &plugin.unsupported {
        println!("  not run here: {} — {}", part.part, part.reason);
    }
    for warning in &plugin.warnings {
        println!("  note: {warning}");
    }
    if let Some(source) = &plugin.source {
        match &source.commit {
            Some(commit) => println!("  from {} at {}", source.from, short(commit)),
            None => println!("  from {}", source.from),
        }
    }
}

/// What happens next for a plugin just added.
fn next_step(plugin: &PluginSummary, workspace: &Path) -> String {
    let prefix = format!("{}:", plugin.name);
    let example = plugin
        .skills
        .first()
        .map(|skill| format!(" Its skills are named {prefix}{skill}, and so on."))
        .unwrap_or_default();
    if plugin.scope == Scope::Workspace && !trust::is_trusted(workspace) {
        format!(
            "It's in this project, which isn't trusted, so nothing in it runs until it is: \
             `taurus trust --allow`.{example}"
        )
    } else {
        format!("It's on, and new conversations have it.{example}")
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}
