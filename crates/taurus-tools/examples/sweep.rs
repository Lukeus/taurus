//! What a sweep costs, and what it sees, on a real workspace.
//!
//! Every `run_command` pays this twice — once to index, once to compare — so
//! the number matters more than most. Run it against something large before
//! changing the caps in `sweep.rs`.
//!
//! ```sh
//! cargo run -p taurus-tools --example sweep -- [path]
//! ```
//!
//! It reports two commands rather than one, because a turn is rarely one
//! command and the second is the one that shows what the cache is for: the
//! first read the workspace, and the second should open almost none of it. If
//! the two are the same number, the cache is not working.
//!
//! It writes nothing to the workspace. The checkpoint log it records into is a
//! temporary directory that goes away when the process does.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use taurus_tools::{sweep::Sweep, CheckpointStore, SweepCache};

#[tokio::main]
async fn main() {
    let root: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| ".".into())
        .parse()
        .expect("a path");
    let root = root
        .canonicalize()
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()));

    let logs = tempfile::TempDir::new().expect("a temp dir");
    let store = CheckpointStore::new(logs.path());
    let recorder = store.begin_turn("example", &root, "measure a sweep");

    println!("workspace: {}", root.display());

    // Once, unreported, so the three below are comparable. Without it the
    // first one carries the cost of pulling a cold workspace into the operating
    // system's own page cache, and reads as several times slower than a sweep
    // with no cache at all — which is the opposite of what is true.
    Sweep::before(&root, None).await;

    // As a turn holds one: shared by every command in it.
    let cache = Arc::new(SweepCache::new());

    let mut change = None;
    for (label, cache) in [
        ("first command ", Some(Arc::clone(&cache))),
        ("second command", Some(Arc::clone(&cache))),
        ("with no cache ", None),
    ] {
        let started = Instant::now();
        let sweep = Sweep::before(&root, cache).await;
        let indexed = started.elapsed();

        // Nothing runs in between, so a correct sweep finds nothing. Anything
        // reported here is either a background process writing into the
        // workspace or a bug in the comparison.
        let started = Instant::now();
        let found = sweep.after(&root, &recorder).await;
        println!(
            "  {label}   index {indexed:>8.1?}   compare {:>8.1?}",
            started.elapsed()
        );
        change = Some(found);
    }

    let change = change.expect("three sweeps ran");

    match change.summary() {
        None => println!("\nnothing changed, which is the right answer"),
        Some(line) => {
            println!("\n{line}");
            for file in &change.files {
                println!("  {file}");
            }
        }
    }
}
