//! A command that outlives the call that started it, end to end.
//!
//! ```sh
//! cargo run -p taurus-tools --example background
//! ```
//!
//! Needs no provider. It drives the registry the way the agent loop does, in a
//! throwaway workspace, and checks the two things the unit tests can only
//! assert one moment of:
//!
//! - a command started in one call, read in another, and *undone from the file
//!   as it stood before it ran* — the pre-image the job carried across the
//!   minutes in between;
//! - a command that would never stop on its own, stopped;
//! - a command read by the window while it runs, and then read *in full* by
//!   `check_command` afterwards — the two cursors, which is what keeps a pane
//!   drawing a build from emptying the buffer the model was going to read.
//!
//! Watch the timings. The start returns immediately, the roster shows the
//! command running while this program is doing something else, and the check
//! returns the moment the command exits rather than when its wait runs out.

use std::sync::Arc;
use std::time::{Duration, Instant};

use taurus_tools::permission::{AllowAll, PermissionEngine};
use taurus_tools::{CheckpointStore, Jobs, ToolContext, ToolRegistry};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let dir = tempfile::TempDir::new().expect("a temp workspace");
    let root = dir.path().canonicalize().expect("a real path");
    std::fs::write(root.join("a.txt"), "original").unwrap();

    let logs = tempfile::TempDir::new().unwrap();
    let store = CheckpointStore::new(logs.path());
    let permissions = Arc::new(PermissionEngine::new(
        &root,
        root.join(".taurus"),
        Box::new(AllowAll),
    ));
    // Held rather than handed straight over: the window reads through this
    // while the registry reads through the context, which is the whole point
    // of the last section below.
    let jobs = Arc::new(Jobs::new());
    let ctx = ToolContext::new(root.clone(), permissions, CancellationToken::new())
        .with_jobs(jobs.clone())
        .with_checkpoints(store.begin_turn("live", &root, "a long command"));
    let registry = ToolRegistry::with_builtins();

    println!("workspace {}\n", root.display());

    let slow = if cfg!(windows) {
        "ping -n 4 127.0.0.1 > nul & echo rewritten > a.txt"
    } else {
        "sleep 3; echo rewritten > a.txt"
    };

    let clock = Instant::now();
    let started = registry
        .execute(
            "run_command",
            serde_json::json!({"command": slow, "background": true}),
            &ctx,
        )
        .await
        .expect("it starts");
    println!(
        "[{:>5.1}s] {}\n",
        clock.elapsed().as_secs_f32(),
        started.to_text()
    );

    // While it runs, the workspace is untouched and the roster says so.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let roster = registry
        .execute("check_command", serde_json::json!({}), &ctx)
        .await
        .expect("a roster");
    println!(
        "[{:>5.1}s] {}",
        clock.elapsed().as_secs_f32(),
        roster.to_text()
    );
    println!(
        "[{:>5.1}s] a.txt is still {:?}\n",
        clock.elapsed().as_secs_f32(),
        std::fs::read_to_string(root.join("a.txt")).unwrap()
    );

    // Waiting returns when the command exits, not when the wait expires.
    let checked = registry
        .execute(
            "check_command",
            serde_json::json!({"id": 1, "wait_secs": 60}),
            &ctx,
        )
        .await
        .expect("a report");
    println!(
        "[{:>5.1}s] {}\n",
        clock.elapsed().as_secs_f32(),
        checked.to_text()
    );

    let turn = store.turns("live").unwrap().into_iter().next();
    let files = turn.map(|t| t.files).unwrap_or_default();
    println!("the turn's changed files: {files:?}");
    assert_eq!(
        files,
        vec!["a.txt"],
        "the finished command was not recorded"
    );

    store.rewind("live", &root, 1, false).expect("a rewind");
    let restored = std::fs::read_to_string(root.join("a.txt")).unwrap();
    println!("after rewinding: {restored:?}");
    assert_eq!(
        restored, "original",
        "the pre-image was taken after the command wrote, not before"
    );

    // And one that would never stop on its own.
    let forever = if cfg!(windows) {
        "ping -t 127.0.0.1 > nul"
    } else {
        "while true; do sleep 1; done"
    };
    registry
        .execute(
            "run_command",
            serde_json::json!({"command": forever, "background": true}),
            &ctx,
        )
        .await
        .expect("it starts");
    let stopped = registry
        .execute("stop_command", serde_json::json!({"id": 2}), &ctx)
        .await
        .expect("it stops");
    println!(
        "\n[{:>5.1}s] {}",
        clock.elapsed().as_secs_f32(),
        stopped.to_text()
    );

    let after = registry
        .execute("check_command", serde_json::json!({}), &ctx)
        .await
        .expect("a roster");
    println!(
        "[{:>5.1}s] {}",
        clock.elapsed().as_secs_f32(),
        after.to_text()
    );
    assert!(
        after.to_text().contains("stopped after"),
        "the command outlived the stop"
    );

    // Two readers of one command.
    //
    // The window polls with its own cursor while the command runs, and what it
    // takes is not taken from anybody: the check afterwards gets every line,
    // including the ones already drawn on screen.
    let chatty = if cfg!(windows) {
        "echo one & ping -n 2 127.0.0.1 > nul & echo two & ping -n 2 127.0.0.1 > nul & echo three"
    } else {
        "echo one; sleep 1; echo two; sleep 1; echo three"
    };
    let started = registry
        .execute(
            "run_command",
            serde_json::json!({"command": chatty, "background": true}),
            &ctx,
        )
        .await
        .expect("it starts");
    println!(
        "\n[{:>5.1}s] {}",
        clock.elapsed().as_secs_f32(),
        started.to_text()
    );

    let id = 3;
    let mut cursor = 0;
    let mut on_screen = String::new();
    for _ in 0..12 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let seen = jobs.read(id, cursor).expect("a reading");
        cursor = seen.cursor;
        if !seen.text.is_empty() {
            print!("{}", seen.text);
            on_screen.push_str(&seen.text);
        }
        if jobs.list().iter().any(|job| job.id == id && !job.running) {
            break;
        }
    }
    println!(
        "[{:>5.1}s] the window read {} bytes",
        clock.elapsed().as_secs_f32(),
        on_screen.len()
    );

    let report = registry
        .execute("check_command", serde_json::json!({"id": id}), &ctx)
        .await
        .expect("a report");
    let checked = report.to_text();
    println!("[{:>5.1}s] {checked}", clock.elapsed().as_secs_f32());

    for line in ["one", "two", "three"] {
        assert!(
            on_screen.contains(line),
            "the window never saw {line:?}: {on_screen:?}"
        );
        assert!(
            checked.contains(line),
            "the window took {line:?} out of the model's copy: {checked}"
        );
    }

    println!("\nok");
}
