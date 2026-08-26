//! Finding a conversation by what was said in it.
//!
//! Transcripts have always been on disk, structured, and replayable, and the
//! only way to reach an old one was to recognise its title in a list. This
//! reads them.
//!
//! # What it searches, and what it does not
//!
//! Prose only: what you typed and what the model wrote back. Not tool calls,
//! not tool results. That is the whole difference between a search and a
//! `grep` of your home directory — tool results are file contents and build
//! logs, so a search that included them would match nearly every session for
//! nearly every query, and rank them by nothing. The question this answers is
//! "which conversation was that", and the answer is in the prose.
//!
//! Thinking blocks are left out for a related reason: a reasoning model's
//! chain of thought mentions everything it considered, including what it went
//! on to reject, and a hit inside one is not evidence the conversation was
//! about that.
//!
//! # Why it is fast enough to run on every keystroke
//!
//! A transcript is JSONL, so a line that does not contain the query as raw
//! bytes cannot contain it once parsed — and skipping the parse is the whole
//! cost of the search. A workspace of forty conversations is forty file reads
//! and, typically, no deserialization at all.
//!
//! That shortcut is only sound while the query survives JSON encoding
//! unchanged. `"` becomes `\"` on the way into the file, so a query holding one
//! would be looked for in a form it is never written in, and the prefilter
//! would rule out lines that do match. [`needs_parsing`] is that check, and a
//! query containing anything JSON escapes takes the slow path instead of a
//! wrong answer.

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use taurus_provider::{ContentBlock, Role};

use crate::sessions::{self, SessionMeta};

/// Conversations named in one answer. Beyond this the list is longer than the
/// question was specific, and the answer is to type another word.
const MAX_SESSIONS: usize = 12;

/// Hits shown per conversation. Enough to tell why it matched; a conversation
/// that mentions the query forty times is identified by the first two.
const MAX_MATCHES: usize = 3;

/// Characters of context around a hit. About a line of the panel it is drawn
/// in — long enough to read the sentence, short enough that four of them fit
/// on screen at once.
const EXCERPT: usize = 160;

/// Where one hit is, and enough around it to recognise.
///
/// Renamed on the way out because `Match` is a name every crate in a workspace
/// might reasonably pick, and ts-rs writes one file per *TypeScript* name —
/// two crates exporting a `Match` is one file, silently describing the wrong
/// payload for one of them. See `scripts/bindings.mjs`.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export, rename = "TranscriptMatch")]
pub struct Match {
    /// Which message in the transcript, so opening the conversation can scroll
    /// to it rather than to the top.
    pub message: u32,
    /// `"user"` or `"assistant"` — who said it.
    pub role: String,
    /// The text around the hit, trimmed, with ellipses where it was cut.
    pub excerpt: String,
    /// Where the hit sits inside `excerpt`, in UTF-16 code units.
    ///
    /// Not bytes. The frontend slices this string to draw the mark, and a
    /// JavaScript string is indexed in UTF-16 — handing it a byte offset works
    /// until somebody searches a conversation with an emoji earlier in the
    /// line, and then silently marks the wrong characters.
    pub from: u32,
    pub to: u32,
}

/// One conversation that matched.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionHit {
    pub session: SessionMeta,
    /// Every hit in this conversation, including the ones not listed.
    pub hits: u32,
    pub matches: Vec<Match>,
}

/// What one search found.
#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchResults {
    /// Most recently updated first — which is the order the rail lists
    /// conversations in, and the order "which one was that" is asked in.
    pub sessions: Vec<SessionHit>,
    /// Conversations that matched and are not listed, because [`MAX_SESSIONS`]
    /// was reached. Said rather than left implied: a list that stops without
    /// saying so reads as the whole list.
    pub more: u32,
}

/// Whether a query has to be looked for in parsed text rather than raw bytes.
///
/// True for anything JSON writes differently than it was typed. See the note
/// at the top of the file about why this is a correctness check and not an
/// optimization switch.
fn needs_parsing(query: &str) -> bool {
    query
        .chars()
        .any(|c| matches!(c, '"' | '\\' | '/') || c.is_control())
}

/// Conversations mentioning `query`, newest first.
///
/// `workspace` of `None` searches every workspace — which is the question
/// "where did I do that", asked when you no longer remember which project it
/// was. A blank query finds nothing rather than everything: an empty box is on
/// the way to typing something, not a request for the whole history.
pub fn search(workspace: Option<&Path>, query: &str) -> SearchResults {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return SearchResults::default();
    }
    let raw_ok = !needs_parsing(&needle);

    let mut found = Vec::new();
    let mut more = 0;
    for meta in sessions::list(workspace) {
        let Some(hit) = search_one(meta, &needle, raw_ok) else {
            continue;
        };
        if found.len() < MAX_SESSIONS {
            found.push(hit);
        } else {
            more += 1;
        }
    }

    SearchResults {
        sessions: found,
        more,
    }
}

fn search_one(meta: SessionMeta, needle: &str, raw_ok: bool) -> Option<SessionHit> {
    // The prefilter, and the reason this runs on a keystroke: a conversation
    // that does not mention the query anywhere is one file read and nothing
    // else. Only when the query would survive JSON encoding unchanged — see
    // `needs_parsing` — because otherwise a raw miss is not a real miss.
    if raw_ok && !sessions::mentions(&meta.id, needle) {
        return None;
    }
    // Past the prefilter, the whole conversation is rebuilt. More than is
    // needed to decide *whether* it matches, but it is the one place that
    // knows how a transcript is laid out, and a second reader here would be a
    // second thing to keep in step with the format.
    let loaded = sessions::load(&meta.id).ok()?;

    let mut matches = Vec::new();
    let mut hits = 0;
    for (index, message) in loaded.session.messages.iter().enumerate() {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            // A system message is the harness talking to the model, not a
            // conversation anybody had.
            _ => continue,
        };
        for block in &message.content {
            let ContentBlock::Text { text } = block else {
                continue;
            };
            let lowered = text.to_lowercase();
            let mut from = 0;
            while let Some(at) = lowered[from..].find(needle) {
                let at = from + at;
                hits += 1;
                if matches.len() < MAX_MATCHES {
                    matches.push(excerpt(text, at, needle.len(), index as u32, role));
                }
                from = at + needle.len();
            }
        }
    }

    if hits == 0 {
        return None;
    }
    Some(SessionHit {
        session: meta,
        hits,
        matches,
    })
}

/// The text around a hit, cut to something readable.
///
/// Cut on character boundaries and marked with ellipses, because a snippet
/// that begins mid-word without saying so reads as a typo in the transcript.
fn excerpt(text: &str, at: usize, len: usize, message: u32, role: &str) -> Match {
    // The line the hit is on, first: a message is often a paragraph, and the
    // sentence around the hit is more use than the same number of characters
    // taken blindly across a line break.
    let line_start = text[..at].rfind('\n').map(|n| n + 1).unwrap_or(0);
    let line_end = text[at..]
        .find('\n')
        .map(|n| at + n)
        .unwrap_or_else(|| text.len());

    let want = EXCERPT / 2;
    let mut start = line_start.max(at.saturating_sub(want));
    let mut end = line_end.min(at + len + want);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    while !text.is_char_boundary(end) {
        end += 1;
    }

    let body = text[start..end].trim();
    // Re-derived after the trim rather than adjusted, so leading whitespace
    // cannot leave the offsets pointing one character to the left.
    let trimmed_start = start + (text[start..end].len() - text[start..end].trim_start().len());
    let head = if start > line_start { "…" } else { "" };
    let tail = if end < line_end { "…" } else { "" };

    let before = utf16_len(&text[trimmed_start..at]) + utf16_len(head);
    Match {
        message,
        role: role.to_string(),
        excerpt: format!("{head}{body}{tail}"),
        from: before,
        to: before + utf16_len(&text[at..at + len]),
    }
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionLog;
    use crate::testing::isolated_home;
    use taurus_core::Session;
    use taurus_provider::Message;

    /// Writes a transcript the way a finished conversation leaves one.
    fn record(id: &str, workspace: &Path, turns: &[(&str, &str)]) {
        let mut session = Session::new("test-model");
        session.id = id.to_string();
        for (asked, answered) in turns {
            session.push(Message::user(*asked));
            session.push(Message::assistant(*answered));
        }
        let mut log = SessionLog::create(&session, workspace, None);
        log.record(&session);
    }

    fn ids(results: &SearchResults) -> Vec<&str> {
        results
            .sessions
            .iter()
            .map(|hit| hit.session.id.as_str())
            .collect()
    }

    #[test]
    fn it_finds_a_conversation_by_something_said_in_it() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/searching");
        record("a1", workspace, &[("fix the trust banner", "done")]);
        record("b2", workspace, &[("add a chart", "done")]);

        let found = search(Some(workspace), "trust banner");
        assert_eq!(ids(&found), vec!["a1"]);
        assert_eq!(found.sessions[0].hits, 1);
        assert!(found.sessions[0].matches[0]
            .excerpt
            .contains("trust banner"));
        assert_eq!(found.sessions[0].matches[0].role, "user");
    }

    #[test]
    fn it_finds_what_the_model_said_as_well_as_what_was_asked() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/both-sides");
        record(
            "c3",
            workspace,
            &[("why", "because the freshness check ran")],
        );

        let found = search(Some(workspace), "freshness");
        assert_eq!(ids(&found), vec!["c3"]);
        assert_eq!(found.sessions[0].matches[0].role, "assistant");
    }

    #[test]
    fn it_ignores_case() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/casing");
        record("d4", workspace, &[("The Trust Banner", "ok")]);

        assert_eq!(ids(&search(Some(workspace), "trust")), vec!["d4"]);
        assert_eq!(ids(&search(Some(workspace), "TRUST")), vec!["d4"]);
    }

    #[test]
    fn an_empty_query_finds_nothing_rather_than_everything() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/blank");
        record("e5", workspace, &[("something", "ok")]);

        // An empty box is on the way to typing something, not a request for
        // the whole history.
        assert!(search(Some(workspace), "").sessions.is_empty());
        assert!(search(Some(workspace), "   ").sessions.is_empty());
    }

    #[test]
    fn a_query_json_escapes_still_matches() {
        // The case the raw prefilter cannot answer: a quote is written `\"` in
        // the file, so looking for it in the bytes would rule the file out.
        // `needs_parsing` is what sends this down the slow path.
        let _home = isolated_home();
        let workspace = Path::new("/tmp/escaped");
        record("f6", workspace, &[(r#"call it "widget" please"#, "ok")]);

        assert!(needs_parsing(r#""widget""#));
        assert_eq!(ids(&search(Some(workspace), r#""widget""#)), vec!["f6"]);
    }

    #[test]
    fn it_counts_every_hit_and_lists_only_the_first_few() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/many");
        let mut turns = Vec::new();
        for _ in 0..6 {
            turns.push(("widget", "widget"));
        }
        record("g7", workspace, &turns);

        let found = search(Some(workspace), "widget");
        assert_eq!(found.sessions[0].hits, 12);
        // Enough to tell why it matched. A conversation that says it forty
        // times is identified by the first two.
        assert_eq!(found.sessions[0].matches.len(), MAX_MATCHES);
    }

    #[test]
    fn it_marks_where_the_hit_is_even_past_an_emoji() {
        // The offsets are sliced by a JavaScript string, which counts in
        // UTF-16. A byte offset works until somebody searches a conversation
        // with an emoji earlier in the line.
        let _home = isolated_home();
        let workspace = Path::new("/tmp/utf16");
        record("h8", workspace, &[("🙂 find the widget here", "ok")]);

        let found = search(Some(workspace), "widget");
        let hit = &found.sessions[0].matches[0];
        let units: Vec<u16> = hit.excerpt.encode_utf16().collect();
        let marked = String::from_utf16(&units[hit.from as usize..hit.to as usize]).unwrap();
        assert_eq!(marked, "widget");
    }

    #[test]
    fn an_excerpt_says_where_it_was_cut() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/long");
        let padding = "z ".repeat(300);
        record(
            "i9",
            workspace,
            &[(&format!("{padding}widget{padding}"), "ok")],
        );

        let hit = &search(Some(workspace), "widget").sessions[0].matches[0];
        // A snippet that begins mid-word without saying so reads as a typo in
        // the transcript.
        assert!(hit.excerpt.starts_with('…'), "{}", hit.excerpt);
        assert!(hit.excerpt.ends_with('…'), "{}", hit.excerpt);
        assert!(hit.excerpt.len() < EXCERPT * 2);
        let units: Vec<u16> = hit.excerpt.encode_utf16().collect();
        assert_eq!(
            String::from_utf16(&units[hit.from as usize..hit.to as usize]).unwrap(),
            "widget"
        );
    }

    #[test]
    fn it_says_how_many_conversations_it_did_not_list() {
        let _home = isolated_home();
        let workspace = Path::new("/tmp/crowded");
        for n in 0..(MAX_SESSIONS + 3) {
            record(&format!("s{n}"), workspace, &[("widget", "ok")]);
        }

        let found = search(Some(workspace), "widget");
        assert_eq!(found.sessions.len(), MAX_SESSIONS);
        // Said rather than left implied: a list that stops without saying so
        // reads as the whole list.
        assert_eq!(found.more, 3);
    }

    #[test]
    fn a_workspace_search_leaves_other_workspaces_out() {
        let _home = isolated_home();
        record("here1", Path::new("/tmp/here"), &[("widget", "ok")]);
        record("there1", Path::new("/tmp/there"), &[("widget", "ok")]);

        assert_eq!(
            ids(&search(Some(Path::new("/tmp/here")), "widget")),
            vec!["here1"]
        );
        // And `None` is the question "where did I do that", asked when you no
        // longer remember which project it was.
        let everywhere = search(None, "widget");
        assert_eq!(everywhere.sessions.len(), 2);
    }
}
