//! `taurus` — the Taurus agent harness on the command line.
//!
//! Same harness the desktop app runs: same config, same skills, same
//! permission allowlist. Everything shared lives in `taurus-host`; this crate
//! only decides how to talk to a terminal.

mod key_cmd;
mod markdown;
mod permission;
mod render;
mod rewind_cmd;
mod session;
mod skills_cmd;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use taurus_host::Host;
use taurus_skills::proposal::CollectingSink;
use tracing_subscriber::EnvFilter;

use permission::{Policy, TerminalPrompt};
use render::Format;

#[derive(Parser)]
#[command(
    name = "taurus",
    version,
    about = "A local agent shell",
    long_about = "Runs the Taurus agent harness in a terminal. Shares its configuration, skills, \
                  and permission allowlist with the Taurus desktop app."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run one task and exit.
    Run(RunArgs),
    /// Start an interactive session.
    Repl(ReplArgs),
    /// List saved sessions.
    Sessions {
        #[command(flatten)]
        session: SessionArgs,

        /// Every workspace, not just this one.
        #[arg(long)]
        all: bool,
    },
    /// List file changes a session made, and undo them.
    Rewind {
        #[command(flatten)]
        session: SessionArgs,

        /// Session to inspect. Defaults to this workspace's most recent.
        #[arg(long, value_name = "ID")]
        id: Option<String>,

        /// Restore the workspace to just before this turn, undoing it and
        /// everything after it. `last` undoes only the most recent turn.
        #[arg(long, value_name = "TURN|last")]
        to: Option<String>,

        /// Show what `--to` would change without writing anything.
        #[arg(long)]
        dry_run: bool,

        /// Skip the confirmation. Required to rewind without a terminal.
        #[arg(long)]
        yes: bool,
    },
    /// Inspect the skill library.
    Skills {
        #[command(subcommand)]
        command: skills_cmd::SkillsCommand,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Show MCP server connection status.
    Mcp {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Store provider API keys in the OS credential store.
    Key {
        #[command(subcommand)]
        command: key_cmd::KeyCommand,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// List the models a provider offers.
    Models {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// List every tool available to the agent, built-ins included.
    Tools {
        #[command(flatten)]
        session: SessionArgs,
    },
}

#[derive(Args, Clone)]
pub struct SessionArgs {
    /// Workspace directory. Defaults to the last one used, else the current
    /// directory.
    #[arg(short = 'w', long, global = true)]
    pub workspace: Option<PathBuf>,

    /// Provider id from ~/.taurus/providers.json.
    #[arg(short = 'p', long, global = true)]
    pub provider: Option<String>,

    /// Model to use. Defaults to the last one used, else the provider's first.
    #[arg(short = 'm', long, global = true)]
    pub model: Option<String>,
}

#[derive(Args, Clone)]
pub struct ResumeArg {
    /// Continue a saved conversation. Bare `--resume` takes this workspace's
    /// most recent one; `--resume <ID>` names one from `taurus sessions`.
    #[arg(long, num_args = 0..=1, value_name = "ID")]
    pub resume: Option<Option<String>>,
}

#[derive(Args)]
pub struct ReplArgs {
    #[command(flatten)]
    pub session: SessionArgs,

    #[command(flatten)]
    pub resume: ResumeArg,
}

#[derive(Args)]
pub struct RunArgs {
    /// What you want done.
    pub task: Vec<String>,

    #[command(flatten)]
    pub session: SessionArgs,

    #[command(flatten)]
    pub resume: ResumeArg,

    #[command(flatten)]
    pub policy: PolicyArgs,

    /// Emit newline-delimited JSON events on stdout instead of prose.
    #[arg(long)]
    pub json: bool,

    /// Print only the model's answer, with no tool activity.
    #[arg(short, long)]
    pub quiet: bool,

    /// Also stream the model's reasoning, when it produces any.
    #[arg(short, long, conflicts_with = "quiet")]
    pub verbose: bool,
}

/// What the agent may do without being asked.
///
/// These matter most when there is no terminal to prompt on — a pipe, a git
/// hook, CI. With a terminal, they act as "stop asking me about this".
#[derive(Args, Clone, Default)]
pub struct PolicyArgs {
    /// Allow a tool without prompting, e.g. `--allow write_file`. Repeatable.
    #[arg(long = "allow", value_name = "TOOL")]
    pub allow: Vec<String>,

    /// Allow a shell program without prompting, e.g. `--allow-command git`.
    /// Repeatable. Matches the leading word only, so `git` does not grant `rm`.
    #[arg(long = "allow-command", value_name = "PROGRAM")]
    pub allow_command: Vec<String>,

    /// Allow every action without prompting. Only sensible in a throwaway or
    /// already-sandboxed environment.
    #[arg(long)]
    pub dangerously_allow_all: bool,
}

impl From<&PolicyArgs> for Policy {
    fn from(args: &PolicyArgs) -> Self {
        Policy {
            allow_tools: args.allow.iter().cloned().collect(),
            allow_commands: args.allow_command.iter().cloned().collect(),
            allow_all: args.dangerously_allow_all,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    // Silent unless asked: a CLI's stderr belongs to the task, not to logs.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("TAURUS_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("taurus: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.command {
        Command::Run(args) => {
            let task = args.task.join(" ");
            if task.trim().is_empty() {
                return Err("no task given. Try: taurus run \"list the rust files\"".into());
            }
            let policy = Policy::from(&args.policy);
            let runtime = build_host(&args.session, policy).await?;
            let format = if args.json {
                Format::Json
            } else {
                Format::Human
            };
            session::run_once(
                &runtime,
                &args.session,
                args.resume.resume.as_ref(),
                &task,
                format,
                args.quiet,
                args.verbose,
            )
            .await
        }

        Command::Repl(args) => {
            // A REPL always has a person at it, so it prompts rather than
            // taking a policy up front.
            let runtime = build_host(&args.session, Policy::default()).await?;
            session::repl(&runtime, &args.session, args.resume.resume.as_ref()).await
        }

        Command::Sessions { session, all } => {
            let host = build_host(&session, Policy::default()).await?.host;
            let workspace = host.workspace().await;
            let listed = taurus_host::sessions::list(if all { None } else { Some(&workspace) });

            if listed.is_empty() {
                println!("No saved sessions for {}.", workspace.display());
                return Ok(ExitCode::SUCCESS);
            }
            for meta in listed {
                let title = if meta.title.is_empty() {
                    "(no turns yet)"
                } else {
                    &meta.title
                };
                println!("{:<38} {:<24} {title}", meta.id, meta.model);
                if all {
                    println!("{:38} {}", "", meta.workspace);
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::Rewind {
            session,
            id,
            to,
            dry_run,
            yes,
        } => {
            let host = build_host(&session, Policy::default()).await?.host;
            rewind_cmd::run(&host, id.as_deref(), to.as_deref(), dry_run, yes).await
        }

        Command::Skills { command, session } => {
            let runtime = build_host(&session, Policy::default()).await?;
            skills_cmd::run(&runtime.host, command).await
        }

        Command::Key { command, session } => {
            let host = build_host(&session, Policy::default()).await?.host;
            key_cmd::run(&host, command).await
        }

        Command::Mcp { session } => {
            let host = build_host(&session, Policy::default()).await?.host;
            let statuses = host.mcp_statuses().await;
            if statuses.is_empty() {
                println!("No MCP servers configured in ~/.taurus/mcp.json.");
                return Ok(ExitCode::SUCCESS);
            }
            let mut failed = false;
            for status in statuses {
                if status.connected {
                    println!(
                        "{:<20} {} tools  {}",
                        status.name, status.tool_count, status.description
                    );
                } else {
                    failed = true;
                    println!(
                        "{:<20} FAILED     {}",
                        status.name,
                        status.error.unwrap_or_default()
                    );
                }
            }
            Ok(if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }

        Command::Models { session } => {
            let host = build_host(&session, Policy::default()).await?.host;
            let (provider_id, _) = host
                .resolve_model(session.provider.as_deref(), None)
                .await?;
            let provider = host.provider(&provider_id).await?;
            let models = provider
                .models()
                .await
                .map_err(|e| format!("could not reach provider '{provider_id}': {e}"))?;
            for model in models {
                println!("{}", model.display_name);
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::Tools { session } => {
            let host = build_host(&session, Policy::default()).await?.host;
            for name in host.tool_names().await {
                println!("{name}");
            }
            // Why a tool is *not* in that list is the question this command is
            // usually being asked, and a config that resolved to nothing is the
            // usual answer. On stderr, so a script reading the list still gets
            // names and nothing else on stdout.
            for problem in host.problems().await {
                eprintln!("problem: {problem}");
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// A configured harness plus the things the CLI needs alongside it.
pub struct Runtime {
    pub host: Arc<Host>,
    /// Proposals are collected during a turn and dealt with after it, so a
    /// review prompt never cuts into streaming output.
    pub proposals: Arc<CollectingSink>,
    /// False when there is no terminal, which changes how proposals and
    /// refusals are reported.
    pub interactive: bool,
}

/// Builds the harness, resolving the workspace first so skills and the
/// permission allowlist come from the right place.
async fn build_host(args: &SessionArgs, policy: Policy) -> Result<Runtime, String> {
    let workspace = match &args.workspace {
        Some(path) => path.canonicalize().map_err(|_| {
            format!(
                "workspace {} does not exist or is not reachable",
                path.display()
            )
        })?,
        None => Host::default_workspace(),
    };
    if !workspace.is_dir() {
        return Err(format!("{} is not a directory", workspace.display()));
    }

    let interactive = TerminalPrompt::new(policy.clone()).is_interactive();
    let prompts = Arc::new(session::TerminalPrompts::new(policy));
    let proposals = Arc::new(CollectingSink::default());

    let host = Arc::new(Host::new(workspace, prompts, proposals.clone()));
    host.reload().await;
    Ok(Runtime {
        host,
        proposals,
        interactive,
    })
}
