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
//! It writes nothing to the workspace. The checkpoint log it records into is a
//! temporary directory that goes away when the process does.

use std::path::PathBuf;
use std::time::Instant;

use taurus_tools::{sweep::Sweep, CheckpointStore};

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

    let started = Instant::now();
    let sweep = Sweep::before(&root).await;
    let indexed = started.elapsed();
    println!("  index:   {indexed:>8.1?}");

    // Nothing runs in between, so a correct sweep finds nothing. Anything it
    // reports here is either a background process writing into the workspace or
    // a bug in the comparison.
    let started = Instant::now();
    let change = sweep.after(&root, &recorder).await;
    println!("  compare: {:>8.1?}", started.elapsed());

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
