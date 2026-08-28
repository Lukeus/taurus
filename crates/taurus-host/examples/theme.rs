//! What Taurus makes of the themes actually on your disk.
//!
//! ```sh
//! cargo run -p taurus-host --example theme            # global themes
//! cargo run -p taurus-host --example theme -- .       # and this workspace's
//! ```
//!
//! It reads `~/.taurus/themes` and, with a workspace, that folder's
//! `.taurus/themes` as well. It writes nothing.
//!
//! This is the answer to "why is my theme not in the picker", which is the one
//! question this feature will be asked most and the one the app is worst
//! placed to answer — a file that will not parse is, by construction, a file
//! that has nothing to say for itself on screen beyond a line in Settings.
//! Here it says the whole thing: which files were found, which layer each came
//! from, which of the two palettes it can paint, which `--lk-*` tokens it will
//! actually set, whether its logo was read, and every complaint the loader had
//! on the way past.
//!
//! The last of those is the part with no equivalent anywhere else. The app
//! reports the problems of the theme *in force*; the picker reports the rest
//! only while it is open. From a terminal you get all of them at once, before
//! the app has been started.

use std::path::PathBuf;

use taurus_host::theme::{self, COLORS, FONTS};

fn main() {
    let workspace = std::env::args().nth(1).map(PathBuf::from);
    let where_from = workspace
        .as_ref()
        .map(|w| format!("~/.taurus/themes and {}/.taurus/themes", w.display()))
        .unwrap_or_else(|| "~/.taurus/themes".into());
    println!("reading {where_from}\n");

    let (themes, problems) = theme::load_themes(workspace.as_deref());

    if themes.is_empty() {
        println!("No themes. The app paints its own palette, which is not a fault.");
        println!("Writing one: docs/configuration.md#themes\n");
    }

    for theme in &themes {
        let modes = match theme.modes {
            theme::ThemeModes::Both => "dark and light",
            theme::ThemeModes::DarkOnly => "dark only — selecting it pins the mode",
            theme::ThemeModes::LightOnly => "light only — selecting it pins the mode",
        };
        println!("{} ({})", theme.name, theme.id);
        println!("  from    {:?} · {}", theme.scope, theme.path);
        println!("  paints  {modes}");

        for (label, palette) in [("dark", &theme.dark), ("light", &theme.light)] {
            if palette.is_empty() {
                continue;
            }
            println!("  {label}");
            // The token each colour actually sets. A theme names the *job* and
            // the stylesheet names the colour, so this mapping is the one part
            // of a theme file that cannot be checked by reading it.
            for (name, token) in COLORS {
                if let Some(value) = palette.get(*name) {
                    println!("    {name:<14} {value:<10} -> {token}");
                }
            }
        }

        let fonts = [
            (theme.fonts.display.as_deref(), FONTS[0].1),
            (theme.fonts.body.as_deref(), FONTS[1].1),
            (theme.fonts.mono.as_deref(), FONTS[2].1),
        ];
        for (family, token) in fonts {
            if let Some(family) = family {
                // Named rather than checked: whether it is installed is a
                // question only the window can answer, and one it answers
                // silently by falling back.
                println!("  font    {family} -> {token} (must be installed on this machine)");
            }
        }

        match (&theme.wordmark, &theme.logo) {
            (None, None) => {}
            (word, logo) => {
                println!(
                    "  brand   wordmark {} · logo {}",
                    match word.as_deref() {
                        None => "unchanged".to_string(),
                        Some("") => "none, mark only".to_string(),
                        Some(w) => format!("{w:?}"),
                    },
                    match logo {
                        // The inlined length, because that is what travels
                        // with every status the window is pushed. In bytes
                        // under a kilobyte: a wordmark SVG is often 300 bytes,
                        // and "0KB" reads like it failed.
                        Some(uri) if uri.len() < 1024 => {
                            format!("read, {} bytes inlined", uri.len())
                        }
                        Some(uri) => format!("read, {}KB inlined", uri.len() / 1024),
                        None => "none".to_string(),
                    }
                );
            }
        }

        let shape = &theme.shape;
        if shape.radius.is_some() || shape.gutter.is_some() || shape.rail_gutter.is_some() {
            println!(
                "  shape   radius {} · gutter {} · rail-gutter {}",
                shape
                    .radius
                    .map_or("as shipped".into(), |r| format!("{r}x")),
                shape
                    .gutter
                    .map_or("as shipped".into(), |g| format!("{g}px")),
                shape
                    .rail_gutter
                    .map_or("as shipped".into(), |g| format!("{g}px")),
            );
        }
        println!();
    }

    if problems.is_empty() {
        println!("Nothing to report.");
    } else {
        println!("{} problem{}:", problems.len(), plural(problems.len()));
        for problem in &problems {
            println!("  {problem}");
        }
        println!(
            "\nA file that will not parse costs itself and nothing else — everything above still loads."
        );
    }

    // Contrast is deliberately not scored here. It is checked in the editor,
    // against the palette *resolved* over whichever of the two the app ships,
    // and the stylesheet those defaults live in is not something Rust can
    // read. A second table of pairs here would be a second thing to keep in
    // step with `src/lib/contrast.ts`, and the one that drifted would be this
    // one — because nothing looks at it as often.
    println!("\nContrast is checked in Settings › Appearance, where the palette can be");
    println!("resolved against the one the app ships.");
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}
