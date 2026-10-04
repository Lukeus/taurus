//! Conversation state and the context-window budget.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use taurus_provider::{ContentBlock, Message, Role, TokenUsage};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub model: String,
    pub messages: Vec<Message>,
    /// Cumulative across the session, for the UI's token counter.
    pub usage: TokenUsage,
    /// What the last request really cost, beside what was estimated for it.
    ///
    /// Kept because it is the only exact number in this file. See
    /// [`Measured`].
    #[serde(default)]
    pub last_request: Option<Measured>,
    /// How far a transcript's numbering runs ahead of `messages`.
    ///
    /// Compaction takes messages off the front and puts one summary in their
    /// place, and the summary is never written down. So the transcript still
    /// holds everything that was dropped, and message `i` here is message
    /// `i + summarized_away` there, for every message a compaction kept. A
    /// writer that counted by position alone lost the round after each
    /// compaction: the list had shrunk under its offset, so it found nothing
    /// new, then moved its offset past the round it had skipped.
    ///
    /// Zero for a session loaded from disk, because what was loaded is the
    /// whole transcript.
    #[serde(default)]
    pub summarized_away: usize,
    /// Results owed to calls a stopped process never answered.
    ///
    /// A transcript whose last message is a call with no result is one the
    /// process died in the middle of. Sending it as it stands is a request
    /// every provider rejects, and replaying the call would repeat something
    /// that may already have happened. So the loader answers each call with
    /// "outcome unknown", and the answers ride in front of the next user
    /// message, whatever it is. A message of their own would put two user
    /// messages in a row. See [`owed_results`].
    #[serde(skip)]
    pub owed: Vec<ContentBlock>,
    /// The turn this conversation was in when the process running it stopped,
    /// as the transcript tells it. Cleared by the next turn to start.
    #[serde(skip)]
    pub interrupted: Option<Interrupted>,
}

/// How many turns one request gets, counting the one that was interrupted and
/// every continuation of it.
///
/// A restart doesn't reset it, because it's counted from the transcript. A
/// message you type does, because that's a new request.
pub const MAX_ATTEMPTS: u32 = 3;

/// What a continuation says. Nothing is replayed: the model reads the history,
/// including which calls have unknown outcomes, and decides what's left.
pub const CONTINUE_PROMPT: &str =
    "Your previous run was interrupted. Continue from where you left off.";

/// A turn that was running when the process stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interrupted {
    /// Turns spent on this request so far: the original, and each
    /// continuation. Continuing is refused at [`MAX_ATTEMPTS`].
    pub attempts: u32,
    /// Calls that were running, whose outcome is unknown.
    pub unanswered: usize,
}

impl Interrupted {
    pub fn can_continue(&self) -> bool {
        self.attempts < MAX_ATTEMPTS
    }
}

/// "Outcome unknown" answers for every call in `assistant`, which the process
/// stopped before answering.
///
/// Marked as errors, because the one thing known is that nothing reported
/// success. Each says what to check before trying again, from the call's own
/// input: the path a file tool named, the command a shell call ran.
pub fn owed_results(assistant: &Message) -> Vec<ContentBlock> {
    assistant
        .tool_uses()
        .map(|(id, name, input)| {
            let mut text = String::from(
                "Outcome unknown: Taurus stopped while this call was running, so it may or may not \
                 have taken effect.",
            );
            let path = input.get("path").and_then(|v| v.as_str());
            let command = input.get("command").and_then(|v| v.as_str());
            match (path, command) {
                (Some(path), _) => text.push_str(&format!(
                    " It named `{path}`. Read it before doing this again."
                )),
                (None, Some(command)) => text.push_str(&format!(
                    " It ran `{command}`, and what a command changes isn't known until it \
                     finishes. Check before running it again."
                )),
                _ => text.push_str(&format!(
                    " Check what `{name}` did before calling it again."
                )),
            }
            ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: text.into(),
                is_error: true,
            }
        })
        .collect()
}

/// A request's real size, beside the estimates that were made of it.
///
/// Three numbers rather than two, because the one thing a pair cannot do is
/// tell its two unknowns apart. `input_tokens - estimated_messages` folds
/// together the fixed part of a request — the system prompt, every tool's
/// schema, the plan appended to the end, the envelope the provider wraps it
/// all in — and the gap between four-characters-a-token and what the model's
/// own tokenizer makes of the *messages*. Those two behave nothing alike: the
/// fixed part is the same on every request of a session, and the gap grows
/// with the conversation.
///
/// Folded together they still answer "does the whole prompt fit", because the
/// gap is added back to a conversation of about the size it was measured on.
/// They are wrong for every question about a *part* of the history, and
/// [`crate::agent`] asks one: before summarizing, whether the recent messages
/// it cannot summarize would fit on their own. Charging a whole
/// conversation's worth of estimator error against eight messages made that
/// check fail on long code-heavy sessions where compaction would have worked
/// fine — and fail by reporting a window too small for the conversation,
/// which was not what had happened.
///
/// So the fixed part is estimated directly and kept, and what is left over is
/// attributed to the messages, where it belongs, as a ratio. See
/// [`Session::calibration`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Measured {
    /// The whole prompt as the provider counted it, cache hits included.
    pub input_tokens: u32,
    /// What [`Session::estimated_tokens`] said about the messages alone at the
    /// moment that request was sent.
    pub estimated_messages: u32,
    /// What the caller estimated the fixed part of that same request at.
    ///
    /// Defaulted for sessions written before it was recorded: zero reads as
    /// "no anchor", [`Session::calibration`] answers 1.0, and the next
    /// answered request replaces the whole reading.
    #[serde(default)]
    pub estimated_overhead: u32,
}

/// The least estimated history a calibration may be drawn from.
///
/// [`Session::calibration`] divides by the message estimate, so on a
/// conversation of two short messages any error in the overhead anchor is
/// multiplied by a large number. Below this a reading says more about the
/// anchor than about the tokenizer, and 1.0 is the better answer. A working
/// session passes this within a turn or two.
const MIN_CALIBRATION_SAMPLE: u32 = 500;

/// How far a calibration may correct an estimate, at each end.
///
/// These guard against a wrong anchor; they are not a belief about how
/// tokenizers work. The floor is 1.0 rather than something lower on purpose.
/// A ratio below one says the provider counted *fewer* tokens than
/// four-characters-a-token predicted, which over a history of code and JSON
/// nearly always means the fixed part was over-estimated rather than that the
/// model's tokenizer is unusually efficient — and the two ways of being wrong
/// do not cost the same. Correcting too far up compacts a turn earlier than it
/// had to and loses some history; too far down never compacts at all, and the
/// provider refuses the request outright.
const CALIBRATION_BOUNDS: (f32, f32) = (1.0, 3.0);

impl Session {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            model: model.into(),
            messages: Vec::new(),
            usage: TokenUsage::default(),
            last_request: None,
            summarized_away: 0,
            owed: Vec::new(),
            interrupted: None,
        }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Replaces the first `drop` messages with `summary`, and keeps
    /// [`Self::summarized_away`] in step with it.
    ///
    /// One method rather than two edits at the call site, because the offset
    /// is only right if it moves in the same step as the list.
    pub fn summarize_front(&mut self, drop: usize, summary: Message) {
        let drop = drop.min(self.messages.len());
        let rest = self.messages.split_off(drop);
        self.messages.clear();
        self.messages.push(summary);
        self.messages.extend(rest);
        // `drop` messages left and one arrived that no transcript will hold.
        // A summary of an earlier summary drops that one too; it was never
        // written, so it was never counted.
        self.summarized_away = (self.summarized_away + drop).saturating_sub(1);
    }

    pub fn add_usage(&mut self, usage: TokenUsage) {
        self.usage.add(&usage);
    }

    /// Records what a request cost, against what was estimated for it.
    ///
    /// Called with the messages still exactly as they were sent — before the
    /// answer is pushed — because the estimate has to be of the same thing the
    /// provider counted. `estimated_overhead` is the caller's estimate of the
    /// fixed part of that same request, and it is the anchor that lets the two
    /// unknowns in [`Measured`] be told apart.
    ///
    /// A report of zero is not a measurement. Every backend here reports usage
    /// on a completed stream, but a cancelled one, a gateway that strips the
    /// field, and a prompted-tools fallback can all leave it empty, and taking
    /// that at face value would say the entire prompt cost nothing.
    pub fn record_request(&mut self, input_tokens: u32, estimated_overhead: u32) {
        if input_tokens == 0 {
            return;
        }
        self.last_request = Some(Measured {
            input_tokens,
            estimated_messages: self.estimated_tokens(),
            estimated_overhead,
        });
    }

    /// What to multiply an estimate of history by to get what the provider
    /// would count.
    ///
    /// Everything in the last answered request that was not its fixed part was
    /// messages, so the ratio of that to what those messages were estimated at
    /// is how wrong four-characters-a-token is on *this* conversation's
    /// particular mix of prose, code and JSON. Code and JSON run denser than
    /// four, so on a working session this is normally above one.
    ///
    /// 1.0 until there is a reading worth trusting, which is the honest answer
    /// rather than a cautious one: with no measurement, the estimate is all
    /// there is.
    pub fn calibration(&self) -> f32 {
        let Some(m) = self.last_request else {
            return 1.0;
        };
        // No anchor, nothing to divide, or an anchor that claims the fixed
        // part was the whole request. The last is not arithmetic being
        // careful: it is what a stale reading looks like after the tool set
        // grew, and dividing through it would invert the correction.
        if m.estimated_overhead == 0
            || m.estimated_messages < MIN_CALIBRATION_SAMPLE
            || m.input_tokens <= m.estimated_overhead
        {
            return 1.0;
        }
        let counted = (m.input_tokens - m.estimated_overhead) as f32;
        let ratio = counted / m.estimated_messages as f32;
        ratio.clamp(CALIBRATION_BOUNDS.0, CALIBRATION_BOUNDS.1)
    }

    /// An estimate of some or all of the history, corrected by
    /// [`Self::calibration`].
    pub fn calibrated(&self, estimate: u32) -> u32 {
        // Saturating: an `f32` past `u32::MAX` casts to `u32::MAX` rather than
        // wrapping, and the bounds keep it nowhere near either way.
        (estimate as f32 * self.calibration()) as u32
    }

    /// What the next request will cost, as closely as this can be known.
    ///
    /// `overhead` is the fixed part, which the caller estimates directly
    /// because it is holding the two things that make it up. Once a request
    /// has been answered the *messages* half of this is corrected against what
    /// the provider really counted, which is what makes it right on a backend
    /// whose tokenizer disagrees with four-characters-a-token.
    pub fn estimated_prompt_tokens(&self, overhead: u32) -> u32 {
        self.calibrated(self.estimated_tokens())
            .saturating_add(overhead)
    }

    /// Rough token count for the whole history.
    ///
    /// Deliberately an estimate: asking the provider to count costs a round
    /// trip per iteration, and the only decision this feeds is "compact now or
    /// later", which tolerates being wrong by a few percent.
    pub fn estimated_tokens(&self) -> u32 {
        self.messages.iter().map(estimate_message).sum()
    }

    /// Shrinks tool results that history no longer needs verbatim.
    ///
    /// Tool output is most of what a working session holds — a file read, a
    /// grep, a build log — and every byte of it is re-sent on every later
    /// iteration of the turn. Two things make a result safe to shorten:
    ///
    /// - The same question was asked again later and answered, so the earlier
    ///   answer has been superseded by one already in the transcript.
    /// - It is old enough to be outside the tail kept verbatim, in which case
    ///   the model is working from conclusions rather than from the bytes.
    ///
    /// Superseding is the strong claim of the two — it throws the output away
    /// rather than shortening it — so it is fenced in three ways, all of which
    /// have to hold. `repeats_supersede` decides whether asking a given tool
    /// twice means anything: a build log or an MCP call answers a moment, not a
    /// question, and two runs of `cargo test` around a fix are the two halves
    /// of the loop the model is in. No tool that changes the world may have
    /// run in between, or the later read answers a different question than the
    /// earlier one — read, edit, read is the shape that makes this matter. And
    /// the later call must have actually produced a result: a turn canceled
    /// between the model's call and the tool's answer leaves a dangling
    /// `tool_use` in history, and pointing at it would delete the only real
    /// output in favor of a note about a result that is not there.
    ///
    /// The block itself always stays: replacing its text keeps every tool call
    /// paired with a result, which is what providers actually validate. Errors
    /// are left alone — they are short, and they are usually the reason the
    /// next few messages look the way they do.
    ///
    /// Costs no model call, which is the point: this runs before summarizing.
    pub fn trim_tool_results(
        &mut self,
        keep_recent: usize,
        repeats_supersede: &dyn Fn(&str) -> bool,
    ) -> Trimmed {
        let cutoff = self.messages.len().saturating_sub(keep_recent);
        if cutoff == 0 {
            return Trimmed::default();
        }
        let (call_of, last_use) = index_calls(&self.messages);
        let mutations = mutations_before(&self.messages, repeats_supersede);

        let mut trimmed = Trimmed::default();
        for (index, message) in self.messages.iter_mut().enumerate().take(cutoff) {
            for block in &mut message.content {
                let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = block
                else {
                    continue;
                };
                if *is_error {
                    continue;
                }
                let Some(call) = call_of.get(tool_use_id.as_str()) else {
                    continue;
                };

                let superseded = repeats_supersede(&call.name)
                    && last_use.get(call.signature.as_str()).is_some_and(|&last| {
                        last > index && mutations[last + 1] == mutations[index]
                    });
                // Measured in the units the budget is kept in, not in bytes.
                // A result carrying a screenshot is a few hundred bytes of
                // text and the most expensive thing in the window; compared by
                // byte length it would look like the one result not worth
                // shortening, which is exactly backwards.
                let cost = output_chars(content);
                let replacement = if superseded {
                    superseded_note(&call.name)
                } else if cost > squeeze_floor(&call.name) {
                    squeeze(&content.to_text(), &call.name)
                } else {
                    continue;
                };

                // A note longer than what it replaces is not a saving.
                if replacement.len() >= cost {
                    continue;
                }
                trimmed.results += 1;
                trimmed.tokens_saved += ((cost - replacement.len()) / 4) as u32;
                // Replaces the images along with the text, which is the point:
                // reclaiming the window means letting go of the most expensive
                // block in it, not keeping the picture and shortening the
                // caption.
                content.replace_with_text(replacement);
            }
        }
        trimmed
    }
}

/// What one [`Session::trim_tool_results`] pass gave back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trimmed {
    /// Tool results collapsed or shortened.
    pub results: usize,
    /// Estimated tokens recovered, on the same ~4-characters-per-token basis
    /// the compaction trigger uses.
    pub tokens_saved: u32,
}

impl Trimmed {
    pub fn is_empty(&self) -> bool {
        self.results == 0
    }
}

/// How much of a shortened result survives. Enough to keep what the output was
/// *about* — the path, the header line, the first hits — without the body.
const SQUEEZE_HEAD_BYTES: usize = 400;

/// Results at or under this are left alone: the note that would replace one
/// costs most of what shortening it saves.
///
/// It is head-plus-note rather than a flat number because the note names the
/// tool twice, and `mcp__server__tool` names run long enough to push an
/// already-shortened result back over any fixed threshold — which would shorten
/// it again on the next pass, and let a long turn erode its own history a
/// little at a time while reporting the loss with a wrong character count.
/// Measuring the note instead of counting its characters by hand keeps that
/// true if the wording changes.
/// `trimming_twice_changes_nothing_the_second_time` holds it.
fn squeeze_floor(name: &str) -> usize {
    SQUEEZE_HEAD_BYTES + squeeze_note(name, usize::MAX, usize::MAX).len()
}

/// The call a tool result answers.
struct Call {
    name: String,
    /// Tool name plus serialized input. Two calls with the same signature ask
    /// the same question, so the later answer is the one worth keeping.
    signature: String,
}

/// Maps every tool-use id to its call, and every call signature to the last
/// message that made it *and got an answer*.
///
/// Answered is the load-bearing part: a call with no result behind it cannot
/// supersede anything, because there is nothing to point the model at. That
/// covers the canceled turn, whose assistant message is in history while the
/// tool's answer never arrived, and the call that failed, where the earlier
/// result is the only one that ever worked.
fn index_calls(messages: &[Message]) -> (HashMap<String, Call>, HashMap<String, usize>) {
    let answered: std::collections::HashSet<&str> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error: false,
                ..
            } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();

    let mut call_of = HashMap::new();
    let mut last_use = HashMap::new();
    for (index, message) in messages.iter().enumerate() {
        for (id, name, input) in message.tool_uses() {
            let signature = format!("{name}\u{0}{input}");
            if answered.contains(id) {
                last_use.insert(signature.clone(), index);
            }
            call_of.insert(
                id.to_string(),
                Call {
                    name: name.to_string(),
                    signature,
                },
            );
        }
    }
    (call_of, last_use)
}

/// A running count of world-changing tool calls, indexed so that
/// `counts[b] - counts[a]` is how many ran in messages `a..b`.
///
/// Two identical reads only ask the same question if nothing moved underneath
/// them. Anything whose repeat does not supersede — a write, a command, an MCP
/// call — is treated as having moved something, which is the conservative
/// reading and the correct one for `edit_file`.
fn mutations_before(messages: &[Message], repeats_supersede: &dyn Fn(&str) -> bool) -> Vec<usize> {
    let mut counts = Vec::with_capacity(messages.len() + 1);
    let mut running = 0;
    counts.push(0);
    for message in messages {
        running += message
            .tool_uses()
            .filter(|(_, name, _)| !repeats_supersede(name))
            .count();
        counts.push(running);
    }
    counts
}

fn superseded_note(name: &str) -> String {
    format!(
        "[dropped: `{name}` was called again later with the same input and nothing changed the \
         answer in between, so that result — further down — is this one.]"
    )
}

fn squeeze(content: &str, name: &str) -> String {
    let head = head_lines(content, SQUEEZE_HEAD_BYTES);
    let note = squeeze_note(name, content.len() - head.len(), content.len());
    format!("{head}\n{note}")
}

fn squeeze_note(name: &str, dropped: usize, total: usize) -> String {
    format!(
        "[shortened: {dropped} of {total} characters of this older `{name}` result were dropped \
         to fit the context window. Call `{name}` again if you need the rest.]"
    )
}

/// A prefix of `text` no longer than `max` bytes, cut at a line break where
/// there is one to cut at and a character boundary otherwise.
fn head_lines(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    // Cutting mid-line is legible but cutting at one is more so, as long as it
    // does not cost most of the excerpt.
    match text[..end].rfind('\n') {
        Some(newline) if newline > max / 2 => &text[..newline],
        _ => &text[..end],
    }
}

/// ~4 characters per token, the standard approximation for English and code.
///
/// Public so that anything reporting on a transcript — `taurus usage`, the
/// token counter — arrives at the same numbers the compaction trigger does. Two
/// estimators would disagree, and the one the user sees would be the wrong one.
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() / 4) as u32
}

/// How long `value` is as compact JSON, without building the string.
///
/// Exactly what `value.to_string().len()` says — serde_json's own formatter
/// writes it — but to a writer that only counts. The estimates below walk the
/// whole history two or three times an iteration, and every tool call's
/// arguments and every structured result were serialized into a string each
/// time only to be measured and thrown away.
fn json_len(value: &serde_json::Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    // A writer that only counts cannot fail, and a `Value` always serializes.
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}

/// What a tool's answer costs, in the character units everything here budgets
/// in.
///
/// An image inside a tool result is charged the same flat 4,000 characters an
/// attached one is, and for the same reason: its real cost has nothing to do
/// with the length of its base64. Counting the base64 instead would put a
/// small screenshot at ten times what the model is billed for it and a large
/// one at a hundred, and the compaction trigger reading those numbers would
/// summarize a conversation that had plenty of room left.
pub fn output_chars(output: &taurus_provider::ToolOutput) -> usize {
    use taurus_provider::ToolResultBlock;
    output
        .as_slice()
        .iter()
        .map(|block| match block {
            ToolResultBlock::Text { text } => text.len(),
            ToolResultBlock::Json { value } => json_len(value),
            ToolResultBlock::Image { .. } => 4000,
        })
        .sum()
}

/// What a single block costs, on the same basis.
pub fn estimate_block(block: &ContentBlock) -> u32 {
    match block {
        ContentBlock::Text { text } | ContentBlock::Thinking { text, .. } => estimate_tokens(text),
        ContentBlock::ToolResult { content, .. } => (output_chars(content) / 4) as u32,
        ContentBlock::ToolUse { name, input, .. } => {
            estimate_tokens(name) + (json_len(input) / 4) as u32
        }
        // Images cost far more than their base64 length suggests; a flat
        // estimate is closer than counting characters.
        ContentBlock::Image { .. } => 1000,
    }
}

/// What a whole message costs, blocks plus the envelope around them.
///
/// Public for the same reason [`estimate_tokens`] is: `taurus usage` reports on
/// a transcript the compaction trigger is also measuring, and summing blocks
/// without the envelope would have the two disagree by four tokens a message —
/// small, and exactly the kind of small that makes a user distrust the number.
pub fn estimate_message(message: &Message) -> u32 {
    let chars: usize = message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } | ContentBlock::Thinking { text, .. } => text.len(),
            ContentBlock::ToolResult { content, .. } => output_chars(content),
            ContentBlock::ToolUse { name, input, .. } => name.len() + json_len(input),
            ContentBlock::Image { .. } => 4000,
        })
        .sum();
    // Per-message envelope overhead (role tokens, delimiters).
    (chars / 4) as u32 + 4
}

/// Splits history into the part to summarize and the part to keep verbatim.
///
/// The tail is kept whole, and never split between an assistant's tool call
/// and the result that answers it: a dangling tool result confuses every
/// provider and a dangling tool call makes some of them error outright.
pub fn split_for_compaction(
    messages: &[Message],
    keep_recent: usize,
    keep_tokens: u32,
) -> (usize, usize) {
    let kept = tail_within(messages, keep_recent, keep_tokens);
    if messages.len() <= kept {
        return (0, messages.len());
    }
    let mut boundary = messages.len() - kept;

    // Walk forward off any tool result whose call would be left behind.
    while boundary < messages.len() && starts_with_tool_result(&messages[boundary]) {
        boundary += 1;
    }
    (boundary, messages.len() - boundary)
}

/// The fewest recent messages the model is left with, whatever they cost.
///
/// Below this the tail stops being context and starts being a fragment: one
/// exchange is the call the model just made and the answer it got, and taking
/// that away to save room would leave it summarizing its way around the thing
/// it is in the middle of.
const MIN_KEEP: usize = 2;

/// How many of the most recent messages to keep verbatim.
///
/// A count alone cannot promise progress. Eight recent messages can be eight
/// large tool results — a shape that got likelier when a search grew the
/// option to bring context back with it — and a tail bigger than the budget
/// means compaction summarizes the head, achieves nothing, and is asked for
/// again on the next iteration, and the one after that.
///
/// So the count is a ceiling and the tokens are the real bound, with a floor
/// under both: whichever of the three binds first wins.
fn tail_within(messages: &[Message], keep_recent: usize, keep_tokens: u32) -> usize {
    let mut kept = 0usize;
    let mut spent = 0u32;
    for message in messages.iter().rev().take(keep_recent) {
        let cost = estimate_message(message);
        if kept >= MIN_KEEP && spent.saturating_add(cost) > keep_tokens {
            break;
        }
        spent = spent.saturating_add(cost);
        kept += 1;
    }
    kept
}

fn starts_with_tool_result(message: &Message) -> bool {
    message.role == Role::User
        && message
            .content
            .first()
            .is_some_and(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the context estimate costs over a long session: 300 messages, a
    /// hundred `write_file` calls with 1 KB of arguments and a 4 KB result
    /// each. The loop walks the whole history two or three times an iteration,
    /// so this grows with the session. Ignored, because only a release build's
    /// numbers mean anything. See `docs/development.md`.
    #[test]
    #[ignore]
    fn estimate_cost_on_a_long_session() {
        use taurus_provider::{Role, ToolOutput};
        let mut session = Session::new("m");
        let content = "fn main() { println!(\"hello\"); }\n".repeat(28);
        for turn in 0..100 {
            session.push(Message::user(format!(
                "turn {turn}: change the handler and run the tests"
            )));
            session.push(Message::new(
                Role::Assistant,
                vec![ContentBlock::tool_use(
                    format!("t{turn}"),
                    "write_file",
                    serde_json::json!({"path": format!("src/module_{turn}.rs"), "content": content}),
                )],
            ));
            let output = if turn % 2 == 0 {
                ToolOutput::text("x".repeat(4096))
            } else {
                ToolOutput::json(serde_json::json!({
                    "columns": ["name", "count", "share"],
                    "rows": (0..40)
                        .map(|i| serde_json::json!([format!("item {i}"), i * 7, i as f64 / 40.0]))
                        .collect::<Vec<_>>(),
                }))
            };
            session.push(Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: format!("t{turn}"),
                    content: output,
                    is_error: false,
                }],
            ));
        }
        let first = session.estimated_tokens();
        let t = std::time::Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(session.estimated_tokens());
        }
        eprintln!(
            "1,000 walks over {} messages ({first} tokens): {:.1?}",
            session.messages.len(),
            t.elapsed()
        );
    }

    #[test]
    fn json_len_is_what_serializing_would_have_measured() {
        use serde_json::json;
        for value in [
            json!(null),
            json!(true),
            json!(0),
            json!(-12),
            json!(u64::MAX),
            json!(i64::MIN),
            json!(1.5),
            json!(1e-7),
            json!(f64::MAX),
            json!(""),
            json!("quote \" backslash \\ newline \n tab \t bell \u{7}"),
            json!("ünïcödé ✓ 🦀"),
            json!([]),
            json!({}),
            json!([1, "two", [3], {"four": 4}]),
            json!({"path": "src/lib.rs", "content": "fn main() {}\n", "nested": {"a": [null, false, 2.25]}}),
        ] {
            assert_eq!(json_len(&value), value.to_string().len(), "{value}");
        }
    }

    fn tool_call() -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "t1",
                "read_file",
                serde_json::json!({}),
            )],
        )
    }

    fn tool_result() -> Message {
        Message::new(Role::User, vec![ContentBlock::tool_result("t1", "body")])
    }

    fn call(id: &str, name: &str, path: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                id,
                name,
                serde_json::json!({ "path": path }),
            )],
        )
    }

    fn result(id: &str, body: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::tool_result(id, body)])
    }

    /// Stands in for the registry: the read-only tools whose repeat asks the
    /// same question again. Everything else — `shell`, `edit_file`, an MCP
    /// call — answers a moment rather than a question.
    fn reads(name: &str) -> bool {
        matches!(name, "read_file" | "grep" | "list_dir")
    }

    fn body_of(message: &Message) -> String {
        match &message.content[0] {
            ContentBlock::ToolResult { content, .. } => content.to_text().into_owned(),
            other => panic!("expected a tool result, got {other:?}"),
        }
    }

    /// Long enough to be worth shortening.
    fn bulky() -> String {
        "some output line\n".repeat(200)
    }

    #[test]
    fn a_repeated_call_drops_the_earlier_answer() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "read_file", "a.rs"),
            result("t2", &bulky()),
            Message::assistant("done"),
        ];
        let trimmed = session.trim_tool_results(2, &reads);
        assert!(!trimmed.is_empty());
        assert!(body_of(&session.messages[1]).contains("called again later"));
        // The answer that is still current survives untouched.
        assert_eq!(body_of(&session.messages[3]), bulky());
    }

    #[test]
    fn a_different_input_is_not_a_repeat() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", "short"),
            call("t2", "read_file", "b.rs"),
            result("t2", "short"),
            Message::assistant("done"),
        ];
        session.trim_tool_results(2, &reads);
        assert_eq!(body_of(&session.messages[1]), "short");
    }

    #[test]
    fn an_old_bulky_result_keeps_its_head_and_says_what_went() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "shell", "x"),
            result("t1", &bulky()),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        let trimmed = session.trim_tool_results(2, &reads);
        assert_eq!(trimmed.results, 1);
        assert!(trimmed.tokens_saved > 0);

        let body = body_of(&session.messages[1]);
        assert!(body.starts_with("some output line"), "{body}");
        assert!(body.contains("shortened"), "{body}");
        assert!(body.len() < bulky().len() / 2, "{body}");
    }

    /// The trigger fires on every iteration once history is large, so this runs
    /// repeatedly over the same messages. A second pass that shortened them
    /// again would erode the conversation a little at a time until nothing of
    /// it was left.
    ///
    /// The long name is the case that matters: the note names its tool twice,
    /// so a fixed threshold that clears head-plus-note for `shell` does not
    /// clear it for anything namespaced, and those results would shrink a
    /// little on every pass while reporting a character count that was already
    /// wrong.
    #[test]
    fn trimming_twice_changes_nothing_the_second_time() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "read_file", "a.rs"),
            result("t2", &bulky()),
            call("t3", "shell", "cargo test"),
            result("t3", &bulky()),
            call(
                "t4",
                "mcp__documentation__search_reference_articles",
                "a.rs",
            ),
            result("t4", &bulky()),
            Message::assistant("a"),
            Message::assistant("b"),
        ];

        assert!(!session.trim_tool_results(2, &reads).is_empty());
        let after_one = session.messages.clone();

        assert!(session.trim_tool_results(2, &reads).is_empty());
        assert_eq!(session.messages, after_one);
    }

    /// The read → edit → read loop. The two reads have the same signature and
    /// different answers, and the first one is the only record of what the file
    /// held before the edit.
    #[test]
    fn a_repeat_across_an_edit_does_not_supersede() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "edit_file", "a.rs"),
            result("t2", "edited"),
            call("t3", "read_file", "a.rs"),
            result("t3", &bulky()),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        session.trim_tool_results(2, &reads);

        let body = body_of(&session.messages[1]);
        assert!(!body.contains("dropped:"), "{body}");
        // Shortened for age is still fine — the head of the pre-edit file
        // survives, which is what the note about it promises.
        assert!(body.starts_with("some output line"), "{body}");
    }

    /// Run a suite, fix, run it again: the first result is the failure the fix
    /// was for, and "this one said the same thing" is the one thing it did not.
    #[test]
    fn a_repeated_command_does_not_supersede() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "shell", "cargo test"),
            result("t1", &bulky()),
            call("t2", "shell", "cargo test"),
            result("t2", &bulky()),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        session.trim_tool_results(2, &reads);

        let body = body_of(&session.messages[1]);
        assert!(!body.contains("dropped:"), "{body}");
        assert!(body.starts_with("some output line"), "{body}");
    }

    /// A turn canceled between the model's call and the tool's answer leaves
    /// the call in history with nothing behind it. Superseding to it would
    /// delete the only real output and point at a result that is not there.
    #[test]
    fn an_unanswered_repeat_supersedes_nothing() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "read_file", "a.rs"),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        session.trim_tool_results(2, &reads);

        let body = body_of(&session.messages[1]);
        assert!(!body.contains("dropped:"), "{body}");
        assert!(body.starts_with("some output line"), "{body}");
    }

    /// A repeat that failed leaves the earlier result the only one that ever
    /// worked.
    #[test]
    fn a_failed_repeat_supersedes_nothing() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "read_file", "a.rs"),
            Message::new(Role::User, vec![ContentBlock::tool_error("t2", "gone")]),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        session.trim_tool_results(2, &reads);

        let body = body_of(&session.messages[1]);
        assert!(!body.contains("dropped:"), "{body}");
        assert!(body.starts_with("some output line"), "{body}");
    }

    #[test]
    fn recent_results_are_left_alone() {
        let mut session = Session::new("m");
        session.messages = vec![call("t1", "shell", "x"), result("t1", &bulky())];
        assert!(session.trim_tool_results(8, &reads).is_empty());
        assert_eq!(body_of(&session.messages[1]), bulky());
    }

    #[test]
    fn a_short_result_is_not_worth_shortening() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "shell", "x"),
            result("t1", "ok"),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        assert!(session.trim_tool_results(2, &reads).is_empty());
        assert_eq!(body_of(&session.messages[1]), "ok");
    }

    #[test]
    fn failures_survive_trimming() {
        let mut session = Session::new("m");
        let message = bulky();
        session.messages = vec![
            call("t1", "shell", "x"),
            Message::new(Role::User, vec![ContentBlock::tool_error("t1", &message)]),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        assert!(session.trim_tool_results(2, &reads).is_empty());
        assert_eq!(body_of(&session.messages[1]), message);
    }

    #[test]
    fn trimming_leaves_every_call_paired_with_a_result() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "read_file", "a.rs"),
            result("t2", &bulky()),
            Message::assistant("done"),
        ];
        let before = session.messages.len();
        session.trim_tool_results(1, &reads);

        assert_eq!(session.messages.len(), before);
        for id in ["t1", "t2"] {
            assert!(
                session.messages.iter().any(|m| m.content.iter().any(|b| {
                    matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id)
                })),
                "the result for {id} must still be there"
            );
        }
    }

    #[test]
    fn trimming_lowers_the_estimate_it_reports_lowering() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "shell", "x"),
            result("t1", &bulky()),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        let before = session.estimated_tokens();
        let trimmed = session.trim_tool_results(2, &reads);
        let after = session.estimated_tokens();

        assert!(after < before);
        // Both round chars down to tokens, just at different points, so they
        // agree to within the rounding rather than exactly.
        assert!(
            (before - after).abs_diff(trimmed.tokens_saved) <= 1,
            "reported {} saved, estimate fell by {}",
            trimmed.tokens_saved,
            before - after
        );
    }

    #[test]
    fn estimate_grows_with_content() {
        let mut session = Session::new("m");
        let small = session.estimated_tokens();
        session.push(Message::user("x".repeat(4000)));
        assert!(session.estimated_tokens() > small + 900);
    }

    #[test]
    fn usage_accumulates_across_turns() {
        let mut session = Session::new("m");
        session.add_usage(TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        });
        session.add_usage(TokenUsage {
            input_tokens: 3,
            output_tokens: 2,
            ..Default::default()
        });
        assert_eq!(session.usage.total(), 20);
    }

    #[test]
    fn usage_keeps_the_cache_and_reasoning_counts() {
        // Summing only input and output told a cached Anthropic session it had
        // read nothing from the cache, however much it had.
        let mut session = Session::new("m");
        session.add_usage(TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_input_tokens: Some(700),
            cache_creation_input_tokens: Some(40),
            reasoning_tokens: Some(12),
        });
        // A turn on a backend that reports none of them adds nothing to them.
        session.add_usage(TokenUsage {
            input_tokens: 3,
            output_tokens: 2,
            ..Default::default()
        });
        assert_eq!(session.usage.cache_read_input_tokens, Some(700));
        assert_eq!(session.usage.cache_creation_input_tokens, Some(40));
        assert_eq!(session.usage.reasoning_tokens, Some(12));
        assert_eq!(session.usage.total(), 20);
    }

    #[test]
    fn a_tail_of_large_messages_is_cut_short_by_its_cost() {
        // Eight recent messages can be eight large tool results, and a tail
        // bigger than the budget means compaction summarizes the head,
        // achieves nothing, and is asked again on the next iteration.
        let messages: Vec<Message> = (0..8)
            .map(|i| Message::user(format!("{i}{}", "x".repeat(4_000))))
            .collect();

        // A thousand tokens buys one of these, and the floor buys the second.
        let (dropped, kept) = split_for_compaction(&messages, 8, 1_000);
        assert_eq!(kept, MIN_KEEP);
        assert_eq!(dropped, messages.len() - MIN_KEEP);

        // Given room, the count is what binds.
        let (_, roomy) = split_for_compaction(&messages, 8, u32::MAX);
        assert_eq!(roomy, 8);
    }

    #[test]
    fn the_tail_never_falls_below_one_exchange() {
        // Below this the tail stops being context and starts being a fragment.
        let messages = vec![
            Message::user("a"),
            Message::assistant("b"),
            Message::user(format!("huge {}", "x".repeat(100_000))),
        ];
        let (_, kept) = split_for_compaction(&messages, 8, 10);
        assert_eq!(kept, MIN_KEEP);
    }

    #[test]
    fn nothing_is_dropped_when_history_is_short() {
        let messages = vec![Message::user("a"), Message::assistant("b")];
        assert_eq!(split_for_compaction(&messages, 6, u32::MAX), (0, 2));
    }

    #[test]
    fn compaction_never_orphans_a_tool_result() {
        // Boundary would land on the tool result, separating it from the call.
        let messages = vec![
            Message::user("a"),
            tool_call(),
            tool_result(),
            Message::assistant("done"),
        ];
        let (dropped, kept) = split_for_compaction(&messages, 2, u32::MAX);
        assert_eq!(dropped + kept, messages.len());
        assert!(
            !starts_with_tool_result(&messages[dropped]),
            "kept history must not begin with an orphan tool result"
        );
    }

    #[test]
    fn consecutive_tool_results_are_all_skipped() {
        let messages = vec![
            Message::user("a"),
            tool_call(),
            tool_result(),
            tool_result(),
            Message::assistant("done"),
        ];
        let (dropped, _) = split_for_compaction(&messages, 3, u32::MAX);
        assert!(!starts_with_tool_result(&messages[dropped]));
    }

    /// A session whose history estimates to at least `tokens`.
    fn sized(tokens: u32) -> Session {
        let mut session = Session::new("m");
        session.push(Message::user("z".repeat(tokens as usize * 4)));
        session
    }

    #[test]
    fn an_unmeasured_session_corrects_nothing() {
        // The estimate is all there is, so it is the answer. Guessing a
        // correction would be guessing at a tokenizer nobody has met yet.
        let session = sized(2_000);
        assert_eq!(session.calibration(), 1.0);
        assert_eq!(session.calibrated(1_000), 1_000);
    }

    #[test]
    fn the_correction_is_what_the_provider_counted_over_what_was_estimated() {
        let mut session = sized(2_000);
        let estimated = session.estimated_tokens();
        // A backend that counted half again what four-characters-a-token
        // predicted for the messages, on top of a 300-token fixed part.
        session.record_request((estimated as f32 * 1.5) as u32 + 300, 300);

        assert!((session.calibration() - 1.5).abs() < 0.01);
        // And it applies to a slice of the history, not only to all of it.
        // That is the whole reason this is a ratio: the reading was taken on
        // the whole conversation, and the question asked of it is about eight
        // messages.
        assert_eq!(session.calibrated(1_000), 1_500);
    }

    #[test]
    fn a_reading_with_no_anchor_corrects_nothing() {
        // What a session stored before the anchor was recorded deserializes
        // to. Correcting by `input_tokens / estimated_messages` here would
        // charge the whole fixed part to the messages and compact a turn that
        // had room.
        let mut session = sized(2_000);
        let estimated = session.estimated_tokens();
        session.last_request = Some(Measured {
            input_tokens: estimated + 4_000,
            estimated_messages: estimated,
            estimated_overhead: 0,
        });
        assert_eq!(session.calibration(), 1.0);
    }

    #[test]
    fn too_little_history_to_read_anything_from_corrects_nothing() {
        // Two short messages against a large fixed part: the ratio here is
        // almost entirely a statement about the anchor's error, amplified by
        // dividing by a small number.
        let mut session = Session::new("m");
        session.push(Message::user("hello"));
        session.record_request(4_200, 4_000);
        assert_eq!(
            session.calibration(),
            1.0,
            "a reading this small is noise, not a measurement"
        );
    }

    #[test]
    fn an_anchor_larger_than_the_whole_request_corrects_nothing() {
        // What a stale reading looks like after the tool set grew. The
        // subtraction would go negative, and a ratio built on it would invert
        // the correction rather than merely get its size wrong.
        let mut session = sized(2_000);
        session.record_request(1_000, 4_000);
        assert_eq!(session.calibration(), 1.0);
    }

    #[test]
    fn a_wild_reading_is_held_inside_the_bounds() {
        let mut session = sized(2_000);
        let estimated = session.estimated_tokens();

        // Far denser than any tokenizer: the anchor must be wrong.
        session.record_request(estimated * 40, 300);
        assert_eq!(session.calibration(), CALIBRATION_BOUNDS.1);

        // And the other way. An estimate that already covers the count is not
        // evidence to spend history on — the floor keeps the budget honest
        // when the anchor is the thing that is off.
        session.record_request(estimated / 2 + 300, 300);
        assert_eq!(session.calibration(), CALIBRATION_BOUNDS.0);
    }

    #[test]
    fn the_prompt_estimate_is_the_corrected_history_plus_the_fixed_part() {
        let mut session = sized(2_000);
        let estimated = session.estimated_tokens();
        session.record_request((estimated as f32 * 1.5) as u32 + 300, 300);

        let expected = (estimated as f32 * 1.5) as u32 + 300;
        let got = session.estimated_prompt_tokens(300);
        assert!(
            got.abs_diff(expected) <= 2,
            "the next request is the same size as the one measured: {got} vs {expected}"
        );
    }
}
