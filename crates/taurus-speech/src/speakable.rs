//! What of a reply is worth saying out loud.
//!
//! A reply is written to be read. Read out as it is, it's a voice spelling out
//! asterisks, every URL letter by letter, and forty lines of Rust. So this
//! keeps the prose and drops the rest:
//!
//! - **Code blocks are skipped whole**, fenced with backticks or tildes. So
//!   are tables. Nobody follows a diff by ear.
//! - **Inline code is kept when it's short**, because "run cargo test" is a
//!   sentence and the backticks were only typography. Past
//!   [`MAX_INLINE_CODE`] characters it's dropped.
//! - **Links say their text, and a bare URL says its host.** "See
//!   github.com for details" is what a person reading it aloud would say.
//! - **Markup goes**: emphasis, heading and list markers, quote bars, HTML
//!   tags, footnote marks, and emoji, which system voices read by name.
//!
//! It runs on a stream. A reply arrives a few words at a time, and waiting for
//! the end before speaking would make a long answer a long silence. So text
//! is handed out a sentence at a time, as soon as a sentence is known to be
//! finished and known not to be inside something that's about to be dropped.
//! A heading or a list item is its own utterance, so the voice pauses where
//! the eye would.
//!
//! Only the model's prose ever reaches this. Tool calls and their output
//! arrive on other events, which the window never feeds in.

/// Inline code longer than this is dropped rather than said.
pub const MAX_INLINE_CODE: usize = 40;

/// A sentence longer than this is split at a comma or a space. Engines take
/// much longer text, but a long first chunk is a long wait before the first
/// word.
const MAX_SENTENCE: usize = 300;

/// The streaming filter. Push text in as it arrives; take sentences out.
#[derive(Debug, Default)]
pub struct Speakable {
    /// The line being written, from its start.
    line: String,
    /// How much of `line` has already gone into `pending`.
    said: usize,
    /// What `line` turned out to be, once there's enough of it to tell.
    kind: Option<Line>,
    /// The fence a code block opened with, while inside one.
    fence: Option<String>,
    /// Prose not yet ending in a sentence, still marked up: where a sentence
    /// ends depends on what's inline code, which cleaning would forget.
    pending: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    /// Words to say. `own` lines, a heading or a list item, end their
    /// utterance when they end.
    Prose { own: bool },
    /// Opens a code block.
    Fence,
    /// Nothing to say: inside a code block, or a table row.
    Skip,
    /// A blank line or a rule, which ends a paragraph.
    Break,
}

impl Speakable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the next piece of a reply and returns the sentences it finished.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        self.line.push_str(text);
        while let Some(end) = self.line[self.said..].find('\n').map(|at| self.said + at) {
            let rest = self.line.split_off(end + 1);
            self.line.truncate(end);
            self.end_line(&mut out);
            self.line = rest;
            self.said = 0;
            self.kind = None;
        }
        self.partial(&mut out);
        out
    }

    /// The reply is over: everything still held, as sentences.
    ///
    /// Leaves the filter ready for the next reply, outside any code block.
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.line.is_empty() {
            self.end_line(&mut out);
        }
        flush(&mut self.pending, &mut out);
        *self = Self::default();
        out
    }

    /// Text that arrived without a line end. Prose is said up to its last
    /// finished sentence; everything else waits for the line to end.
    fn partial(&mut self, out: &mut Vec<String>) {
        if self.kind.is_none() {
            self.kind = self.decide(false);
            // A list item or heading starts an utterance, so the paragraph
            // before it is over even though it never ended in a full stop.
            if let Some(Line::Prose { own: true }) = self.kind {
                flush(&mut self.pending, out);
            }
        }
        let Some(Line::Prose { .. }) = self.kind else {
            return;
        };
        let rest = &self.line[self.said..];
        if let Some(&cut) = boundaries(rest, false).last() {
            join(&mut self.pending, &rest[..cut]);
            self.said += cut;
            drain(&mut self.pending, out);
        }
    }

    fn end_line(&mut self, out: &mut Vec<String>) {
        // A line's kind is decided before it ends when it can be, and the
        // fence it might close is only known now.
        if let Some(fence) = &self.fence {
            if closes(self.line.trim(), fence) {
                self.fence = None;
            }
            return;
        }
        let kind = match self.kind {
            Some(kind) => kind,
            None => self.decide(true).unwrap_or(Line::Break),
        };
        match kind {
            Line::Fence => {
                let trimmed = self.line.trim_start();
                let marker: String = trimmed
                    .chars()
                    .take_while(|&c| c == trimmed.chars().next().unwrap_or('`'))
                    .collect();
                // Whatever was said before the block is a finished thought.
                flush(&mut self.pending, out);
                self.fence = Some(marker);
            }
            Line::Skip => {}
            Line::Break => flush(&mut self.pending, out),
            Line::Prose { own } => {
                let rest = &self.line[self.said..];
                if own {
                    flush(&mut self.pending, out);
                    join(&mut self.pending, rest);
                    flush(&mut self.pending, out);
                } else {
                    join(&mut self.pending, rest);
                    drain(&mut self.pending, out);
                }
            }
        }
    }

    /// What the current line is, or `None` while there isn't enough of it to
    /// tell. Moves `said` past any marker the line starts with.
    fn decide(&mut self, complete: bool) -> Option<Line> {
        if self.fence.is_some() {
            return Some(Line::Skip);
        }
        let indent = self.line.len() - self.line.trim_start().len();
        let trimmed = &self.line[indent..];
        if trimmed.is_empty() {
            return complete.then_some(Line::Break);
        }
        let rule = trimmed.chars().all(|c| matches!(c, '-' | '*' | '_' | ' '));
        if !complete && (trimmed.chars().count() < 4 || rule || numbering(trimmed)) {
            return None;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            return Some(Line::Fence);
        }
        if trimmed.starts_with('|') {
            return Some(Line::Skip);
        }
        if rule && trimmed.chars().filter(|c| *c != ' ').count() >= 3 {
            return Some(Line::Break);
        }
        let (marker, own) = marker(trimmed);
        self.said = indent + marker;
        Some(Line::Prose { own })
    }
}

/// A whole reply at once.
pub fn sentences(text: &str) -> Vec<String> {
    let mut filter = Speakable::new();
    let mut out = filter.push(text);
    out.extend(filter.finish());
    out
}

/// Whether `trimmed` is still only digits, which could become `12. `.
fn numbering(trimmed: &str) -> bool {
    trimmed.chars().all(|c| c.is_ascii_digit())
}

/// How many bytes of line-start markup `trimmed` opens with, and whether it
/// makes the line an utterance of its own.
fn marker(trimmed: &str) -> (usize, bool) {
    let bytes = trimmed.as_bytes();
    let mut at = 0;
    // Quote bars, any depth: `> > text`.
    while at < bytes.len() && bytes[at] == b'>' {
        at += 1;
        while at < bytes.len() && bytes[at] == b' ' {
            at += 1;
        }
    }
    let quoted = at;
    let rest = &trimmed[at..];
    let hashes = rest.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && rest[hashes..].starts_with(' ') {
        return (at + hashes + 1, true);
    }
    let list = if rest.len() >= 2
        && matches!(rest.as_bytes()[0], b'-' | b'*' | b'+')
        && rest.as_bytes()[1] == b' '
    {
        2
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if (1..=3).contains(&digits)
            && rest.len() > digits + 1
            && matches!(rest.as_bytes()[digits], b'.' | b')')
            && rest.as_bytes()[digits + 1] == b' '
        {
            digits + 2
        } else {
            0
        }
    };
    if list > 0 {
        at += list;
        let rest = &trimmed[at..];
        for checkbox in ["[ ] ", "[x] ", "[X] "] {
            if rest.starts_with(checkbox) {
                at += checkbox.len();
            }
        }
        return (at, true);
    }
    (quoted, false)
}

/// Whether `trimmed` closes a code block opened with `fence`: the same
/// character, at least as many, and nothing after.
fn closes(trimmed: &str, fence: &str) -> bool {
    let Some(c) = fence.chars().next() else {
        return false;
    };
    let run = trimmed.chars().take_while(|&x| x == c).count();
    run >= fence.chars().count() && trimmed.chars().skip(run).all(char::is_whitespace)
}

/// Every place in `raw` a sentence ends, as the byte offset just past it.
///
/// `whole` says `raw` ends where its text does, so a full stop at the very
/// end ends a sentence. Partway through a line it might be `2.` of `2.5`.
///
/// Not inside inline code, and not inside a link's URL: `echo hi. there` and
/// `(https://x.dev/a. b)` are one piece each. Closing quotes and brackets go
/// with the sentence they close.
fn boundaries(raw: &str, whole: bool) -> Vec<usize> {
    let mut code = false;
    let mut url = false;
    let mut found = Vec::new();
    let mut chars = raw.char_indices().peekable();
    let mut previous = ' ';
    while let Some((at, c)) = chars.next() {
        match c {
            '`' => code = !code,
            '(' if !code && previous == ']' => url = true,
            ')' if url => url = false,
            '.' | '!' | '?' | '…' if !code && !url => {
                let mut end = at + c.len_utf8();
                while let Some(&(next, n)) = chars.peek() {
                    if matches!(n, '"' | '\'' | ')' | '”' | '’' | '*' | '_') {
                        end = next + n.len_utf8();
                        chars.next();
                    } else {
                        break;
                    }
                }
                let ends = match chars.peek() {
                    Some(&(_, next)) => next == ' ',
                    None => whole,
                };
                if ends && !abbreviation(&raw[..at]) {
                    found.push(end);
                }
            }
            _ => {}
        }
        previous = c;
    }
    found
}

/// Words a full stop doesn't end a sentence after.
fn abbreviation(before: &str) -> bool {
    let word = before
        .rsplit(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("");
    let word = word.to_ascii_lowercase();
    matches!(
        word.as_str(),
        "e.g" | "i.e" | "etc" | "vs" | "mr" | "mrs" | "ms" | "dr" | "cf" | "approx"
    )
}

/// Appends a stretch of prose to `pending` with a space between.
fn join(pending: &mut String, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    if !pending.is_empty() && !pending.ends_with(' ') {
        pending.push(' ');
    }
    pending.push_str(chunk);
}

/// Moves every finished sentence out of `pending`.
fn drain(pending: &mut String, out: &mut Vec<String>) {
    loop {
        let long = |end: usize| pending[..end].chars().count() > MAX_SENTENCE;
        let cut = match boundaries(pending, true).first().copied() {
            Some(end) if !long(end) => Some(end),
            Some(_) => Some(long_cut(pending)),
            None => long(pending.len()).then(|| long_cut(pending)),
        };
        let Some(cut) = cut else { return };
        let rest = pending.split_off(cut);
        let sentence = clean(pending);
        *pending = rest.trim_start().to_string();
        if speaks(&sentence) {
            out.push(sentence);
        }
    }
}

/// Moves everything out of `pending`, finished or not.
fn flush(pending: &mut String, out: &mut Vec<String>) {
    drain(pending, out);
    let rest = clean(&std::mem::take(pending));
    if speaks(&rest) {
        out.push(rest);
    }
}

/// Whether there's anything a voice would say in `text`: a lone colon left
/// behind by a dropped code block is not worth a pause.
fn speaks(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

/// Where to split a sentence that's run past [`MAX_SENTENCE`]: the last
/// comma, semicolon or colon before the limit, or failing that a space.
fn long_cut(text: &str) -> usize {
    let limit = text
        .char_indices()
        .nth(MAX_SENTENCE)
        .map_or(text.len(), |(at, _)| at);
    let head = &text[..limit];
    head.rfind([',', ';', ':'])
        .map(|at| at + 1)
        .or_else(|| head.rfind(' '))
        .filter(|&at| at > 0)
        .unwrap_or(limit)
}

/// Markdown and markup out of one stretch of prose.
pub fn clean(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() => {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            '`' => {
                let run = chars[i..].iter().take_while(|&&x| x == '`').count();
                let open = i + run;
                if let Some(close) = find_run(&chars, open, '`', run) {
                    let code: String = chars[open..close].iter().collect();
                    let code = code.trim();
                    if code.chars().count() <= MAX_INLINE_CODE {
                        out.push_str(code);
                    }
                    i = close + run;
                } else {
                    i = open;
                }
                continue;
            }
            '!' if chars.get(i + 1) == Some(&'[') => {
                // An image says nothing. Its alt text describes a picture
                // nobody listening can see.
                if let Some(end) = link_end(&chars, i + 1) {
                    i = end.end;
                    continue;
                }
            }
            '[' => {
                if let Some(link) = link_end(&chars, i) {
                    let text: String = chars[i + 1..link.text_end].iter().collect();
                    if !text.starts_with('^') {
                        out.push_str(&clean(&text));
                    }
                    i = link.end;
                    continue;
                }
            }
            '<' => {
                if let Some(close) = chars[i..].iter().position(|&x| x == '>') {
                    let inner: String = chars[i + 1..i + close].iter().collect();
                    if inner.starts_with("http://") || inner.starts_with("https://") {
                        out.push_str(&host(&inner));
                        i += close + 1;
                        continue;
                    }
                    if inner
                        .chars()
                        .next()
                        .is_some_and(|f| f.is_ascii_alphabetic() || f == '/' || f == '!')
                    {
                        i += close + 1;
                        continue;
                    }
                }
            }
            'h' if starts_url(&chars[i..]) && (i == 0 || !chars[i - 1].is_alphanumeric()) => {
                let end = chars[i..]
                    .iter()
                    .position(|x| x.is_whitespace())
                    .map_or(chars.len(), |n| i + n);
                let mut url: String = chars[i..end].iter().collect();
                // Sentence punctuation after a URL is the sentence's.
                let mut tail = String::new();
                while url.ends_with(['.', ',', ')', ';', ':', '!', '?']) {
                    tail.insert(0, url.pop().unwrap_or_default());
                }
                out.push_str(&host(&url));
                out.push_str(&tail);
                i = end;
                continue;
            }
            '*' | '~' => {
                i += 1;
                continue;
            }
            '_' => {
                let before = i > 0 && chars[i - 1].is_alphanumeric();
                let after = chars.get(i + 1).is_some_and(|x| x.is_alphanumeric());
                // Inside a word it's an identifier's, `snake_case`. At a
                // word's edge it's emphasis.
                if !(before && after) {
                    i += 1;
                    continue;
                }
            }
            c if emoji(c) => {
                i += 1;
                continue;
            }
            _ => {}
        }
        out.push(c);
        i += 1;
    }
    collapse(&out)
}

struct Link {
    /// Where the bracketed text ends, at its `]`.
    text_end: usize,
    /// Just past the whole link.
    end: usize,
}

/// A `[text](url)`, `[text][ref]` or footnote `[^1]` starting at `open`.
fn link_end(chars: &[char], open: usize) -> Option<Link> {
    let mut depth = 0;
    let mut close = None;
    for (n, &c) in chars.iter().enumerate().skip(open) {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(n);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let after = close + 1;
    let end = match chars.get(after) {
        Some('(') => after + chars[after..].iter().position(|&c| c == ')')? + 1,
        Some('[') => after + chars[after..].iter().position(|&c| c == ']')? + 1,
        _ if chars.get(open + 1) == Some(&'^') => after,
        // Brackets that aren't a link are just brackets: `[WIP]`.
        _ => return None,
    };
    Some(Link {
        text_end: close,
        end,
    })
}

fn find_run(chars: &[char], from: usize, c: char, run: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == c {
            let here = chars[i..].iter().take_while(|&&x| x == c).count();
            if here == run {
                return Some(i);
            }
            i += here;
        } else {
            i += 1;
        }
    }
    None
}

fn starts_url(chars: &[char]) -> bool {
    let head: String = chars.iter().take(8).collect();
    head.starts_with("http://") || head.starts_with("https://")
}

/// The part of a URL a person would say: `https://www.github.com/a/b` is
/// `github.com`.
fn host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// Pictographs, which system voices read out by name: "check mark button".
fn emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0xFE00..=0xFE0F | 0x200D)
}

fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        // Punctuation left hanging by something dropped before it: "See ."
        if !out.is_empty() && !word.starts_with([',', '.', ';', ':', '!', '?', ')']) {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `text` a few characters at a time, the way a reply streams.
    fn streamed(text: &str, step: usize) -> Vec<String> {
        let mut filter = Speakable::new();
        let mut out = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        for piece in chars.chunks(step) {
            out.extend(filter.push(&piece.iter().collect::<String>()));
        }
        out.extend(filter.finish());
        out
    }

    #[test]
    fn prose_comes_out_a_sentence_at_a_time() {
        assert_eq!(
            sentences("The build passed. Two tests were slow! Want me to look?"),
            [
                "The build passed.",
                "Two tests were slow!",
                "Want me to look?"
            ]
        );
    }

    #[test]
    fn streaming_gives_the_same_sentences_however_it_is_cut() {
        let reply = "I read **the config**. It sets `max_iterations` to 25.\n\n\
                     ## Next steps\n\n- Raise the limit.\n- Run `cargo test` again.\n\n\
                     ```rust\nfn main() {}\n```\n\nThat's all. See [the docs](https://x.dev/a).";
        let whole = sentences(reply);
        for step in [1, 2, 3, 7, 50] {
            assert_eq!(streamed(reply, step), whole, "step {step}");
        }
        assert_eq!(
            whole,
            [
                "I read the config.",
                "It sets max_iterations to 25.",
                "Next steps",
                "Raise the limit.",
                "Run cargo test again.",
                "That's all.",
                "See the docs."
            ]
        );
    }

    #[test]
    fn a_sentence_is_handed_out_before_the_line_ends() {
        let mut filter = Speakable::new();
        assert!(filter.push("The first sentence is done").is_empty());
        assert_eq!(filter.push(". And the"), ["The first sentence is done."]);
        assert_eq!(filter.finish(), ["And the"]);
    }

    #[test]
    fn code_blocks_are_skipped_whole() {
        let reply =
            "Here's the fix:\n\n```diff\n- old line.\n+ new line. Done.\n```\n\nThat should do it.";
        assert_eq!(sentences(reply), ["Here's the fix:", "That should do it."]);
    }

    #[test]
    fn a_tilde_fence_and_a_longer_backtick_fence_close_only_on_their_own() {
        let reply =
            "Before.\n~~~\n```\nstill code.\n~~~\nAfter.\n````\n```\nnot closed yet.\n````\nEnd.";
        assert_eq!(sentences(reply), ["Before.", "After.", "End."]);
    }

    #[test]
    fn a_reply_that_ends_inside_a_code_block_leaves_the_next_one_alone() {
        let mut filter = Speakable::new();
        filter.push("Look:\n```\nlet x = 1;");
        filter.finish();
        assert_eq!(filter.push("Fresh reply.\n"), ["Fresh reply."]);
    }

    #[test]
    fn tables_and_rules_say_nothing() {
        let reply = "Results:\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\nDone.";
        assert_eq!(sentences(reply), ["Results:", "Done."]);
    }

    #[test]
    fn long_inline_code_is_dropped_and_short_is_kept() {
        assert_eq!(
            clean("Run `cargo test -p taurus-speech` or `a very long command line with many many flags in it`."),
            "Run cargo test -p taurus-speech or."
        );
    }

    #[test]
    fn links_say_their_text_and_urls_their_host() {
        assert_eq!(
            clean("Read [the guide](https://example.com/g) or https://www.github.com/a/b."),
            "Read the guide or github.com."
        );
        assert_eq!(clean("Mail <https://foo.dev/x> now"), "Mail foo.dev now");
    }

    #[test]
    fn images_footnotes_tags_and_emoji_go() {
        assert_eq!(
            clean("Done ✅ ![chart](c.png) see note[^1]<br/> <details>more</details>"),
            "Done see note more"
        );
    }

    #[test]
    fn brackets_that_are_not_links_are_kept() {
        assert_eq!(clean("Marked [WIP] for now"), "Marked [WIP] for now");
    }

    #[test]
    fn emphasis_goes_but_identifiers_keep_their_underscores() {
        assert_eq!(
            clean("**Bold**, _italic_, ~~gone~~ and snake_case_name"),
            "Bold, italic, gone and snake_case_name"
        );
    }

    #[test]
    fn markers_go_and_list_items_are_their_own_utterances() {
        let reply = "# Title\n> Quoted line\n1. First item\n2) Second item\n- [x] Checked\n+ Plus";
        assert_eq!(
            sentences(reply),
            [
                "Title",
                "Quoted line",
                "First item",
                "Second item",
                "Checked",
                "Plus"
            ]
        );
    }

    #[test]
    fn abbreviations_and_decimals_do_not_end_a_sentence() {
        assert_eq!(
            sentences("Use a tool, e.g. ripgrep. It's 2.5 times faster."),
            ["Use a tool, e.g. ripgrep.", "It's 2.5 times faster."]
        );
    }

    #[test]
    fn a_sentence_inside_inline_code_is_not_cut_while_streaming() {
        let mut filter = Speakable::new();
        assert!(filter.push("Type `echo hi. there` ").is_empty());
        assert_eq!(filter.push("now. Ok"), ["Type echo hi. there now."]);
    }

    #[test]
    fn a_long_run_on_is_split_at_a_comma() {
        let clause = "word ".repeat(40);
        let text = format!("{clause}, {clause}, {clause}.");
        let out = sentences(&text);
        assert!(out.len() > 1, "{out:?}");
        assert!(
            out.iter().all(|s| s.chars().count() <= MAX_SENTENCE),
            "{out:?}"
        );
    }

    #[test]
    fn quotes_closing_a_sentence_stay_with_it() {
        assert_eq!(
            sentences("He said \"stop.\" Then left."),
            ["He said \"stop.\"", "Then left."]
        );
    }
}
