//! Normalized conversation types.
//!
//! These deliberately follow the Anthropic content-block shape rather than the
//! OpenAI `tool_calls` + `role: "tool"` shape. Content blocks are the superset:
//! an OpenAI or Ollama response maps into them without loss, while the reverse
//! direction cannot represent interleaved text, thinking, and multiple tool
//! calls inside a single assistant turn.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// Chain-of-thought emitted by reasoning models. Kept separate from `Text`
    /// so the UI can collapse it and compaction can drop it first.
    Thinking {
        text: String,
        /// Opaque proof-of-origin, for providers that issue one and require it
        /// back verbatim.
        ///
        /// Anthropic is the case: a turn that thought and then called a tool is
        /// only legal on the next request if its thinking blocks come back
        /// signed and unedited, and dropping them is a 400 rather than a
        /// degradation. Carried rather than regenerated because it is a
        /// signature — the whole point is that this harness cannot produce one.
        ///
        /// `None` on every other backend, and on transcripts written before
        /// this field existed, which is why it defaults rather than being
        /// required.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        #[ts(type = "unknown")]
        input: serde_json::Value,
        /// Opaque proof-of-origin for the *call*, for providers that issue one.
        ///
        /// Gemini is the case, and it is separate from the one on `Thinking`:
        /// a model that reasoned its way to a tool call signs the call itself,
        /// and replaying that call without the signature is a 400 naming the
        /// tool and its position in the history. Anthropic signs the thinking
        /// instead, which is why this is a second field rather than the same
        /// one moved.
        ///
        /// `None` on every other backend, and on transcripts written before
        /// this field existed, which is why it defaults rather than being
        /// required.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        signature: Option<String>,
    },
    ToolResult {
        tool_use_id: String,
        content: ToolOutput,
        is_error: bool,
    },
    Image {
        mime_type: String,
        /// Base64, no data-URI prefix.
        data: String,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Reasoning with no signature, which is every provider but Anthropic.
    pub fn thinking(text: impl Into<String>) -> Self {
        Self::Thinking {
            text: text.into(),
            signature: None,
        }
    }

    /// A call with no signature, which is every provider but Gemini.
    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
            signature: None,
        }
    }

    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<ToolOutput>) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error: false,
        }
    }

    /// A failed call. Always text: an error is something to read, and a tool
    /// that failed has nothing to show.
    pub fn tool_error(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: ToolOutput::text(content),
            is_error: true,
        }
    }

    /// Text carried by this block, if any. Tool results count: compaction and
    /// token estimation care about their size.
    ///
    /// A tool result answers only when it is exactly one text block, which is
    /// nearly all of them. A result carrying a picture has no `&str` to lend
    /// out, and inventing one — the empty string, a placeholder — would have
    /// every caller that sizes or searches a transcript quietly skip the
    /// blocks that cost the most. Callers that want text regardless ask
    /// [`ToolOutput::text`] for it and take the allocation.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } | Self::Thinking { text, .. } => Some(text),
            Self::ToolResult { content, .. } => content.as_text(),
            _ => None,
        }
    }
}

/// One piece of what a tool handed back.
///
/// Three kinds because there are three things a tool can mean. Text is prose
/// the model reads. An image is something for it to *look* at — a screenshot,
/// a rendered chart, a page of a PDF — which no amount of describing replaces.
/// JSON is a structure it should parse rather than read, kept apart from text
/// so that intent is stated rather than guessed at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ToolResultBlock {
    Text {
        text: String,
    },
    Image {
        mime_type: String,
        /// Base64, no data-URI prefix — the same encoding
        /// [`ContentBlock::Image`] carries, so nothing is re-encoded on the way
        /// through.
        data: String,
    },
    /// Structure the model should parse rather than read.
    ///
    /// Only ever produced by a tool that asked for it. Text is never reparsed
    /// to find out whether it "looks like" JSON: a `read_file` on a `.json`
    /// would become structured output on that guess, and a tool whose text
    /// happens to start with `{` would change shape depending on its input.
    Json {
        #[ts(type = "unknown")]
        value: serde_json::Value,
    },
}

impl ToolResultBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn image(mime_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self::Image {
            mime_type: mime_type.into(),
            data: data.into(),
        }
    }

    pub fn json(value: serde_json::Value) -> Self {
        Self::Json { value }
    }
}

/// Everything one tool call handed back, in the order the model should read it.
///
/// Never empty. A zero-block result is rejected by every provider at the
/// request boundary, so allowing one here would turn a tool's bug into a failed
/// turn one request later, with nothing left to say which call caused it. A
/// tool with genuinely nothing to report says so with one empty text block.
///
/// Held as a list rather than a string because that is what the wire formats
/// underneath already are. Anthropic takes a `tool_result` whose content is a
/// list of blocks including images; the previous `String` could not express
/// that, so a tool that took a screenshot had no way to hand it over and an
/// MCP server that returned one had it flattened to the words
/// `[image: image/png]`.
#[derive(Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct ToolOutput(Vec<ToolResultBlock>);

impl ToolOutput {
    /// One text block, which is what nearly every tool returns.
    pub fn text(text: impl Into<String>) -> Self {
        Self(vec![ToolResultBlock::text(text)])
    }

    /// One image and nothing else.
    pub fn image(mime_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self(vec![ToolResultBlock::image(mime_type, data)])
    }

    /// One structured value the model should parse.
    pub fn json(value: serde_json::Value) -> Self {
        Self(vec![ToolResultBlock::json(value)])
    }

    /// Several blocks, in reading order.
    ///
    /// The one funnel every multi-block construction passes through, which is
    /// why the emptiness check lives here — a tool returning nothing, and a
    /// caller rewriting a result down to nothing, are both caught where the
    /// mistake was made rather than at the next request.
    pub fn blocks(blocks: Vec<ToolResultBlock>) -> Result<Self, &'static str> {
        if blocks.is_empty() {
            return Err(
                "a tool result has no content; return at least one block — an empty text block \
                 is valid",
            );
        }
        Ok(Self(blocks))
    }

    pub fn as_slice(&self) -> &[ToolResultBlock] {
        &self.0
    }

    /// The literal text, when this is exactly one text block.
    ///
    /// `None` for anything carrying a picture or a structure, so a caller that
    /// wanted a borrow has to decide what to do about the rest rather than
    /// silently treating it as absent.
    pub fn as_text(&self) -> Option<&str> {
        match self.0.as_slice() {
            [ToolResultBlock::Text { text }] => Some(text),
            _ => None,
        }
    }

    /// Everything the model would read, as one string.
    ///
    /// Images become a line naming what they are rather than being dropped: a
    /// caller flattening a result is usually measuring it, logging it, or
    /// showing it to a person, and a screenshot that leaves no trace in any of
    /// the three is a result that appears to have returned nothing.
    pub fn to_text(&self) -> std::borrow::Cow<'_, str> {
        if let Some(text) = self.as_text() {
            return std::borrow::Cow::Borrowed(text);
        }
        let joined = self
            .0
            .iter()
            .map(|block| match block {
                ToolResultBlock::Text { text } => text.clone(),
                ToolResultBlock::Image { mime_type, .. } => format!("[image: {mime_type}]"),
                ToolResultBlock::Json { value } => value.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::borrow::Cow::Owned(joined)
    }

    /// Adds a text block at the end.
    ///
    /// Its own block rather than appended to the last one: the last block may
    /// be an image, which has no text to extend, or JSON, which stops being
    /// JSON the moment a note is welded to it.
    pub fn push_text(&mut self, text: impl Into<String>) {
        self.0.push(ToolResultBlock::text(text));
    }

    /// Replaces everything with one text block.
    ///
    /// What trimming and superseding do. Deliberately drops any image along
    /// with the text: the whole point of shortening an old result is to get the
    /// context back, and a picture is the most expensive thing in it.
    pub fn replace_with_text(&mut self, text: impl Into<String>) {
        self.0 = vec![ToolResultBlock::text(text)];
    }

    pub fn images(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().filter_map(|block| match block {
            ToolResultBlock::Image { mime_type, data } => Some((mime_type.as_str(), data.as_str())),
            _ => None,
        })
    }

    pub fn has_images(&self) -> bool {
        self.images().next().is_some()
    }

    /// Everything except the images, or `None` if that would be nothing.
    ///
    /// For a model that cannot see. See [`ToolOutput::without_images`] for why
    /// the caller usually wants that instead.
    pub fn text_blocks_only(&self) -> Option<Self> {
        let kept: Vec<ToolResultBlock> = self
            .0
            .iter()
            .filter(|b| !matches!(b, ToolResultBlock::Image { .. }))
            .cloned()
            .collect();
        Self::blocks(kept).ok()
    }

    /// This result as a model with no vision should see it.
    ///
    /// Each image becomes a line saying what was there. Dropping them outright
    /// would leave a tool that returned only a screenshot answering with
    /// nothing at all, which reads to the model as a tool that did not work —
    /// and the right conclusion is that it worked and this model cannot look at
    /// the answer.
    pub fn without_images(&self) -> Self {
        if !self.has_images() {
            return self.clone();
        }
        let blocks: Vec<ToolResultBlock> = self
            .0
            .iter()
            .map(|block| match block {
                ToolResultBlock::Image { mime_type, .. } => ToolResultBlock::text(format!(
                    "[an image of type {mime_type}, which this model cannot read]"
                )),
                other => other.clone(),
            })
            .collect();
        Self(blocks)
    }

    /// This result as a backend with text-only tool messages must send it:
    /// what the tool message can carry, and the images that cannot ride in it.
    ///
    /// Only Anthropic takes a picture inside a `tool_result`. OpenAI's
    /// `role: "tool"`, Gemini's `functionResponse` and Ollama's tool message
    /// are all text, so an image handed back by a tool has to travel as a
    /// separate user message placed immediately after — near enough that the
    /// model reads the two as one answer.
    ///
    /// The text keeps a line where each image was, rather than the images being
    /// quietly moved. A tool that returns "here is the chart" and a chart would
    /// otherwise appear to have returned only the sentence, and the picture
    /// arriving afterwards would read as something the user sent.
    ///
    /// Adapters must place the returned images directly after the tool message,
    /// or the association is lost.
    ///
    /// Says nothing about failure. Whether an error is marked by a flag, a key,
    /// or a prefix on the text is the wire format's business and differs on
    /// every one of them — Gemini answers under an `error` key that would read
    /// as `{"error": "Error: boom"}` if this decided.
    pub fn split_relocating_images(&self) -> (String, Vec<(&str, &str)>) {
        let mut relocated = Vec::new();
        let mut lines = Vec::new();
        for block in &self.0 {
            match block {
                ToolResultBlock::Text { text } => lines.push(text.clone()),
                ToolResultBlock::Json { value } => lines.push(value.to_string()),
                ToolResultBlock::Image { mime_type, data } => {
                    relocated.push((mime_type.as_str(), data.as_str()));
                    lines.push(format!(
                        "[image {} of type {mime_type}, in the message that follows]",
                        relocated.len()
                    ));
                }
            }
        }
        (lines.join("\n"), relocated)
    }
}

impl std::fmt::Display for ToolOutput {
    /// Exactly [`ToolOutput::to_text`], for the places a result is being read
    /// rather than sent: a log line, a test assertion, an error message.
    ///
    /// Note what this is *not* for. An adapter building a request must never
    /// reach for it — every backend but Anthropic needs the images lifted out
    /// into a message of their own, and that is
    /// [`ToolOutput::split_relocating_images`]. Flattening here would send the
    /// words `[image: image/png]` and silently drop the picture, which is the
    /// exact bug this type was introduced to fix.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_text())
    }
}

impl From<String> for ToolOutput {
    fn from(text: String) -> Self {
        Self::text(text)
    }
}

impl From<&str> for ToolOutput {
    fn from(text: &str) -> Self {
        Self::text(text)
    }
}

impl Serialize for ToolOutput {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ToolOutput {
    /// Reads the block list, and also the bare string this field used to be.
    ///
    /// Not a courtesy. Every transcript already on disk holds the string form,
    /// and transcripts are not a cache — a conversation that will not load is a
    /// conversation lost, and the checkpoint log that makes a turn rewindable
    /// is keyed to the messages beside it. So the old shape reads as one text
    /// block, forever, and nothing has to be migrated.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Blocks(Vec<ToolResultBlock>),
            Text(String),
        }

        match Either::deserialize(deserializer)? {
            // An empty list in a hand-written transcript reads as an empty
            // result rather than failing the load: refusing to open the
            // conversation would be a far worse answer to a stray `[]` than
            // showing a call that returned nothing.
            Either::Blocks(blocks) if blocks.is_empty() => Ok(Self::text("")),
            Either::Blocks(blocks) => Ok(Self(blocks)),
            Either::Text(text) => Ok(Self::text(text)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self { role, content }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![ContentBlock::text(text)])
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, vec![ContentBlock::text(text)])
    }

    /// Concatenated plain text of the message, ignoring thinking and tool blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &serde_json::Value)> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse {
                id, name, input, ..
            } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }

    pub fn has_tool_use(&self) -> bool {
        self.tool_uses().next().is_some()
    }
}

/// The line that carries a relocated tool image, naming the call it came from.
///
/// Every backend but Anthropic needs one, so it is written once here and used
/// from all of them: the wording is what tells the model this picture is the
/// answer to a tool call rather than something the user just sent, and three
/// adapters phrasing that differently would be three different behaviours from
/// one harness.
pub fn relocated_note(tool: Option<&str>, count: usize) -> String {
    let what = if count == 1 { "image" } else { "images" };
    match tool {
        Some(name) => format!("The {what} returned by the `{name}` call above."),
        None => format!("The {what} returned by the tool call above."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_block() -> ToolResultBlock {
        ToolResultBlock::image("image/png", "aGVsbG8=")
    }

    #[test]
    fn a_transcript_written_before_blocks_existed_still_loads() {
        // The one non-negotiable property. Every conversation already on disk
        // holds the string form, and a transcript is not a cache — the
        // checkpoint log that makes a turn rewindable is keyed to the messages
        // beside it, so a transcript that will not parse is a turn that can no
        // longer be undone.
        let old =
            r#"{"type":"tool_result","tool_use_id":"t1","content":"459 lines","is_error":false}"#;
        let block: ContentBlock = serde_json::from_str(old).expect("the old shape must load");
        let ContentBlock::ToolResult { content, .. } = &block else {
            panic!("expected a tool result");
        };
        assert_eq!(content.as_text(), Some("459 lines"));
    }

    #[test]
    fn a_result_written_today_reads_back_as_it_was_written() {
        let original = ToolOutput::blocks(vec![
            ToolResultBlock::text("here is the chart"),
            image_block(),
        ])
        .expect("two blocks");
        let json = serde_json::to_string(&original).expect("serializes");
        let round_tripped: ToolOutput = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(round_tripped, original);
    }

    #[test]
    fn a_stray_empty_list_reads_as_an_empty_result_rather_than_failing_the_load() {
        // Not reachable through this crate — `blocks` refuses it — but a
        // hand-edited transcript is a thing, and refusing to open the
        // conversation would be a far worse answer to a stray `[]`.
        let output: ToolOutput = serde_json::from_str("[]").expect("loads");
        assert_eq!(output.as_text(), Some(""));
    }

    #[test]
    fn a_result_with_no_blocks_is_refused_where_it_is_built() {
        // Every provider rejects an empty tool result at the request boundary.
        // Caught here it names the tool that produced it; caught there it is a
        // failed turn with nothing to say which call caused it.
        assert!(ToolOutput::blocks(Vec::new()).is_err());
        // A tool with genuinely nothing to report still has a way to say so.
        assert_eq!(ToolOutput::text("").as_text(), Some(""));
    }

    #[test]
    fn text_is_never_reparsed_as_json_to_guess_what_it_meant() {
        // A `read_file` on a `.json` returns JSON-shaped text, and it is text.
        // Guessing otherwise would make a tool's output change shape depending
        // on the file it was pointed at.
        let output = ToolOutput::text(r#"{"a": 1}"#);
        assert!(matches!(output.as_slice(), [ToolResultBlock::Text { .. }]));
        // Structure is opted into, and then it stays structure.
        let structured = ToolOutput::json(serde_json::json!({"a": 1}));
        assert!(matches!(
            structured.as_slice(),
            [ToolResultBlock::Json { .. }]
        ));
    }

    #[test]
    fn splitting_for_a_text_only_backend_leaves_a_marker_where_each_image_was() {
        // The picture moves to the next message; if the text did not say so,
        // the tool would appear to have returned only the sentence and the
        // image would read as something the user sent.
        let output = ToolOutput::blocks(vec![
            ToolResultBlock::text("the chart you asked for"),
            image_block(),
        ])
        .expect("two blocks");

        let (text, relocated) = output.split_relocating_images();
        assert!(text.contains("the chart you asked for"));
        assert!(text.contains("[image 1 of type image/png"), "{text}");
        assert_eq!(relocated, vec![("image/png", "aGVsbG8=")]);
    }

    #[test]
    fn a_text_only_result_is_split_into_itself_and_nothing() {
        let output = ToolOutput::text("459 lines");
        let (text, relocated) = output.split_relocating_images();
        assert_eq!(text, "459 lines");
        assert!(relocated.is_empty());
    }

    #[test]
    fn a_model_that_cannot_see_is_told_what_was_there() {
        // Dropping the block would leave a tool that returned only a screenshot
        // answering with nothing at all, which reads as a tool that did not
        // work. It worked; this model cannot look at the answer.
        let output = ToolOutput::blocks(vec![image_block()]).expect("one block");
        let flattened = output.without_images();
        assert!(!flattened.has_images());
        assert!(
            flattened.to_text().contains("cannot read"),
            "{}",
            flattened.to_text()
        );
    }

    #[test]
    fn a_note_is_appended_as_its_own_block() {
        // Welded onto a JSON block it would stop being JSON, which is the one
        // thing a tool returning JSON was promised.
        let mut output = ToolOutput::json(serde_json::json!({"ok": true}));
        output.push_text("[taurus] a note");
        assert_eq!(output.as_slice().len(), 2);
        assert!(matches!(output.as_slice()[0], ToolResultBlock::Json { .. }));
    }

    #[test]
    fn trimming_a_result_lets_go_of_the_picture_too() {
        // The whole point of shortening an old result is to get the context
        // back, and the image is the most expensive thing in it.
        let mut output =
            ToolOutput::blocks(vec![ToolResultBlock::text("chart"), image_block()]).expect("two");
        output.replace_with_text("[taurus] output dropped; call again if needed");
        assert!(!output.has_images());
        assert_eq!(output.as_slice().len(), 1);
    }
}
