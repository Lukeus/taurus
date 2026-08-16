//! A turn recorded, read back as a diff, and committed — against real git.
//!
//! The unit tests cover each half separately with a fixture in the middle. This
//! runs the whole path the **Changes** drawer runs: a checkpoint log written by
//! the same recorder the tools write through, read back by
//! `CheckpointStore::changes`, and handed to `Repo::commit` in a repository
//! created by the git binary on this machine.
//!
//! ```sh
//! cargo run -p taurus-host --example turn
//! ```
//!
//! Needs git and no provider. It builds its own repository in a temporary
//! directory and takes it away again, so it touches nothing you own.

use std::path::Path;

use taurus_host::git::Repo;
use taurus_tools::checkpoint::TurnChange;
use taurus_tools::CheckpointStore;

#[tokio::main]
async fn main() {
    let dir = tempfile::TempDir::new().expect("a temp dir");
    let root = dir.path().canonicalize().expect("a real path");
    let logs = tempfile::TempDir::new().expect("a temp dir for logs");
    let store = CheckpointStore::new(logs.path());

    setup(&root).await;
    println!("workspace: {}\n", root.display());

    // Turn 1: the seed commit's worth of files, so there is a HEAD to diff
    // against and one tracked file for turn 2 to change.
    write(&root, "src/widget.rs", "pub struct Widget;\n");
    write(&root, ".gitignore", ".env\n");
    let repo = Repo::discover(&root)
        .await
        .expect("git must be available")
        .expect("the example just created a repository");
    repo.commit(
        &["src/widget.rs".into(), ".gitignore".into()],
        "seed the example",
    )
    .await
    .expect("the seed commit");

    // Turn 2: what a real turn looks like — one file rewritten, one created,
    // one deleted, and one that git is ignoring on purpose.
    write(&root, "doomed.rs", "// removed below\n");
    let recorder = store.begin_turn("example", &root, "rename Widget to Gadget");
    for file in ["src/widget.rs", "src/gadget.rs", "doomed.rs", ".env"] {
        recorder.capture(&root.join(file)).await;
    }
    write(&root, "src/widget.rs", "pub struct Gadget;\n");
    write(&root, "src/gadget.rs", "pub use crate::widget::Gadget;\n");
    write(&root, ".env", "SECRET=1\n");
    std::fs::remove_file(root.join("doomed.rs")).expect("remove");

    let turns = store.turns("example").expect("the log reads back");
    let turn = turns.last().expect("one turn was recorded");
    println!(
        "turn {} — {:?}, {} files",
        turn.turn,
        turn.prompt,
        turn.files.len()
    );

    // What the drawer draws when a turn is expanded.
    for change in store
        .changes("example", &root, turn.turn)
        .expect("the diff builds")
    {
        match change {
            TurnChange::Diff { diff } => println!(
                "  {:<8} {:<22} +{} −{}",
                verb(&diff),
                diff.path,
                diff.added,
                diff.removed
            ),
            TurnChange::Opaque { path, reason } => {
                println!("  {:<8} {path} — {reason}", "not shown")
            }
        }
    }

    // And what the button does. `.env` is ignored, so it must come back named
    // rather than silently absent.
    let commit = repo
        .commit(&turn.files, "rename Widget to Gadget")
        .await
        .expect("the commit");
    println!("\ncommitted {} — {}", commit.sha, commit.subject);
    for file in &commit.files {
        println!("  in       {file}");
    }
    for skipped in &commit.skipped {
        println!("  left out {} — {}", skipped.path, skipped.reason);
    }

    // Proof it landed in the repository rather than only in the return value.
    println!("\ngit log --oneline:");
    for line in git(&root, &["log", "--oneline"]).await.lines() {
        println!("  {line}");
    }
    println!("\ngit status --porcelain (should hold only the ignored file's absence):");
    let left = git(&root, &["status", "--porcelain"]).await;
    println!(
        "  {}",
        if left.trim().is_empty() {
            "clean"
        } else {
            left.trim()
        }
    );
}

fn verb(diff: &taurus_tools::FileDiff) -> &'static str {
    if diff.deleted {
        "delete"
    } else if diff.created {
        "create"
    } else {
        "replace"
    }
}

fn write(root: &Path, name: &str, contents: &str) {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, contents).expect("write");
}

/// Identity is set locally so the example does not depend on — or disturb —
/// whatever this machine's git config says.
async fn setup(root: &Path) {
    for args in [
        vec!["init", "--initial-branch", "main"],
        vec!["config", "user.email", "example@taurus.invalid"],
        vec!["config", "user.name", "Taurus Example"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        git(root, &args).await;
    }
}

async fn git(root: &Path, args: &[&str]) -> String {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .await
        .unwrap_or_else(|e| panic!("git {}: {e}", args.join(" ")));
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
