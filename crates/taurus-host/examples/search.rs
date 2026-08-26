//! What searching your real transcripts costs, and what it finds.
//!
//! ```sh
//! cargo run -p taurus-host --example search -- "trust banner" [workspace]
//! ```
//!
//! It reads `~/.taurus/sessions` — your actual conversations — and writes
//! nothing. With no workspace it searches every one of them, which is both the
//! slowest case and the one worth timing: the palette calls this while
//! somebody is typing, so the number that matters is the whole-history search,
//! not the one against a fresh project with four transcripts in it.
//!
//! Two numbers are reported and the gap between them is the point. A search
//! for something that appears nowhere pays only the prefilter — every file
//! read, nothing parsed — and is the floor. A search that hits pays to rebuild
//! each conversation that matched. If those two are close on a large history,
//! the prefilter in `sessions::mentions` is not doing its job, and the palette
//! will feel like it stutters a word behind the typing.

use std::path::PathBuf;
use std::time::Instant;

use taurus_host::search;
use taurus_host::sessions;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(query) = args.next() else {
        eprintln!("usage: search <query> [workspace]");
        std::process::exit(2);
    };
    let workspace = args.next().map(PathBuf::from);

    let listed = sessions::list(workspace.as_deref());
    println!(
        "{} conversation{} in {}\n",
        listed.len(),
        if listed.len() == 1 { "" } else { "s" },
        workspace
            .as_ref()
            .map(|w| w.display().to_string())
            .unwrap_or_else(|| "every workspace".into()),
    );
    if listed.is_empty() {
        println!("Nothing to search. Have a conversation first.");
        return;
    }

    // The floor: a query nothing can match, so every file is read and none is
    // parsed. Deliberately not a word — a random string might genuinely occur.
    let miss = Instant::now();
    let nothing = search::search(workspace.as_deref(), "zqxjkvwpbmgf");
    let miss = miss.elapsed();
    assert!(
        nothing.sessions.is_empty(),
        "that string should match nothing"
    );
    println!("prefilter only   {miss:>8.1?}   (nothing matched, nothing parsed)");

    let hit = Instant::now();
    let found = search::search(workspace.as_deref(), &query);
    let hit = hit.elapsed();
    println!(
        "searching {query:?}   {hit:>8.1?}   ({} matched)\n",
        found.sessions.len() + found.more as usize
    );

    if found.sessions.is_empty() {
        println!("No conversation mentions that.");
        return;
    }

    for entry in &found.sessions {
        println!(
            "{}  {}",
            entry.session.id,
            if entry.session.title.is_empty() {
                "(no turns)"
            } else {
                &entry.session.title
            }
        );
        println!(
            "  {} hit{}",
            entry.hits,
            if entry.hits == 1 { "" } else { "s" }
        );
        for m in &entry.matches {
            // Printed with the mark the palette would draw, so a wrong offset
            // is visible here rather than only in the window.
            let units: Vec<u16> = m.excerpt.encode_utf16().collect();
            let before = String::from_utf16_lossy(&units[..m.from as usize]);
            let marked = String::from_utf16_lossy(&units[m.from as usize..m.to as usize]);
            let after = String::from_utf16_lossy(&units[m.to as usize..]);
            println!("    {:<9} {before}[{marked}]{after}", m.role);
        }
        println!();
    }

    if found.more > 0 {
        println!("{} more matched and are not listed.", found.more);
    }
}
