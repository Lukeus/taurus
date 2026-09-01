//! What is inside the config a workspace is asking you to trust.
//!
//! ```sh
//! cargo run -p taurus-host --example inspect -- .
//! cargo run -p taurus-host --example inspect -- ~/src/some-clone
//! ```
//!
//! Needs no provider. It reads the workspace's own config layer and writes
//! nothing, starts nothing, and loads nothing.
//!
//! Two questions this answers that the app cannot. The first is "why is this
//! flagged" — a finding names a file and a line, and the fastest way to judge a
//! heuristic is to run it over a directory you already trust and see whether it
//! stays quiet. The second is "what is in this repository", asked before the
//! app has ever been pointed at it: the banner only appears once a workspace is
//! open, and the moment you want this is the moment after `git clone` and
//! before anything has read a line of it.
//!
//! A clean run is the ordinary result and the one worth checking for. If this
//! finds something in every repository on your disk, the rules are wrong — see
//! `taurus_host::inspect` for the two false positives they are written around.

use std::path::PathBuf;

use taurus_host::inspect;
use taurus_host::trust;

fn main() {
    let workspace = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let workspace = workspace.canonicalize().unwrap_or(workspace);

    println!("{}", workspace.display());

    // The counts first, from the gate itself rather than recomputed here: what
    // this example reports and what the banner reports have to be the same
    // answer, or it is answering a question nobody asked.
    let status = trust::status(&workspace);
    if status.pending.is_empty() {
        println!("  No config of its own. Nothing here would be read, and");
        println!("  nothing is scanned — see `trust::pending`.");
        return;
    }

    println!(
        "  {} ({})",
        status.pending.summary().join(", "),
        if status.trusted {
            "trusted, so this is being read"
        } else {
            "not trusted, so none of it is being read"
        }
    );
    for command in &status.pending.mcp_commands {
        println!("    {command}");
    }

    let findings = inspect::inspect(&workspace);
    println!();
    if findings.is_empty() {
        println!("Nothing worth pointing out.");
        println!();
        println!("Which is not the same as safe. This reads configuration, not");
        println!("behaviour: what running the project's own build does is a");
        println!("different question, and the permission prompt is where it is");
        println!("asked. See docs/known-gaps.md.");
        return;
    }

    println!("{} worth reading first:", findings.len());
    for finding in &findings {
        println!();
        println!("  {}", finding.path);
        println!("    {} — {}", finding.kind.label(), finding.detail);
    }

    println!();
    println!("None of these is a verdict. They are the parts of those files");
    println!("worth your eyes, including the ones you could not see by looking.");
    if findings.len() >= inspect::MAX_FINDINGS {
        println!();
        println!(
            "Stopped at {} findings — there may be more.",
            inspect::MAX_FINDINGS
        );
    }
}
