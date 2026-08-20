fn main() {
    ensure_conpty_runtime();
    tauri_build::build()
}

/// Puts Microsoft's ConPTY runtime on disk before Tauri looks for it.
///
/// `tauri.windows.conf.json` declares those files as bundle resources, and
/// `tauri_build::build` checks that every declared resource exists — at
/// *compile* time, not at bundle time. So a Windows build fails without them
/// even when it is a plain `cargo test` that will never produce a bundle. That
/// is not obvious from the config, and the error it produces (`resource path
/// 'conpty\conpty.dll' doesn't exist`) names the symptom rather than the fix.
///
/// Doing it here rather than as a CI step is what makes that true everywhere:
/// a contributor's first `cargo build` on Windows works with nothing explained
/// to them, and no workflow can forget a step it does not have. See
/// `scripts/conpty.mjs` for why the files are shipped at all.
///
/// Keyed on the *target* rather than the host, so cross-compiling to Windows
/// fetches them too — and the script is told to run regardless of the platform
/// it finds itself on, because this function has already decided.
fn ensure_conpty_runtime() {
    println!("cargo:rerun-if-changed=../scripts/conpty.mjs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let script = std::path::Path::new("../scripts/conpty.mjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .env("TAURUS_CONPTY_FORCE", "1")
        .output();

    match output {
        Ok(output) if output.status.success() => {}
        Ok(output) => panic!(
            "could not fetch the ConPTY runtime that the Windows bundle needs.\n{}\n{}\n\
             Run `node scripts/conpty.mjs` to see the failure on its own.",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        ),
        // The likeliest cause by a wide margin, and worth saying plainly: the
        // Rust build of a Tauri app needs Node anyway, but nothing else in this
        // crate's build says so.
        Err(e) => panic!(
            "could not run `node scripts/conpty.mjs`, which fetches the ConPTY runtime the \
             Windows bundle needs: {e}\nNode is required to build this app."
        ),
    }
}
