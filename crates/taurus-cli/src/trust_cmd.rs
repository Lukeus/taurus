//! `taurus trust` — deciding whether a workspace's own config may be read.
//!
//! The desktop app can put the question in front of someone at the moment it
//! matters. A terminal cannot: `taurus run "..."` is often the whole of a
//! session, and stopping it to ask about a config file would be worse than the
//! problem. So the CLI reports rather than asks — every command prints one line
//! when a workspace has config waiting — and this is the command that answers.
//!
//! See [`taurus_host::trust`] for what the decision governs.

use std::path::Path;
use std::process::ExitCode;

use clap::Args;
use taurus_host::Host;

#[derive(Args)]
pub struct TrustArgs {
    /// Let this workspace's own config take effect, from now on.
    #[arg(long, conflicts_with_all = ["revoke", "list"])]
    pub allow: bool,

    /// Stop reading this workspace's config.
    #[arg(long, conflicts_with_all = ["allow", "list"])]
    pub revoke: bool,

    /// Every workspace trusted so far.
    #[arg(long, conflicts_with_all = ["allow", "revoke"])]
    pub list: bool,
}

pub async fn run(host: &Host, args: TrustArgs) -> Result<ExitCode, String> {
    if args.list {
        let trusted = taurus_host::trust::trusted_workspaces();
        if trusted.is_empty() {
            println!("No workspaces trusted yet.");
        } else {
            for path in trusted {
                println!("{path}");
            }
        }
        return Ok(ExitCode::SUCCESS);
    }

    let workspace = host.workspace().await;

    if args.allow {
        host.trust_workspace().await?;
        println!("Trusted {}.", workspace.display());
        // Reported after the reload, so these are counts of what is now loaded
        // rather than of what was promised.
        let status = host.trust_status().await;
        for line in status.pending.summary() {
            println!("  {line}");
        }
        return Ok(ExitCode::SUCCESS);
    }

    if args.revoke {
        host.revoke_trust().await?;
        println!("No longer reading config from {}.", workspace.display());
        return Ok(ExitCode::SUCCESS);
    }

    let status = host.trust_status().await;
    if status.trusted {
        println!("Trusted: {}", workspace.display());
        println!("Its own config is being read.");
    } else {
        println!("Not trusted: {}", workspace.display());
        println!("Its own config is not being read.");
    }

    if status.pending.is_empty() {
        println!("It has no config of its own.");
        return Ok(ExitCode::SUCCESS);
    }

    println!();
    println!(
        "{}:",
        if status.trusted {
            "What it contributes"
        } else {
            "What it would contribute"
        }
    );
    for line in status.pending.summary() {
        println!("  {line}");
    }
    // Named in full rather than counted, because a command line is the whole of
    // what someone can actually judge here.
    for command in &status.pending.mcp_commands {
        println!("    {command}");
    }

    // What is inside those files, for the half of them nobody can read by
    // looking. See `taurus_host::inspect`, including what this does not claim.
    if status.pending.has_findings() {
        println!();
        println!("Worth reading first:");
        for finding in &status.pending.findings {
            println!("  {} — {}", finding.path, finding.kind.label());
            println!("    {}", finding.detail);
        }
    }

    if !status.trusted {
        println!();
        println!("Read them first, then: taurus trust --allow");
    }

    Ok(ExitCode::SUCCESS)
}

/// One line on stderr when a workspace has config that is not being read.
///
/// Printed by every command that builds a host, and to stderr so it never
/// lands in a redirected `taurus run > answer.txt`. Silent in the two cases
/// that matter: a trusted workspace, and a workspace with nothing waiting.
/// A notice that appeared everywhere would be one nobody reads, and the whole
/// value of this gate is that its one message is worth stopping for.
pub fn notice(workspace: &Path) {
    let status = taurus_host::trust::status(workspace);
    if !status.decision_needed {
        return;
    }
    let mut what = status.pending.summary().join(", ");
    // A count, not the findings themselves. This line is printed by every
    // command and lands beside somebody's actual work — the place to read what
    // was found is `taurus trust`, and this is the sentence that sends them
    // there rather than the one that tries to be it.
    if status.pending.has_findings() {
        let n = status.pending.findings.len();
        what.push_str(&format!("; {n} worth reading first",));
    }
    eprintln!(
        "  note: not reading config from {} ({what}). Review it, then: taurus trust --allow",
        workspace.display()
    );
}
