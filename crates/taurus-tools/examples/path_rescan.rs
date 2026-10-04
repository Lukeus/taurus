//! Reading your login shell's PATH at startup and again later, against your
//! real shell, the way a Dock launch sees it.
//!
//! ```sh
//! cargo run -p taurus-tools --example path_rescan
//! ```
//!
//! Starts from launchd's PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), runs the
//! startup probe, then [`rescan`](taurus_tools::login_path::rescan) — what
//! Reconnect does — and checks three things: the rescan worked against your
//! real shell and profile, a child started afterwards is handed the PATH, and
//! the process environment wasn't touched after startup. It runs your
//! `.zshrc` (or equivalent) twice, and writes nothing.
use taurus_tools::login_path;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // What a window started from the Dock inherits. Set before anything else
    // runs, the way startup's own write is.
    std::env::set_var("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");

    let startup = login_path::adopt();
    println!(
        "startup: added {} directories{}",
        startup.added.len(),
        startup
            .skipped
            .as_deref()
            .map(|why| format!(" (skipped: {why})"))
            .unwrap_or_default()
    );
    let env_after_startup = std::env::var("PATH").unwrap();

    let started = std::time::Instant::now();
    let again = login_path::rescan();
    println!(
        "rescan:  {} directories searched, {} added since launch, in {:.0?}{}",
        login_path::entries().len(),
        again.added.len(),
        started.elapsed(),
        again
            .skipped
            .as_deref()
            .map(|why| format!(" (skipped: {why})"))
            .unwrap_or_default()
    );
    assert!(
        again.skipped.is_none(),
        "your shell didn't answer the rescan"
    );

    let mut child = tokio::process::Command::new("sh");
    let out = login_path::apply(&mut child)
        .args(["-c", "printf %s \"$PATH\""])
        .output()
        .await
        .expect("sh runs");
    let child_path = String::from_utf8_lossy(&out.stdout);
    assert_eq!(child_path, again.path, "a child gets the PATH read again");
    assert_eq!(
        std::env::var("PATH").unwrap(),
        env_after_startup,
        "the rescan never writes the process environment"
    );
    println!("child:   handed the PATH read again; process environment untouched");
}
