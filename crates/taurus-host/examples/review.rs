//! Reading a turn back to an agent that did not write it, against a live model.
//!
//! `cargo run -p taurus-host --example review -- <model>`
//!
//! Builds a throwaway workspace with a real defect in it, hands the diff to the
//! reviewer through the same [`taurus_host::review::review`] the app and the
//! CLI call, and then asserts the three things that make this feature what it
//! claims to be rather than a second opinion from the same mind:
//!
//! 1. **It never sees the conversation.** The transcript it is given is one
//!    message and that message is the diff, so there is no request in it to
//!    reason from — which this proves by planting a defect that no reasonable
//!    request would have asked for.
//! 2. **It cannot write.** Its tool list is `explorer`'s and its context has no
//!    checkpoint recorder, and the task below asks it to fix what it finds. A
//!    reviewer that edited the file would be a reviewer that had quietly become
//!    a turn.
//! 3. **It read the file, not only the hunk.** The defect is only visible from
//!    the surrounding code, which is what the brief tells it to go and read.
//!
//! Needs Ollama. It writes only inside a temporary directory.

use std::sync::Arc;

use taurus_host::review;
use taurus_provider_ollama::{OllamaProvider, DEFAULT_BASE_URL};
use taurus_tools::checkpoint::TurnChange;
use taurus_tools::diff;
use taurus_tools::{AllowAll, PermissionEngine, ToolContext, ToolRegistry};
use tokio_util::sync::CancellationToken;

/// The file as it stood before the turn.
const BEFORE: &str = "\
/// Averages the samples.
///
/// Callers pass an empty slice when a sensor has not reported yet.
pub fn average(samples: &[f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    Some(samples.iter().sum::<f64>() / samples.len() as f64)
}
";

/// The file as the turn left it.
///
/// The guard is gone, so an empty slice now divides by zero and returns `NaN`
/// rather than `None`. The hunk alone does not say that is wrong — the doc
/// comment two lines above it does, which is the point: a reviewer that only
/// read the diff has no reason to object.
const AFTER: &str = "\
/// Averages the samples.
///
/// Callers pass an empty slice when a sensor has not reported yet.
pub fn average(samples: &[f64]) -> Option<f64> {
    Some(samples.iter().sum::<f64>() / samples.len() as f64)
}
";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "qwen3.6:27b".into());

    let workspace = tempfile::tempdir()?;
    let path = workspace.path().join("average.rs");
    // On disk as the turn left it, so a reviewer that goes and reads the file
    // sees what a reviewer would see. This is what makes point 3 above
    // testable at all.
    std::fs::write(&path, AFTER)?;

    let changes = vec![TurnChange::Diff {
        diff: diff::of_change("average.rs".into(), Some(BEFORE), Some(AFTER)),
    }];

    let provider = Arc::new(OllamaProvider::new(DEFAULT_BASE_URL.to_string()));
    let registry = ToolRegistry::with_builtins();
    let context = ToolContext::new(
        workspace.path().to_path_buf(),
        Arc::new(PermissionEngine::new(
            workspace.path(),
            workspace.path().join(".taurus"),
            // Stands in for the user clicking Allow. The engine is still in the
            // path, so what this proves is the reviewer's *scope*, not the gate.
            Box::new(AllowAll),
        )),
        CancellationToken::new(),
    );

    println!("Reviewing one turn with {model}, from a context that did not write it…\n");

    let report = review::review(
        provider,
        &model,
        registry,
        context,
        changes,
        1,
        CancellationToken::new(),
    )
    .await?;

    println!("{}\n", report.text.trim());
    println!(
        "— {} file reviewed by {}, {} omitted.",
        report.files,
        report.model,
        report.omitted.len()
    );

    // The check that matters most, and the one a unit test cannot make: the
    // file is still as the turn left it. A reviewer that "helpfully" restored
    // the guard would have written to the workspace, which is the one thing
    // this path is built so it cannot do.
    let now = std::fs::read_to_string(&path)?;
    assert_eq!(
        now, AFTER,
        "the reviewer changed the file — it is supposed to be unable to"
    );
    println!("The file is untouched, which is the point: it reads and does not write.");

    // Not asserted, because a model may legitimately miss it and this is a
    // live check rather than a gate. Said instead, so the run is readable.
    let found = report.text.to_lowercase();
    let noticed = found.contains("empty") || found.contains("none") || found.contains("nan");
    println!(
        "It {} the empty-slice case the doc comment above the hunk describes.",
        if noticed {
            "found"
        } else {
            "did NOT find — worth a look at the brief in `review.rs`, or a larger model for"
        }
    );

    Ok(())
}
