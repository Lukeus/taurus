//! What Taurus makes of the notes actually on your disk, and what a save does.
//!
//! ```sh
//! cargo run -p taurus-host --example notes                    # list both notebooks
//! cargo run -p taurus-host --example notes -- .                # and this workspace's
//! cargo run -p taurus-host --example notes -- . 'Auth redesign' # read one
//! cargo run -p taurus-host --example notes -- --check           # exercise a save
//! ```
//!
//! Listing and reading write nothing. `--check` writes, and only ever inside a
//! temporary directory it makes and reports — never your real notebooks.
//!
//! # Why this exists rather than only the unit tests
//!
//! Two of the three things worth knowing about this feature cannot be seen from
//! a test, because a test supplies its own filesystem.
//!
//! The **notebooks are real directories somebody else can edit.** A note added
//! by hand, a file with the wrong extension, a directory that is not there yet, a
//! name the app would refuse but the filesystem accepted — every one of those is
//! ordinary, and every one of them is a shape the unit tests have to invent.
//! Listing what is actually there is the only way to find out what the app makes
//! of the notes you have.
//!
//! And **the compare-and-swap is about a race**, which a test can only simulate.
//! `--check` runs the real sequence against a real filesystem: read, write behind
//! the editor's back, save with the stale stamp, watch it refused with the other
//! version in hand, then save with the stamp the refusal handed back.

use std::path::{Path, PathBuf};

use taurus_host::config::Scope;
use taurus_host::notebook::{self, PageSaved};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--check") {
        check();
        return;
    }

    let workspace = args.first().map(PathBuf::from).and_then(|w| {
        w.canonicalize()
            .inspect_err(|e| eprintln!("{}: {e}", w.display()))
            .ok()
    });
    match args.get(1) {
        Some(name) => read(workspace.as_deref(), name),
        None => list(workspace.as_deref()),
    }
}

fn list(workspace: Option<&Path>) {
    for scope in [Scope::Workspace, Scope::Global] {
        let label = match scope {
            Scope::Workspace => "project",
            Scope::Global => "global",
        };
        match notebook::dir(scope, workspace) {
            None => println!("\n{label}: no workspace given, so there is no project notebook"),
            Some(dir) => {
                let there = dir.is_dir();
                println!(
                    "\n{label}: {}{}",
                    dir.display(),
                    if there { "" } else { "  (not there yet)" }
                );
            }
        }
    }

    let pages = notebook::list(workspace);
    if pages.is_empty() {
        println!("\nNo notes. The pane's + button writes the first one.");
        return;
    }

    println!(
        "\n{} note(s), newest first within each notebook:",
        pages.len()
    );
    for page in &pages {
        let scope = match page.scope {
            Scope::Workspace => "project",
            Scope::Global => "global ",
        };
        // The two things a listing cannot get from the filename, and the reason
        // `PageRef` carries them: how big it is and when it last changed.
        println!(
            "  {scope}  {:<40}  {:>7} bytes  {}",
            page.name, page.bytes, page.at
        );
    }

    // The check with no equivalent in the app: a name the filesystem accepted
    // that this would refuse, which is how a note ends up listed and unopenable.
    let refused: Vec<_> = pages
        .iter()
        .filter_map(|p| notebook::check(&p.name).err().map(|why| (&p.name, why)))
        .collect();
    if !refused.is_empty() {
        println!("\nListed but not openable — a name the app would not have written:");
        for (name, why) in refused {
            println!("  {name}: {why}");
        }
    }
}

fn read(workspace: Option<&Path>, name: &str) {
    for scope in [Scope::Workspace, Scope::Global] {
        match notebook::read(scope, workspace, name) {
            Ok(page) => {
                println!(
                    "\n--- {name} ({}) fingerprint {} ---\n{}",
                    match page.scope {
                        Scope::Workspace => "project",
                        Scope::Global => "global",
                    },
                    page.fingerprint,
                    page.text
                );
            }
            Err(why) => println!("\n{why}"),
        }
    }
}

/// The race, run for real.
fn check() {
    let home = std::env::temp_dir().join(format!("taurus-notes-check-{}", std::process::id()));
    // The notebook reads its global directory out of the config home, so
    // pointing that at a temp directory is what keeps this off the real one.
    std::env::set_var("TAURUS_HOME", &home);
    println!("Writing only inside {}\n", home.display());

    let made = notebook::create(Scope::Global, None, "Check").expect("start a note");
    println!("created  '{}'  fingerprint {}", made.name, made.fingerprint);
    assert_eq!(
        made.text, "# Check\n\n",
        "a new note opens with its own title"
    );

    // Somebody else — the model, git, another editor — writes it behind the
    // editor's back.
    let path = notebook::dir(Scope::Global, None).unwrap().join("Check.md");
    std::fs::write(&path, "# Check\n\nwritten by somebody else\n").expect("write behind it");
    println!("somebody else wrote the file");

    // The save the editor was going to make, with the stamp it is still holding.
    match notebook::save(
        Scope::Global,
        None,
        "Check",
        "# Check\n\nmine\n",
        &made.fingerprint,
    ) {
        Ok(PageSaved::Stale { current }) => {
            println!("refused, and handed back the other version:");
            println!("  {:?}", current.text);
            println!("  its fingerprint is {}", current.fingerprint);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "# Check\n\nwritten by somebody else\n",
                "a refused save must not have written anything"
            );

            // Keep mine: the same text, against the stamp the refusal gave back.
            match notebook::save(
                Scope::Global,
                None,
                "Check",
                "# Check\n\nmine\n",
                &current.fingerprint,
            ) {
                Ok(PageSaved::Written { page }) => {
                    println!("\nwritten  {:?}", page.text);
                    assert_eq!(std::fs::read_to_string(&path).unwrap(), page.text);
                }
                other => panic!("the second save should have landed: {other:?}"),
            }
        }
        other => panic!("the stale save should have been refused: {other:?}"),
    }

    // And the guard, which is the reason a name is checked before it is a path.
    for name in ["../escape", "../../escape", "Bad/name"] {
        let why = notebook::read(Scope::Global, None, name).unwrap_err();
        println!("\nrefused '{name}': {why}");
    }

    std::fs::remove_dir_all(&home).ok();
    println!("\nAll of it held. Cleaned up.");
}
