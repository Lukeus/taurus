//! Whether asking after a stored key raises the keychain dialog.
//!
//! ```sh
//! cargo run -p taurus-host --example keychain                  # the selected search backend
//! cargo run -p taurus-host --example keychain -- search:brave openai
//! cargo run -p taurus-host --example keychain -- --read        # and read it, which may ask
//! ```
//!
//! It reads your real credential store and writes nothing. Never prints a key.
//!
//! The check a unit test cannot make, because the suite runs on an in-memory
//! store: on macOS, reading a secret is what raises "wants to use your
//! confidential information", and a binary the keychain has not been told to
//! trust gets that dialog on every read. Every `cargo run` is such a binary —
//! a rebuild changes the signature — so this is exactly the state a window
//! finds itself in the first time it loads after an update.
//!
//! The startup reload used to read the search key, and a dialog nobody had
//! noticed held the window at its ten-second status wait and hung a folder
//! switch outright. It now asks only whether a key is there. So the thing to
//! watch for is the first column: `present` must come back in milliseconds with
//! no dialog on screen. If a dialog appears before it prints, the attribute
//! lookup in `secrets::backend::exists` has started touching the secret, and
//! the stall is back.
//!
//! `--read` then reads each key the way the first search does, timed, so the
//! difference between the two questions is on screen. Expect the dialog here.

use std::time::Instant;

use taurus_host::{config, secrets};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let read = args.iter().any(|a| a == "--read");
    let mut ids: Vec<String> = args.into_iter().filter(|a| a != "--read").collect();

    if ids.is_empty() {
        match config::load_global_search().backend {
            Some(backend) if !backend.is_empty() => ids.push(config::search_key_id(&backend)),
            _ => {
                eprintln!(
                    "No search backend is selected in ~/.taurus/search.json, so there is no \
                     key to ask after. Name one: keychain -- search:<backend> or a provider id."
                );
                std::process::exit(2);
            }
        }
    }

    println!("{:<28} {:>8} {:>10}", "id", "present", "took");
    for id in &ids {
        let started = Instant::now();
        let present = secrets::exists(id);
        let took = started.elapsed();
        println!(
            "{id:<28} {:>8} {took:>10.1?}",
            if present { "yes" } else { "no" }
        );
    }

    if !read {
        return;
    }

    println!("\n{:<28} {:>8} {:>10}", "id", "read", "took");
    for id in &ids {
        let started = Instant::now();
        let found = secrets::stored(id).is_some();
        let took = started.elapsed();
        // Found or not, never the value.
        println!(
            "{id:<28} {:>8} {took:>10.1?}",
            if found { "a key" } else { "nothing" }
        );
    }
}
