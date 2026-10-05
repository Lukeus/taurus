//! `taurus` — the Taurus agent harness on the command line.
//!
//! Same harness the desktop app runs: same config, same skills, same
//! permission allowlist. Everything shared lives in `taurus-host`; this crate
//! only decides how to talk to a terminal.

mod agents_cmd;
mod ask;
mod data_cmd;
mod fork_cmd;
mod hooks_cmd;
mod key_cmd;
mod markdown;
mod notes_cmd;
mod permission;
mod render;
mod review_cmd;
mod rewind_cmd;
mod session;
mod skills_cmd;
mod trust_cmd;
mod usage_cmd;
mod views;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use taurus_agents::proposal::CollectingSink as AgentCollectingSink;
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

        /// The sub-agents one session delegated to, with the transcript each
        /// one left behind.
        #[arg(long, value_name = "ID")]
        agents: Option<String>,
    },
    /// Read a turn back to an agent that did not write it.
    ///
    /// With no `--turn`, lists what there is to review. Reviewing one costs a
    /// model round trip on this workspace's provider.
    Review {
        #[command(flatten)]
        session: SessionArgs,

        /// Session to review. Defaults to this workspace's most recent.
        #[arg(long, value_name = "ID")]
        id: Option<String>,

        /// The turn to review. Omit to list what there is.
        #[arg(long, value_name = "TURN")]
        turn: Option<u32>,

        /// Review it again even if the same diff and claims have been
        /// reviewed on this model before. Without it, that review is shown.
        #[arg(long)]
        again: bool,
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
    /// Start a new conversation from before a turn, keeping the original; or
    /// put a conversation's files back after a fork took the workspace.
    ///
    /// With no `--at` or `--switch`, lists the turns there are to fork before.
    Fork {
        #[command(flatten)]
        session: SessionArgs,

        /// Session to fork, or with `--switch`, to switch to. Defaults to this
        /// workspace's most recent for a fork.
        #[arg(long, value_name = "ID")]
        id: Option<String>,

        /// Fork before this turn: the new conversation has everything up to
        /// it, and the files go back to how they were then. `last` forks
        /// before the most recent turn.
        #[arg(long, value_name = "TURN|last", conflicts_with = "switch")]
        at: Option<String>,

        /// Ask the forked turn's question again in the new conversation, on
        /// `--provider` and `--model` if given.
        #[arg(long, requires = "at")]
        resend: bool,

        /// Ask this in the new conversation instead of the forked turn's
        /// question: the same request worded differently, or with what a
        /// review of the first attempt found. Implies `--resend`.
        #[arg(long, value_name = "TEXT", requires = "at", conflicts_with = "resend")]
        ask: Option<String>,

        /// What the resent turn may do without asking, as for `taurus run`.
        #[command(flatten)]
        policy: PolicyArgs,

        /// Put this conversation's files back in the workspace, setting aside
        /// the branch of it that's there now.
        #[arg(long)]
        switch: bool,

        /// Compare this conversation with another branch of it, without
        /// switching: what each asked, spent and changed since they parted,
        /// and how their files differ now.
        #[arg(long, value_name = "ID", conflicts_with_all = ["at", "switch"])]
        compare: Option<String>,

        /// Show what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show or change whether this workspace's own config is read.
    Trust {
        #[command(flatten)]
        args: trust_cmd::TrustArgs,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Inspect the configured hooks.
    Hooks {
        #[command(subcommand)]
        command: hooks_cmd::HooksCommand,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Inspect the skill library.
    Skills {
        #[command(subcommand)]
        command: skills_cmd::SkillsCommand,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Read or prune what earlier conversations left for the next one.
    Notes {
        #[command(subcommand)]
        command: notes_cmd::NotesCommand,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Inspect the sub-agents this workspace can delegate to.
    Agents {
        #[command(subcommand)]
        command: agents_cmd::AgentsCommand,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// List loaded datasets and recipes, and run a recipe.
    Data {
        #[command(subcommand)]
        command: data_cmd::DataCommand,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Show MCP server connection status.
    Mcp {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Store provider and web-search API keys in the OS credential store.
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
    /// Show what a session spent its context window on, by tool.
    Usage {
        #[command(flatten)]
        session: SessionArgs,

        /// Session to inspect. Defaults to this workspace's most recent.
        #[arg(long, value_name = "ID")]
        id: Option<String>,

        /// Total every session in this workspace instead of one.
        #[arg(long, conflicts_with = "id")]
        all: bool,
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
    let filter = EnvFilter::try_from_env("TAURUS_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let settings = taurus_host::config::load_settings(None);
    // Bound, not discarded: dropping this flushes buffered spans, and a
    // `taurus run` that finishes and exits immediately would otherwise export
    // nothing at all — which is exactly the run somebody is watching for the
    // first time they point this at a collector.
    let _telemetry =
        taurus_telemetry::install(filter, "taurus-cli", Some(settings.otlp_endpoint.as_str()));

    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("taurus: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode, String> {
    let servers = Servers::for_command(&cli.command);
    match cli.command {
        Command::Run(args) => {
            let task = args.task.join(" ");
            if task.trim().is_empty() {
                return Err("no task given. Try: taurus run \"list the rust files\"".into());
            }
            let policy = Policy::from(&args.policy);
            let runtime = build_host(&args.session, policy, servers).await?;
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
            let runtime = build_host(&args.session, Policy::default(), servers).await?;
            session::repl(&runtime, &args.session, args.resume.resume.as_ref()).await
        }

        Command::Sessions {
            session,
            all,
            agents,
        } => {
            let host = build_host(&session, Policy::default(), servers).await?.host;
            let workspace = host.workspace().await;

            if let Some(parent) = agents {
                let listed = taurus_host::sessions::list_subagents(&parent);
                if listed.is_empty() {
                    println!("Session {parent} delegated to no sub-agents.");
                    return Ok(ExitCode::SUCCESS);
                }
                if let Some(dir) = taurus_host::sessions::subagents_dir(&parent) {
                    println!("{}\n", dir.display());
                }
                for meta in listed {
                    let kind = meta.agent.as_deref().unwrap_or("sub-agent");
                    println!("{:<38} {:<20} {}", meta.id, kind, meta.title);
                }
                return Ok(ExitCode::SUCCESS);
            }

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
                // A fork starts with its original's title, so this is the line
                // that tells the two apart.
                if let Some(from) = &meta.forked_from {
                    println!("{:38} fork of {} {}", "", from.session, from.place());
                }
                let store = host.checkpoints_for(std::path::Path::new(&meta.workspace));
                if let Ok(Some(other)) = store.away(&meta.id) {
                    println!("{:38} files set aside when {other} took the workspace", "");
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::Review {
            session,
            id,
            turn,
            again,
        } => {
            let provider = session.provider.clone();
            let model = session.model.clone();
            let host = build_host(&session, Policy::default(), servers).await?.host;
            review_cmd::run(
                &host,
                id.as_deref(),
                turn,
                again,
                provider.as_deref(),
                model.as_deref(),
            )
            .await
        }

        Command::Rewind {
            session,
            id,
            to,
            dry_run,
            yes,
        } => {
            let host = build_host(&session, Policy::default(), servers).await?.host;
            rewind_cmd::run(&host, id.as_deref(), to.as_deref(), dry_run, yes).await
        }

        Command::Fork {
            session,
            id,
            at,
            resend,
            ask,
            policy,
            switch,
            compare,
            dry_run,
        } => {
            let runtime = build_host(&session, Policy::from(&policy), servers).await?;
            let action = match (&at, switch) {
                (Some(turn), _) => fork_cmd::Action::At {
                    turn,
                    resend,
                    ask: ask.as_deref(),
                },
                (None, true) => fork_cmd::Action::Switch,
                (None, false) if compare.is_some() => fork_cmd::Action::Compare {
                    other: compare.as_deref().expect("checked just above"),
                },
                (None, false) => fork_cmd::Action::List,
            };
            let (code, again) =
                fork_cmd::run(&runtime.host, id.as_deref(), action, dry_run).await?;
            match again {
                Some((fork, prompt)) => {
                    let said = if ask.is_some() {
                        "Asking"
                    } else {
                        "Asking it again"
                    };
                    println!("\n{said}: {prompt}\n");
                    session::run_once(
                        &runtime,
                        &session,
                        Some(&Some(fork)),
                        &prompt,
                        Format::Human,
                        false,
                        false,
                    )
                    .await
                }
                None => Ok(code),
            }
        }

        Command::Trust { args, session } => {
            let host = build_host_quietly(&session, Policy::default(), servers)
                .await?
                .host;
            trust_cmd::run(&host, args).await
        }

        Command::Hooks { command, session } => {
            let host = build_host(&session, Policy::default(), servers).await?.host;
            hooks_cmd::run(&host, command).await
        }

        Command::Skills { command, session } => {
            let runtime = build_host(&session, Policy::default(), servers).await?;
            skills_cmd::run(&runtime.host, command).await
        }

        Command::Notes { command, session } => {
            let runtime = build_host(&session, Policy::default(), servers).await?;
            notes_cmd::run(&runtime.host, command).await
        }

        Command::Agents { command, session } => {
            let runtime = build_host(&session, Policy::default(), servers).await?;
            agents_cmd::run(&runtime.host, command).await
        }
        Command::Data { command, session } => {
            let runtime = build_host(&session, Policy::default(), servers).await?;
            data_cmd::run(&runtime.host, command).await
        }

        Command::Key { command, session } => {
            let host = build_host(&session, Policy::default(), servers).await?.host;
            key_cmd::run(&host, command).await
        }

        Command::Mcp { session } => {
            let host = build_host(&session, Policy::default(), servers).await?.host;
            let statuses = host.mcp_statuses().await;
            // An entry that will not parse has no server to hang a status off,
            // so it is only reachable here. It is a failure — the user asked for
            // a server and does not have one — and this command exists to fail
            // a build over exactly that.
            let unreadable = host.problems_from(&[taurus_host::ProblemSource::Mcp]).await;

            if statuses.is_empty() && unreadable.is_empty() {
                println!("No MCP servers configured in ~/.taurus/mcp.json.");
                return Ok(ExitCode::SUCCESS);
            }

            let mut failed = false;
            for status in statuses {
                if status.disabled {
                    // Not a failure: someone switched it off on purpose. Listed
                    // rather than hidden, because a server missing from this
                    // output is the thing that sends people to read the file.
                    println!("{:<20} off        {}", status.name, status.description);
                } else if status.connected {
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
            for problem in &unreadable {
                failed = true;
                println!("{:<20} {}", "UNREADABLE", problem.message);
            }

            Ok(if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }

        Command::Models { session } => {
            let host = build_host(&session, Policy::default(), servers).await?.host;
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
            let host = build_host(&session, Policy::default(), servers).await?.host;
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

        Command::Usage { session, id, all } => {
            let host = build_host(&session, Policy::default(), servers).await?.host;
            let workspace = host.workspace().await;
            let fixed =
                usage_cmd::Fixed::new(&host.system_prompt().await, host.tool_definitions().await);
            usage_cmd::run(&workspace, id.as_deref(), all, &fixed)
        }
    }
}

/// Whether a command starts the MCP servers.
///
/// A full reload starts every configured server and waits for each — up to a
/// minute apiece for an `npx` package being unpacked — and a command that only
/// lists notes, skills or conversations has no use for any of them. The ones
/// that do start them are the ones that run a turn, which may call a server's
/// tools, and the ones that report on what the servers offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Servers {
    Start,
    Leave,
}

impl Servers {
    /// Every command named, so a new one has to say which it is.
    fn for_command(command: &Command) -> Self {
        match command {
            // A turn may call a server's tools.
            Command::Run(_) | Command::Repl(_) => Self::Start,
            // What the servers offer: their status, the tools they add, and
            // what those tools' schemas cost the context window.
            Command::Mcp { .. } | Command::Tools { .. } | Command::Usage { .. } => Self::Start,
            // Only when the forked turn is asked again, which is a turn.
            Command::Fork { resend: true, .. } | Command::Fork { ask: Some(_), .. } => Self::Start,
            Command::Fork { .. } => Self::Leave,
            Command::Sessions { .. }
            | Command::Review { .. }
            | Command::Rewind { .. }
            | Command::Trust { .. }
            | Command::Hooks { .. }
            | Command::Skills { .. }
            | Command::Notes { .. }
            | Command::Agents { .. }
            | Command::Data { .. }
            | Command::Key { .. }
            | Command::Models { .. } => Self::Leave,
        }
    }
}

/// A configured harness plus the things the CLI needs alongside it.
pub struct Runtime {
    pub host: Arc<Host>,
    /// Proposals are collected during a turn and dealt with after it, so a
    /// review prompt never cuts into streaming output.
    pub proposals: Arc<CollectingSink>,
    /// The same, for proposed sub-agents.
    pub agent_proposals: Arc<AgentCollectingSink>,
    /// False when there is no terminal, which changes how proposals and
    /// refusals are reported.
    pub interactive: bool,
}

/// Builds the harness, resolving the workspace first so skills and the
/// permission allowlist come from the right place.
/// A host, plus the one-line notice when this workspace has config going
/// unread. Every command builds its host through here.
async fn build_host(
    args: &SessionArgs,
    policy: Policy,
    servers: Servers,
) -> Result<Runtime, String> {
    let runtime = build_host_quietly(args, policy, servers).await?;
    // After the reload, so it reports the state the session actually starts in.
    trust_cmd::notice(&runtime.host.workspace().await);
    Ok(runtime)
}

/// The same, without the notice — for `taurus trust`, which is about to say
/// all of it at greater length and would otherwise say it twice.
async fn build_host_quietly(
    args: &SessionArgs,
    policy: Policy,
    servers: Servers,
) -> Result<Runtime, String> {
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
    let agent_proposals = Arc::new(AgentCollectingSink::default());

    let host = Arc::new(Host::new(
        workspace,
        prompts,
        Arc::new(ask::TerminalAsker::new()),
        proposals.clone(),
        agent_proposals.clone(),
    ));
    match servers {
        Servers::Start => host.reload().await,
        // The local half only: this command has no use for a server, and each
        // one started can take up to a minute to answer. See `Servers`.
        Servers::Leave => host.reload_local().await,
    }
    Ok(Runtime {
        host,
        proposals,
        agent_proposals,
        interactive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_command_that_needs_the_servers_starts_them() {
        // Each server can take up to a minute to start, and a one-line listing
        // has no use for any of them. Which half of a reload runs is tested in
        // the host; this is which commands ask for it.
        let servers = |args: &[&str]| {
            let cli = Cli::try_parse_from(args.iter().copied())
                .expect("a command line this build accepts");
            Servers::for_command(&cli.command)
        };

        assert_eq!(servers(&["taurus", "sessions"]), Servers::Leave);
        assert_eq!(servers(&["taurus", "models"]), Servers::Leave);
        assert_eq!(servers(&["taurus", "trust"]), Servers::Leave);

        assert_eq!(
            servers(&["taurus", "run", "list the files"]),
            Servers::Start
        );
        assert_eq!(servers(&["taurus", "tools"]), Servers::Start);
        assert_eq!(servers(&["taurus", "mcp"]), Servers::Start);
    }
}
