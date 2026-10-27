//! Would trimming by age cut what the model goes on to need?
//!
//! ```sh
//! cargo run -p taurus-host --example trim_replay
//! ```
//!
//! Reads every saved conversation and writes nothing. Each one is replayed
//! message by message through the trimming a turn really runs, at the budgets
//! four window sizes leave once the reply's share is held back. Whenever the
//! replayed history passes the budget, the older tool results are trimmed the
//! way `Agent::compact_if_needed` trims them. Each result that is cut is then
//! checked against what the model actually did next: did it read the same
//! file, or search the same pattern, again before anything wrote to it?
//!
//! Transcripts can't answer this on their own. They are written once per
//! round, before trimming ever reaches a message, so a conversation that was
//! trimmed in memory looks untouched on disk. Replaying is the only way to see
//! what the trim would have taken.
//!
//! What it can't see: the model in these conversations had the full result,
//! so "read it again" here means "came back to it" — the result was still in
//! use — not "was made to fetch it again". That is the question reuse-scored
//! trimming turns on. Summarizing isn't replayed (it needs a model), so later
//! trims in a conversation that would have been summarized are overcounted.

use std::collections::HashMap;

use taurus_core::session::split_for_compaction;
use taurus_core::{Session, TrimRules};
use taurus_host::sessions;
use taurus_provider::{ContentBlock, Message};

/// The window, and the budget it leaves: the window less the reply reserve,
/// which `Agent` clamps to between a twentieth and a quarter of the window.
const BUDGETS: &[(&str, u32)] = &[
    ("16k", 12_000),
    ("32k", 24_000),
    ("64k", 48_000),
    ("200k", 168_000),
];

/// The tools a repeat of which supersedes the earlier answer, as `Agent::is_read`
/// answers for the builtins, and the ones whose target is worth following.
const READS: &[&str] = &[
    "read_file",
    "grep",
    "glob",
    "list_dir",
    "search_code",
    "read_note",
];
const WRITES: &[&str] = &["write_file", "edit_file", "write_note", "run_command"];

/// What a call is about: the file, pattern or directory it names.
fn target(input: &serde_json::Value) -> String {
    for key in ["path", "file", "pattern", "query", "dir"] {
        if let Some(value) = input.get(key) {
            return value.to_string();
        }
    }
    input.to_string()
}

/// One tool call in the original conversation, in order.
struct Call {
    /// The index of the message it was made in.
    at: usize,
    name: String,
    target: String,
}

#[derive(Default)]
struct Tally {
    conversations_trimmed: usize,
    trims: usize,
    /// Times trimming wasn't enough and the head was summarized away.
    summaries: usize,
    results_cut: usize,
    reads_cut: usize,
    /// Of `reads_cut`, the ones whose target the model read again later,
    /// nothing having written to it in between.
    came_back: usize,
    /// Of `reads_cut`, the ones whose target a later reply named.
    named_later: usize,
    /// Cut results whose file was edited between the read and the cut: stale
    /// whatever trimming did, so left out of the two counts above.
    stale: usize,
    /// What a scorer could know at the moment of the cut, each as (how many
    /// cut reads had it, how many of those came back). A reply had named the
    /// target since it was read; the target had been read more than once.
    named_before: (usize, usize),
    read_before: (usize, usize),
    neither: (usize, usize),
}

fn main() {
    let listed = sessions::list(None);
    let loaded: Vec<_> = listed
        .iter()
        .filter_map(|meta| sessions::load(&meta.id).ok())
        .collect();
    println!("{} conversations", loaded.len());

    // The baseline the cut results are measured against: of every read in
    // these conversations, how many the model came back to.
    let (mut reads, mut returned) = (0usize, 0usize);
    for conversation in &loaded {
        let calls = calls_of(&conversation.session.messages);
        for (i, call) in calls.iter().enumerate() {
            if READS.contains(&call.name.as_str()) {
                reads += 1;
                if comes_back(&calls[i + 1..], call) {
                    returned += 1;
                }
            }
        }
    }
    println!(
        "baseline: {returned} of {reads} reads came back to later ({:.0}%)\n",
        percent(returned, reads)
    );

    for &(window, budget) in BUDGETS {
        for mode in [Mode::Age, Mode::Ranked] {
            let mut tally = Tally::default();
            for conversation in &loaded {
                replay(&conversation.session, budget, mode, &mut tally);
            }
            println!(
            "{window:>5} window ({budget} budget), {}: {} conversations trimmed, {} trims, {} summaries, {} results cut",
            mode.label(),
            tally.conversations_trimmed,
            tally.trims,
            tally.summaries,
            tally.results_cut
        );
            if tally.reads_cut > 0 {
                println!(
                "       of {} reads cut ({} more were stale already): {} came back to later ({:.0}%), {} named in a later reply ({:.0}%)",
                tally.reads_cut,
                tally.stale,
                tally.came_back,
                percent(tally.came_back, tally.reads_cut),
                tally.named_later,
                percent(tally.named_later, tally.reads_cut),
            );
                for (label, (had, back)) in [
                    ("named in a reply before the cut", tally.named_before),
                    ("read more than once before the cut", tally.read_before),
                    ("neither", tally.neither),
                ] {
                    println!(
                        "         {label:<36} {had:>4} cut, {back:>4} came back ({:.0}%)",
                        percent(back, had)
                    );
                }
            }
        }
        println!();
    }
}

/// Which trimming to replay.
#[derive(Clone, Copy)]
enum Mode {
    /// Everything old and large, every time: what trimming did before it was
    /// ranked.
    Age,
    /// `Session::trim_toward` as `Agent` calls it: stale reads first, files
    /// read more than once last, stopping at half the budget.
    Ranked,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Age => "by age",
            Mode::Ranked => "ranked",
        }
    }
}

/// How far below the budget a ranked trim stops: `Agent`'s `TRIM_LOW_WATER`,
/// unless `TRIM_LOW_WATER` in the environment asks to try another.
fn low_water() -> f32 {
    std::env::var("TRIM_LOW_WATER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.5)
}

/// The files a call writes, standing in for the registry's `touches`.
fn writes_to(name: &str, input: &serde_json::Value) -> Vec<String> {
    match name {
        "write_file" | "edit_file" | "write_note" => input
            .get("path")
            .and_then(|p| p.as_str())
            .map(|p| vec![p.to_string()])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

fn calls_of(messages: &[Message]) -> Vec<Call> {
    messages
        .iter()
        .enumerate()
        .flat_map(|(at, message)| {
            message.tool_uses().map(move |(_, name, input)| Call {
                at,
                name: name.to_string(),
                target: target(input),
            })
        })
        .collect()
}

/// Whether a later call reads `call`'s target again before anything writes it.
fn comes_back<'a>(later: impl IntoIterator<Item = &'a Call>, call: &Call) -> bool {
    for next in later {
        if WRITES.contains(&next.name.as_str())
            && (next.target == call.target || next.name == "run_command")
        {
            return false;
        }
        if next.name == call.name && next.target == call.target {
            return true;
        }
    }
    false
}

/// Replays one conversation at one budget, and adds what trimming cut to the
/// tally, judged against what the model went on to do.
fn replay(original: &Session, budget: u32, mode: Mode, tally: &mut Tally) {
    let messages = &original.messages;
    let calls = calls_of(messages);
    let by_id: HashMap<String, usize> = messages
        .iter()
        .flat_map(|m| m.tool_uses().map(|(id, _, _)| id.to_string()))
        .zip(0..)
        .collect();
    let is_read = |name: &str| READS.contains(&name);

    let mut session = Session::new(original.model.clone());
    let mut cut: HashMap<String, usize> = HashMap::new();
    for (at, message) in messages.iter().enumerate() {
        session.push(message.clone());
        if session.estimated_tokens() <= budget {
            continue;
        }
        // The boundary `compact_if_needed` uses: eight messages, or half the
        // budget, whichever binds first.
        let (_, kept) = split_for_compaction(&session.messages, 8, budget / 2);
        let trimmed = match mode {
            Mode::Age => session.trim_tool_results(kept, &is_read),
            Mode::Ranked => {
                let rules = TrimRules {
                    repeats_supersede: &is_read,
                    writes_to: &writes_to,
                };
                session.trim_toward(kept, &rules, (budget as f32 * low_water()) as u32)
            }
        };
        // Still over after trimming: the agent would summarize, which drops
        // the head of the history for a paragraph. Stood in for by a fixed
        // note the size of a short summary, since a summary needs a model.
        // What a summary drops isn't counted as cut — trimming didn't cut it.
        if session.estimated_tokens() > budget {
            let (drop, _) = split_for_compaction(&session.messages, 8, budget / 2);
            if drop > 0 {
                tally.summaries += 1;
                session.summarize_front(drop, Message::user("[summary] ".repeat(200)));
            }
        }
        if trimmed.is_empty() {
            continue;
        }
        tally.trims += 1;
        // Indexed in the original conversation, which is what the calls the
        // cut is judged against are numbered in.
        let now = at + 1;
        for message in &session.messages {
            for block in &message.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                {
                    let text = content.to_text();
                    if (text.contains("[shortened:") || text.contains("[dropped: `"))
                        && !cut.contains_key(tool_use_id)
                    {
                        cut.insert(tool_use_id.clone(), now);
                    }
                }
            }
        }
    }
    if cut.is_empty() {
        return;
    }
    tally.conversations_trimmed += 1;
    tally.results_cut += cut.len();

    for (id, when) in &cut {
        let Some(&index) = by_id.get(id) else {
            continue;
        };
        let call = &calls[index];
        if !READS.contains(&call.name.as_str()) {
            continue;
        }
        let edited_since = calls.iter().any(|c| {
            c.at > call.at
                && c.at < *when
                && WRITES.contains(&c.name.as_str())
                && c.name != "run_command"
                && c.target == call.target
        });
        if edited_since {
            tally.stale += 1;
            continue;
        }
        tally.reads_cut += 1;
        // Only what happened after the trim: before it, the model had the
        // whole result, and coming back then says nothing about the cut.
        let back = comes_back(calls.iter().filter(|c| c.at >= *when), call);
        if back {
            tally.came_back += 1;
        }
        let quoted = call.target.trim_matches('"');
        let named = quoted.len() > 3
            && messages[call.at + 1..*when]
                .iter()
                .any(|m| m.role == taurus_provider::Role::Assistant && m.text().contains(quoted));
        let reread = calls
            .iter()
            .filter(|c| c.at < *when && c.name == call.name && c.target == call.target)
            .count()
            > 1;
        for (signal, slot) in [
            (named, &mut tally.named_before),
            (reread, &mut tally.read_before),
            (!named && !reread, &mut tally.neither),
        ] {
            if signal {
                slot.0 += 1;
                slot.1 += usize::from(back);
            }
        }
        let name = call.target.trim_matches('"');
        if name.len() > 3
            && messages[*when..]
                .iter()
                .any(|m| m.role == taurus_provider::Role::Assistant && m.text().contains(name))
        {
            tally.named_later += 1;
        }
    }
}
