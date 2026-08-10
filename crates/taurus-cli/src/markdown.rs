//! Light markdown styling for a terminal.
//!
//! Models answer in markdown whether or not anything renders it, so a terminal
//! that prints the raw text shows `**bold**` and stray backticks. This applies
//! ANSI attributes instead — enough to make an answer scannable, without
//! pretending to be a full renderer.
//!
//! Two rules keep it safe. Styling is line-oriented, because a construct that
//! spans lines cannot be resolved from a token stream without buffering the
//! whole answer. And with color disabled — piped output, `NO_COLOR`, a
//! redirect — every line passes through byte for byte, so `taurus run > out.md`
//! still produces valid markdown.

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET_INTENSITY: &str = "\x1b[22m";
const CODE: &str = "\x1b[36m";
const RESET_COLOR: &str = "\x1b[39m";

pub struct MarkdownStyler {
    color: bool,
    /// Inside a fenced block, markdown syntax is code and must not be styled.
    in_fence: bool,
}

impl MarkdownStyler {
    pub fn new(color: bool) -> Self {
        Self {
            color,
            in_fence: false,
        }
    }

    /// Styles one complete line, without its newline.
    pub fn line(&mut self, line: &str) -> String {
        if !self.color {
            // Untouched: redirected output must remain the model's markdown.
            return line.to_string();
        }

        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            self.in_fence = !self.in_fence;
            return format!("{DIM}{line}{RESET_INTENSITY}");
        }
        if self.in_fence {
            return format!("{DIM}{line}{RESET_INTENSITY}");
        }

        let indent_len = line.len() - trimmed.len();
        let indent = &line[..indent_len];

        // Headings: the whole line, marker dropped.
        if let Some(rest) = heading_body(trimmed) {
            return format!("{indent}{BOLD}{}{RESET_INTENSITY}", inline(rest));
        }

        if let Some(rest) = trimmed.strip_prefix("> ") {
            return format!("{indent}{DIM}│ {}{RESET_INTENSITY}", inline(rest));
        }

        // Horizontal rule.
        if trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') {
            return format!("{DIM}{line}{RESET_INTENSITY}");
        }

        // Bullets: a real bullet glyph reads better than the source marker.
        for marker in ["- ", "* ", "+ "] {
            if let Some(rest) = trimmed.strip_prefix(marker) {
                return format!("{indent}• {}", inline(rest));
            }
        }

        format!("{indent}{}", inline(trimmed))
    }

    /// Styles a trailing partial line at end of stream.
    pub fn partial(&mut self, line: &str) -> String {
        self.line(line)
    }
}

/// `## Heading` → `Heading`, for one to six hashes followed by a space.
fn heading_body(trimmed: &str) -> Option<&str> {
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) {
        trimmed[hashes..].strip_prefix(' ')
    } else {
        None
    }
}

/// Applies inline `**bold**` and `` `code` `` within a line.
///
/// Deliberately not a parser: it pairs delimiters left to right and leaves an
/// unmatched one alone, which is what a half-streamed line looks like.
fn inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i..].starts_with(b"**") {
            if let Some(end) = find(text, i + 2, "**") {
                out.push_str(BOLD);
                out.push_str(&text[i + 2..end]);
                out.push_str(RESET_INTENSITY);
                i = end + 2;
                continue;
            }
        }
        if bytes[i] == b'`' {
            if let Some(end) = find(text, i + 1, "`") {
                out.push_str(CODE);
                out.push_str(&text[i + 1..end]);
                out.push_str(RESET_COLOR);
                i = end + 1;
                continue;
            }
        }
        // Copy one whole character so multi-byte text is never split.
        let ch = text[i..].chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn find(text: &str, from: usize, needle: &str) -> Option<usize> {
    text.get(from..)?.find(needle).map(|at| at + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(line: &str) -> String {
        MarkdownStyler::new(true).line(line)
    }

    #[test]
    fn without_color_the_line_is_untouched() {
        // The property that keeps `taurus run > out.md` valid markdown.
        let mut styler = MarkdownStyler::new(false);
        for line in [
            "**bold** and `code`",
            "## Heading",
            "- bullet",
            "```rust",
            "> quote",
        ] {
            assert_eq!(styler.line(line), line);
        }
    }

    #[test]
    fn bold_markers_are_replaced_by_attributes() {
        let out = styled("Done. **README.md** is updated.");
        assert!(out.contains(BOLD));
        assert!(!out.contains("**"));
        assert!(out.contains("README.md"));
    }

    #[test]
    fn inline_code_is_colored_and_unbackticked() {
        let out = styled("Run `cargo test` now.");
        assert!(out.contains(CODE));
        assert!(!out.contains('`'));
        assert!(out.contains("cargo test"));
    }

    #[test]
    fn headings_lose_their_hashes() {
        let out = styled("## Summary");
        assert!(out.contains(BOLD));
        assert!(!out.contains('#'));
        assert!(out.contains("Summary"));
    }

    #[test]
    fn a_hash_that_is_not_a_heading_is_left_alone() {
        assert!(styled("#hashtag").contains("#hashtag"));
        assert!(styled("####### too many").contains("#######"));
    }

    #[test]
    fn bullets_become_glyphs_and_keep_their_indent() {
        assert!(styled("- first").starts_with("• "));
        assert!(styled("  - nested").starts_with("  • "));
        assert!(styled("* star").starts_with("• "));
    }

    #[test]
    fn fenced_blocks_are_not_styled_as_markdown() {
        let mut styler = MarkdownStyler::new(true);
        styler.line("```rust");
        // `*ptr` inside code must not turn into a bullet.
        let inside = styler.line("    let x = *ptr; // **not bold**");
        assert!(inside.contains("**not bold**"));
        assert!(inside.contains("*ptr"));
        styler.line("```");
        // Back outside, styling resumes.
        assert!(styler.line("**bold**").contains(BOLD));
    }

    #[test]
    fn an_unclosed_bold_marker_is_left_as_text() {
        // Exactly what a half-streamed line looks like.
        let out = styled("this is **bol");
        assert!(out.contains("**bol"));
    }

    #[test]
    fn an_unclosed_backtick_is_left_as_text() {
        assert!(styled("run `cargo").contains("`cargo"));
    }

    #[test]
    fn several_spans_on_one_line_all_style() {
        let out = styled("**a** then `b` then **c**");
        assert_eq!(out.matches(BOLD).count(), 2);
        assert_eq!(out.matches(CODE).count(), 1);
    }

    #[test]
    fn blockquotes_are_dimmed_with_a_rule() {
        let out = styled("> note this");
        assert!(out.contains(DIM));
        assert!(out.contains('│'));
    }

    #[test]
    fn horizontal_rules_are_dimmed() {
        assert!(styled("---").contains(DIM));
    }

    #[test]
    fn multibyte_text_survives_styling() {
        let out = styled("**日本語** と emoji 🎉");
        assert!(out.contains("日本語"));
        assert!(out.contains("🎉"));
    }

    #[test]
    fn an_empty_line_stays_empty() {
        assert_eq!(styled(""), "");
    }

    #[test]
    fn plain_prose_is_returned_unchanged() {
        assert_eq!(styled("just a sentence."), "just a sentence.");
    }
}
