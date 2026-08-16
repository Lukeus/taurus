//! Driving turns from a terminal.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::sync::Arc;

use taurus_agents::proposal::{
    save as save_agent, validate_proposal as validate_agent, AgentProposal,
};
use taurus_core::{Session, UiEvent};
use taurus_host::{sessions, PermissionPromptFactory, SessionLog, TurnRef};
use taurus_provider::Message;
use taurus_skills::proposal::{save, validate_proposal, SkillProposal};
use taurus_tools::PermissionPrompt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::permission::{Policy, TerminalPrompt};
use crate::render::{Format, Renderer};
use crate::{Runtime, SessionArgs};

/// Supplies terminal prompts as the host rebuilds its permission engine.
pub struct TerminalPrompts {
    policy: Policy,
}

impl TerminalPrompts {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl PermissionPromptFactory for TerminalPrompts {
    fn create(&self) -> Box<dyn PermissionPrompt> {
        Box::new(TerminalPrompt::new(self.policy.clone()))
    }
}

/// Picks up a saved conversation, or starts a fresh one.
///
/// `Some(None)` is a bare `--resume`, meaning this workspace's most recent
/// session. A named session that cannot be found is an error rather than a
/// silent new conversation: continuing somewhere else is not what was asked
/// for, and the mistake is invisible until the model answers without context.
async fn open_session(
    runtime: &Runtime,
    resume: Option<&Option<String>>,
    model: &str,
) -> Result<(Session, SessionLog), String> {
    let workspace = runtime.host.workspace().await;

    let requested = match resume {
        None => None,
        Some(Some(id)) => Some(id.clone()),
        Some(None) => Some(
            sessions::latest(&workspace)
                .ok_or_else(|| {
                    format!(
                        "no saved sessions for {}. Start one with `taurus repl`.",
                        workspace.display()
                    )
                })?
                .id,
        ),
    };

    let Some(id) = requested else {
        let session = Session::new(model);
        // The CLI records the branch too: a transcript is a transcript, and one
        // started from a terminal must list the same way as one started from
        // the app.
        let log = SessionLog::create(&session, &workspace, runtime.host.branch().await);
        return Ok((session, log));
    };

    let (session, _) = sessions::load(&id)?;
    let log = SessionLog::resume(&session, &workspace);
    eprintln!(
        "  resuming {} — {} messages, model {}",
        session.id,
        session.messages.len(),
        session.model
    );
    Ok((session, log))
}

/// Runs one task and exits.
pub async fn run_once(
    runtime: &Runtime,
    args: &SessionArgs,
    resume: Option<&Option<String>>,
    task: &str,
    format: Format,
    quiet: bool,
    verbose: bool,
) -> Result<ExitCode, String> {
    let (provider_id, model) = runtime
        .host
        .resolve_model(args.provider.as_deref(), args.model.as_deref())
        .await?;
    let provider = runtime.host.provider(&provider_id).await?;

    let cancel = CancellationToken::new();
    install_interrupt_handler(cancel.clone());

    let (mut session, mut log) = open_session(runtime, resume, &model).await?;
    let ok = turn(
        runtime,
        &provider_id,
        provider,
        &model,
        &mut session,
        &mut log,
        task,
        format,
        quiet,
        verbose,
        cancel,
    )
    .await?;

    handle_proposals(runtime).await;
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Interactive session over stdin.
pub async fn repl(
    runtime: &Runtime,
    args: &SessionArgs,
    resume: Option<&Option<String>>,
) -> Result<ExitCode, String> {
    let (provider_id, model) = runtime
        .host
        .resolve_model(args.provider.as_deref(), args.model.as_deref())
        .await?;
    let provider = runtime.host.provider(&provider_id).await?;

    let capabilities = provider
        .capabilities(&model)
        .await
        .map_err(|e| format!("could not reach provider '{provider_id}': {e}"))?;

    eprintln!(
        "taurus — {} in {}",
        model,
        runtime.host.workspace().await.display()
    );
    if !capabilities.native_tools {
        eprintln!("  {model} has no built-in tool calling; using prompted tool calls.");
    }
    eprintln!("  Ctrl-D to exit, Ctrl-C to interrupt a turn.\n");

    let (mut session, mut log) = open_session(runtime, resume, &model).await?;

    loop {
        eprint!("› ");
        let _ = std::io::stderr().flush();

        // Reading stdin blocks; keep it off the async runtime.
        let read = tokio::task::spawn_blocking(move || {
            let mut buf = String::new();
            std::io::stdin().read_line(&mut buf).map(|n| (n, buf))
        })
        .await
        .map_err(|e| e.to_string())?;

        let line = match read {
            Ok((0, _)) => break, // EOF
            Ok((_, buf)) => buf,
            Err(e) => return Err(e.to_string()),
        };

        let task = line.trim();
        if task.is_empty() {
            continue;
        }
        if matches!(task, "exit" | "quit" | ":q") {
            break;
        }

        // A fresh token per turn so an interrupted turn does not kill the next.
        let cancel = CancellationToken::new();
        install_interrupt_handler(cancel.clone());

        turn(
            runtime,
            &provider_id,
            provider.clone(),
            &model,
            &mut session,
            &mut log,
            task,
            Format::Human,
            false,
            false,
            cancel,
        )
        .await?;

        handle_proposals(runtime).await;
        eprintln!();
    }

    Ok(ExitCode::SUCCESS)
}

/// One turn, streamed to the terminal. Returns whether it completed cleanly.
#[allow(clippy::too_many_arguments)]
async fn turn(
    runtime: &Runtime,
    provider_id: &str,
    provider: Arc<dyn taurus_provider::Provider>,
    model: &str,
    session: &mut Session,
    log: &mut SessionLog,
    task: &str,
    format: Format,
    quiet: bool,
    verbose: bool,
    cancel: CancellationToken,
) -> Result<bool, String> {
    runtime.host.remember_session(provider_id, model).await;

    // A leading `/name` runs that skill. Resolved before the turn starts so a
    // mistyped command costs nothing: the user gets told, and no request is
    // made. `task` stays what names the turn, being what was actually asked.
    let prompt = match runtime.host.expand_command(task).await {
        Some(Ok(invocation)) => invocation.prompt,
        Some(Err(e)) => return Err(e.to_string()),
        None => task.to_string(),
    };

    let agent = runtime
        .host
        .build_agent(
            provider,
            model,
            cancel,
            TurnRef {
                session_id: &session.id,
                prompt: task,
            },
        )
        .await;

    let (tx, mut rx) = mpsc::channel::<UiEvent>(256);
    let mut renderer = Renderer::new(format, quiet, verbose);
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            renderer.handle(&event);
        }
        renderer.finish();
    });

    let outcome = agent.run_turn(session, Message::user(prompt), tx).await;
    printer.await.map_err(|e| e.to_string())?;

    // Recorded whatever the outcome: an interrupted or failed turn still
    // produced the messages that led there, and those are the ones worth
    // resuming from.
    log.record(session);

    match outcome {
        Ok(_) => Ok(true),
        Err(e) => {
            eprintln!("taurus: {e}");
            Ok(false)
        }
    }
}

/// Reviews whatever the agent proposed during the turn.
///
/// With a terminal, asks. Without one, reports and discards — writing a skill
/// nobody reviewed would defeat the approval gate the whole design rests on.
async fn handle_proposals(runtime: &Runtime) {
    let proposals: Vec<SkillProposal> = {
        let mut queued = runtime.proposals.proposals.lock().await;
        std::mem::take(&mut *queued)
    };
    if proposals.is_empty() {
        return;
    }

    for proposal in proposals {
        if let Err(e) = validate_proposal(&proposal, &*runtime.host.catalog().read().await) {
            eprintln!("  skill '{}' rejected: {e}", proposal.name);
            continue;
        }

        if !runtime.interactive {
            eprintln!(
                "  skill '{}' was proposed but not saved (no terminal to review it).\n    \
                 Run `taurus repl` or open the app to review it.",
                proposal.name
            );
            continue;
        }

        if !review(&proposal) {
            continue;
        }

        let root = taurus_host::config::workspace_skills_dir(&runtime.host.workspace().await);
        match save(&proposal, &root) {
            Ok(dir) => {
                eprintln!("  saved skill to {}", dir.display());
                // Reload so it is usable in the very next turn of a REPL.
                runtime.host.reload().await;
            }
            Err(e) => eprintln!("  could not save skill: {e}"),
        }
    }

    handle_agent_proposals(runtime).await;
}

/// The same review, for sub-agents the turn proposed.
///
/// Kept beside the skill flow rather than folded into it: the two carry
/// different fields and print differently, and a shared loop over an enum would
/// be longer than both.
async fn handle_agent_proposals(runtime: &Runtime) {
    let proposals: Vec<AgentProposal> = {
        let mut queued = runtime.agent_proposals.proposals.lock().await;
        std::mem::take(&mut *queued)
    };
    if proposals.is_empty() {
        return;
    }

    // Re-checked here rather than trusted from propose time: the roster and the
    // tool registry can both have moved since, and this is the last point
    // before a file is written.
    let available: Vec<String> = runtime
        .host
        .registry()
        .read()
        .await
        .names()
        .map(str::to_string)
        .collect();

    for proposal in proposals {
        let verdict = {
            let catalog = runtime.host.agent_catalog().read().await;
            validate_agent(&proposal, &catalog, &available)
        };
        if let Err(e) = verdict {
            eprintln!("  agent '{}' rejected: {e}", proposal.name);
            continue;
        }

        if !runtime.interactive {
            eprintln!(
                "  agent '{}' was proposed but not saved (no terminal to review it).\n    \
                 Run `taurus repl` or open the app to review it.",
                proposal.name
            );
            continue;
        }

        if !review_agent(&proposal) {
            continue;
        }

        let root = taurus_host::config::workspace_agents_dir(&runtime.host.workspace().await);
        match save_agent(&proposal, &root) {
            Ok(path) => {
                eprintln!("  saved agent to {}", path.display());
                // Narrower than a full reload, which would restart every MCP
                // server to pick up one markdown file.
                runtime.host.rescan_agents().await;
            }
            Err(e) => eprintln!("  could not save agent: {e}"),
        }
    }
}

/// Shows a proposed agent and asks whether to keep it.
fn review_agent(proposal: &AgentProposal) -> bool {
    let mut err = std::io::stderr();
    let _ = writeln!(
        err,
        "\n  Taurus wants to save a sub-agent: {}",
        proposal.name
    );
    let _ = writeln!(err, "    {}", proposal.description);
    let _ = writeln!(
        err,
        "    tools: {}",
        match &proposal.tools {
            Some(tools) => tools.join(", "),
            None => "inherits yours".to_string(),
        }
    );
    let _ = write!(err, "  [v] view prompt  [y] save  [n] discard: ");
    let _ = err.flush();

    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }

    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "v" | "view" => {
            let _ = writeln!(err, "\n{}\n", proposal.prompt);
            let _ = write!(err, "  [y] save  [n] discard: ");
            let _ = err.flush();
            let mut second = String::new();
            let _ = std::io::stdin().read_line(&mut second);
            matches!(second.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        }
        _ => false,
    }
}

/// Shows a proposal and asks whether to keep it.
fn review(proposal: &SkillProposal) -> bool {
    let mut err = std::io::stderr();
    let _ = writeln!(err, "\n  Taurus wants to save a skill: {}", proposal.name);
    let _ = writeln!(err, "    {}", proposal.description);
    let _ = writeln!(err, "    fires when: {}", proposal.when_to_use);
    for script in &proposal.scripts {
        let _ = writeln!(
            err,
            "    bundles script: {} ({})",
            script.path, script.interpreter
        );
    }
    let _ = write!(err, "  [v] view  [y] save  [n] discard: ");
    let _ = err.flush();

    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }

    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "v" | "view" => {
            let _ = writeln!(err, "\n{}\n", proposal.body);
            for script in &proposal.scripts {
                let _ = writeln!(err, "--- {} ---\n{}\n", script.path, script.content);
            }
            let _ = write!(err, "  [y] save  [n] discard: ");
            let _ = err.flush();
            let mut second = String::new();
            let _ = std::io::stdin().read_line(&mut second);
            matches!(second.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        }
        _ => false,
    }
}

/// Makes Ctrl-C cancel the turn rather than kill the process, so a half-written
/// file gets its tool call cleaned up and the session ends consistently.
fn install_interrupt_handler(cancel: CancellationToken) {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            if std::io::stderr().is_terminal() {
                eprintln!("\n  interrupted");
            }
            cancel.cancel();
        }
    });
}
