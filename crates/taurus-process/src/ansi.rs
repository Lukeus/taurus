//! Terminal control sequences, taken out of what a process printed.

/// Removes terminal control sequences from output.
///
/// The cost of asking for a terminal: the program now writes colour, cursor
/// moves, and progress-bar redraws, none of which mean anything to a model
/// reading a transcript and all of which it pays tokens for. A `cargo build`
/// under a pty is more escape bytes than text.
///
/// Every escape form is modelled by *how long it is*, because that is the only
/// thing this needs to know. An escape whose length is guessed wrong does not
/// merely survive — it spills its own tail into the output as text, which is
/// how a stray `B` appears in the middle of a sentence and why the shapes below
/// are enumerated rather than approximated:
///
/// | Form | Written | Ends at |
/// | --- | --- | --- |
/// | CSI | `ESC [ … final` | a byte in `@`–`~` |
/// | OSC | `ESC ] … ` | `BEL`, or `ESC \` |
/// | String | `ESC P`/`X`/`^`/`_` … | `BEL`, or `ESC \` |
/// | Designation | `ESC ( ) * + - . / #` + one byte | the byte after |
/// | Everything else | `ESC` + one byte | the byte after |
///
/// The designation row is the one that bit: `ESC ( B` — "G0 is US-ASCII" — is
/// three bytes, and reading it as two leaves the `B` behind. Windows emits it
/// constantly, which is why this was a Windows-looking bug in something with no
/// Windows in it.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                // A bare carriage return is a progress bar rewriting its own
                // line. Keeping it makes a transcript that overwrites itself;
                // dropping it where it precedes a newline keeps CRLF intact.
                if chars.peek() == Some(&'\n') {
                    continue;
                }
                out.push('\n');
            }
            // A terminal moves the cursor back and the next write covers what
            // was there. Applied rather than dropped, because dropping it
            // leaves the text the program meant to erase — the half-written
            // word in front of a progress counter.
            '\u{8}' => {
                if !out.ends_with('\n') {
                    out.pop();
                }
            }
            // Rings a bell nobody can hear. Dropped rather than passed on,
            // because in a transcript it is an unprintable character sitting in
            // the middle of a line.
            '\u{7}' => {}
            '\u{1b}' => match chars.next() {
                // CSI: parameters and intermediates, then one final byte.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC and the other string-carrying escapes — DCS, SOS, PM,
                // APC — all run until a string terminator. They are grouped
                // because the only thing that differs is the introducer, and
                // treating any of them as two bytes spills a whole payload into
                // the output rather than a single character.
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Three bytes: an introducer, then the character set or the
                // test pattern being selected.
                Some('(' | ')' | '*' | '+' | '-' | '.' | '/' | '#' | ' ') => {
                    chars.next();
                }
                // Two-byte escape; the second byte is the whole of it.
                Some(_) | None => {}
            },
            _ => out.push(c),
        }
    }
    out
}
