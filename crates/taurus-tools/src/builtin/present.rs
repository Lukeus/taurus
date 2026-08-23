//! The tools that address the person watching rather than the machine.
//!
//! Everything else here changes files, runs programs, or reads something back.
//! These produce nothing but a view: a table, a chart, a diagram, a question. That makes them the only tools whose *output* is not the point —
//! what matters is what [`crate::view::TranscriptView`] the call carries, and
//! the string handed back to the model is only there so it knows the drawing
//! happened.
//!
//! Their input schemas are their view payloads, unchanged. See
//! [`crate::view`] for why that identity is worth preserving.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};
use crate::view::{
    Answer, Asker, Column, FlowEdge, FlowStage, Question, SequenceMessage, Series, TranscriptView,
};

/// Rows one call may draw.
///
/// Not a context-window limit — the rows are drawn, not sent back to the model,
/// so they cost the model nothing after the call. It is a reading limit: past a
/// few dozen rows a table in a conversation is worse than the file it came
/// from, and the honest answer is to write the file and say where it is.
const MAX_ROWS: usize = 60;

/// Bars one chart may hold. Beyond this they are too narrow to read, and the
/// shape being asked about is a table's job rather than a chart's.
const MAX_BARS: usize = 40;

/// Lanes one sequence diagram may have.
///
/// A reading limit rather than a drawing one. Past this the lanes are too
/// narrow for their labels in the app and too wide for a terminal, and a
/// conversation between nine things is one that wants breaking into two
/// diagrams anyway.
const MAX_PARTICIPANTS: usize = 8;

/// Arrows one sequence diagram may carry. Beyond this it is a log, and a log
/// reads better as a table.
const MAX_MESSAGES: usize = 40;

/// Questions one `ask_user` call may put.
///
/// Deliberately small. A card with eight questions on it is a form, and a form
/// is what the user opened an agent to avoid filling in.
const MAX_QUESTIONS: usize = 4;

/// Named as constants because these are registered per turn rather than
/// into the shared registry, so `taurus-host` has to be able to name them
/// without a literal in two files that can drift apart.
pub const SHOW_TABLE_TOOL: &str = "show_table";
pub const SHOW_CHART_TOOL: &str = "show_chart";
pub const SHOW_SEQUENCE_TOOL: &str = "show_sequence";
pub const SHOW_FLOW_TOOL: &str = "show_flow";
pub const ASK_USER_TOOL: &str = "ask_user";

// ---------------------------------------------------------------- show_table

#[derive(Deserialize, JsonSchema)]
pub struct ShowTableInput {
    /// What the table is of, as a short noun phrase — `Crates by build time`.
    pub title: String,
    /// Where the numbers came from, in one line. Name the command or file, so
    /// the reader can check them.
    #[serde(default)]
    pub caption: Option<String>,
    /// The columns, left to right.
    pub columns: Vec<Column>,
    /// One inner array per row, with one cell per column, already formatted for
    /// reading — `42.1s`, `+22%`, `—` for nothing. Rows are sorted by the
    /// reader, so send them in whatever order is natural.
    pub rows: Vec<Vec<String>>,
}

/// Draws a table in the transcript.
pub struct ShowTable;

#[async_trait]
impl Tool for ShowTable {
    fn name(&self) -> &str {
        SHOW_TABLE_TOOL
    }

    fn description(&self) -> &str {
        "Draw a sortable table in the conversation. Use it when the answer is several rows of \
         comparable facts — files by size, crates by build time, endpoints by error rate — and the \
         comparison is the point. The reader can sort it by any column and copy it as CSV, which \
         is what makes it worth more than the same rows written as prose. Do not use it for one \
         row, for two columns of prose, or to restate something you already listed in your reply: \
         a table with nothing to compare is harder to read than the sentence it replaced. Say what \
         the table shows in your own words as well — the table is the evidence, not the answer."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ShowTableInput>()
    }

    /// Nothing on the machine changes, so nothing is asked. The user is being
    /// shown something, which needs no permission.
    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Table: {}",
            input.get("title").and_then(|t| t.as_str()).unwrap_or("?")
        )
    }

    fn view(&self, _id: &str, input: &serde_json::Value) -> Option<TranscriptView> {
        let input: ShowTableInput = serde_json::from_value(input.clone()).ok()?;
        // Checked here as well as in `execute`, because the view is drawn
        // first: a ragged table parses perfectly well, and without this the
        // reader would watch a broken one appear and then be told it failed.
        check_table(&input).ok()?;
        Some(TranscriptView::Table {
            title: input.title,
            caption: input.caption,
            columns: input.columns,
            rows: input.rows,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: ShowTableInput = parse_input(input)?;
        check_table(&input)?;
        Ok(format!(
            "Drew '{}' — {} rows over {} columns. The user can see it; do not repeat the rows.",
            input.title,
            input.rows.len(),
            input.columns.len()
        )
        .into())
    }
}

/// Everything about a table that has to hold before it is worth drawing.
///
/// Reported as [`ToolError::InvalidInput`] rather than fixed silently: a row
/// that is one cell short is a model that lost track of its own columns, and
/// padding it would hide that under a table that looks fine and says the wrong
/// thing.
fn check_table(input: &ShowTableInput) -> Result<(), ToolError> {
    if input.columns.is_empty() {
        return Err(ToolError::InvalidInput("a table needs columns".into()));
    }
    if input.rows.is_empty() {
        return Err(ToolError::InvalidInput(
            "a table needs rows; say it in a sentence instead".into(),
        ));
    }
    if input.rows.len() > MAX_ROWS {
        return Err(ToolError::InvalidInput(format!(
            "{} rows is more than a conversation can hold ({MAX_ROWS} at most). Show the rows \
             that answer the question, or write the full set to a file and say where it is.",
            input.rows.len()
        )));
    }
    let width = input.columns.len();
    if let Some((n, row)) = input
        .rows
        .iter()
        .enumerate()
        .find(|(_, r)| r.len() != width)
    {
        return Err(ToolError::InvalidInput(format!(
            "row {} has {} cells but there are {width} columns; every row needs one cell per \
             column, empty string included",
            n + 1,
            row.len()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------- show_chart

#[derive(Deserialize, JsonSchema)]
pub struct ShowChartInput {
    /// What is being plotted — `Tool calls per turn`.
    pub title: String,
    /// Where the numbers came from, in one line.
    #[serde(default)]
    pub caption: Option<String>,
    /// One label per bar, along the bottom. Keep them short: they sit under
    /// bars a few characters wide.
    pub labels: Vec<String>,
    /// One entry per metric. More than one becomes tabs over a single set of
    /// bars, so only send several when they share the same labels and the
    /// reader would want to flip between them.
    pub series: Vec<Series>,
}

/// Draws a bar chart in the transcript.
pub struct ShowChart;

#[async_trait]
impl Tool for ShowChart {
    fn name(&self) -> &str {
        SHOW_CHART_TOOL
    }

    fn description(&self) -> &str {
        "Draw a bar chart in the conversation. Use it when the shape of a series is the answer — \
         where the spike is, whether a number is climbing, which of eight things is the outlier. \
         Use show_table instead when the exact values matter more than their shape, and use a \
         sentence when there are two numbers: a chart of two bars is a comparison the reader \
         could have read faster. Every series must share the same labels."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ShowChartInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Chart: {}",
            input.get("title").and_then(|t| t.as_str()).unwrap_or("?")
        )
    }

    fn view(&self, _id: &str, input: &serde_json::Value) -> Option<TranscriptView> {
        let input: ShowChartInput = serde_json::from_value(input.clone()).ok()?;
        check_chart(&input).ok()?;
        Some(TranscriptView::Chart {
            title: input.title,
            caption: input.caption,
            labels: input.labels,
            series: input.series,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: ShowChartInput = parse_input(input)?;
        check_chart(&input)?;
        Ok(format!(
            "Drew '{}' — {} bars across {} series. The user can see it; do not repeat the values.",
            input.title,
            input.labels.len(),
            input.series.len()
        )
        .into())
    }
}

fn check_chart(input: &ShowChartInput) -> Result<(), ToolError> {
    if input.labels.is_empty() {
        return Err(ToolError::InvalidInput("a chart needs labels".into()));
    }
    if input.labels.len() > MAX_BARS {
        return Err(ToolError::InvalidInput(format!(
            "{} bars is more than a chart this size can show ({MAX_BARS} at most); group them or \
             show the part that answers the question",
            input.labels.len()
        )));
    }
    if input.series.is_empty() {
        return Err(ToolError::InvalidInput("a chart needs a series".into()));
    }
    let bars = input.labels.len();
    if let Some(series) = input.series.iter().find(|s| s.values.len() != bars) {
        return Err(ToolError::InvalidInput(format!(
            "series '{}' has {} values but there are {bars} labels; every series is plotted \
             against the same labels",
            series.name,
            series.values.len()
        )));
    }
    if let Some(series) = input
        .series
        .iter()
        .find(|s| s.values.iter().any(|v| !v.is_finite()))
    {
        return Err(ToolError::InvalidInput(format!(
            "series '{}' contains a value that is not a finite number",
            series.name
        )));
    }
    Ok(())
}

// ------------------------------------------------------------- show_sequence

#[derive(Deserialize, JsonSchema)]
pub struct ShowSequenceInput {
    /// What the exchange is, as a short noun phrase — `Placing an order`.
    pub title: String,
    /// Where this came from, in one line — the module you read it out of, or
    /// that it is the design rather than the code.
    #[serde(default)]
    pub caption: Option<String>,
    /// The participants, in the order they should appear left to right. Put
    /// whoever starts the exchange first. Keep the names short: they head a
    /// lane a few characters wide.
    pub participants: Vec<String>,
    /// The messages in the order they happen, top to bottom. Every `from` and
    /// `to` must be one of the participants above, spelled the same way.
    pub messages: Vec<SequenceMessage>,
}

/// Draws a sequence diagram in the transcript.
pub struct ShowSequence;

#[async_trait]
impl Tool for ShowSequence {
    fn name(&self) -> &str {
        SHOW_SEQUENCE_TOOL
    }

    fn description(&self) -> &str {
        "Draw a sequence diagram in the conversation. Use it when the answer is an order of \
         events between several things — how a request travels through the system, what a \
         handshake exchanges, where a retry loops back, which component calls which and in what \
         order. It is the right shape when the question is 'what happens when…' and the answer \
         would otherwise be a numbered list that the reader has to reassemble into a picture. Do \
         not use it for two participants and one message, for a plain list of steps one thing \
         does on its own — that is a numbered list, or update_plan — or to restate a sequence you \
         have already written out in prose. Say what the diagram shows in your own words as well: \
         the diagram is the shape, not the explanation."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ShowSequenceInput>()
    }

    /// Nothing on the machine changes; the user is being shown something.
    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Sequence: {}",
            input.get("title").and_then(|t| t.as_str()).unwrap_or("?")
        )
    }

    fn view(&self, _id: &str, input: &serde_json::Value) -> Option<TranscriptView> {
        let input: ShowSequenceInput = serde_json::from_value(input.clone()).ok()?;
        // Checked here as well as in `execute`, for the reason `show_table`
        // gives: the view goes out before the call runs, so a diagram naming a
        // participant that does not exist would be drawn with a dangling arrow
        // and only then reported as failed.
        check_sequence(&input).ok()?;
        Some(TranscriptView::Sequence {
            title: input.title,
            caption: input.caption,
            participants: input.participants,
            messages: input.messages,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: ShowSequenceInput = parse_input(input)?;
        check_sequence(&input)?;
        Ok(format!(
            "Drew '{}' — {} messages between {} participants. The user can see it; do not repeat \
             the steps.",
            input.title,
            input.messages.len(),
            input.participants.len()
        )
        .into())
    }
}

/// Everything about a sequence that has to hold before it is worth drawing.
///
/// The one that matters is the last: an arrow to a participant that was never
/// declared has nowhere to land. Refused rather than repaired by adding the
/// lane, because a name that is not in the list is usually a model that spelled
/// one of its own participants two ways, and inventing a ninth lane called
/// `Databse` would draw that mistake as though it were the design.
fn check_sequence(input: &ShowSequenceInput) -> Result<(), ToolError> {
    if input.participants.len() < 2 {
        return Err(ToolError::InvalidInput(
            "a sequence diagram needs at least two participants; one thing doing several things \
             in order is a numbered list"
                .into(),
        ));
    }
    if input.participants.len() > MAX_PARTICIPANTS {
        return Err(ToolError::InvalidInput(format!(
            "{} participants is more than one diagram can hold ({MAX_PARTICIPANTS} at most); show \
             the part of the exchange that answers the question, or split it in two",
            input.participants.len()
        )));
    }
    if let Some(name) = duplicate(&input.participants) {
        return Err(ToolError::InvalidInput(format!(
            "'{name}' is listed as a participant twice; every lane needs its own name, because \
             the messages find their lane by it"
        )));
    }
    if input.messages.is_empty() {
        return Err(ToolError::InvalidInput(
            "a sequence diagram needs messages; say it in a sentence instead".into(),
        ));
    }
    if input.messages.len() > MAX_MESSAGES {
        return Err(ToolError::InvalidInput(format!(
            "{} messages is more than a diagram this size can show ({MAX_MESSAGES} at most); show \
             the part that answers the question, or summarize the repeated stretch as one arrow",
            input.messages.len()
        )));
    }
    for (n, message) in input.messages.iter().enumerate() {
        for end in [&message.from, &message.to] {
            if !input.participants.contains(end) {
                return Err(ToolError::InvalidInput(format!(
                    "message {} names '{end}', which is not one of the participants ({}). Every \
                     arrow starts and ends at a declared participant, spelled the same way.",
                    n + 1,
                    input.participants.join(", ")
                )));
            }
        }
    }
    Ok(())
}

/// The first name that appears twice, if any.
fn duplicate(names: &[String]) -> Option<&String> {
    names
        .iter()
        .enumerate()
        .find(|(i, name)| names[..*i].contains(name))
        .map(|(_, name)| name)
}

// ------------------------------------------------------------------ show_flow

/// Columns one flow diagram may have. Past this the boxes are too narrow to
/// carry a label, and a six-deep chain is one that wants summarizing.
const MAX_STAGES: usize = 6;

/// Boxes one flow diagram may hold, across every stage. A reading limit: past
/// twenty, a picture is a map, and a map wants a page rather than a
/// conversation.
const MAX_NODES: usize = 20;

/// Arrows one flow diagram may hold. Comfortably more than the nodes, because
/// a fan-out is the normal shape and the point.
const MAX_EDGES: usize = 40;

#[derive(Deserialize, JsonSchema)]
pub struct ShowFlowInput {
    /// What the diagram is of — `How a request reaches the database`.
    pub title: String,
    /// Where this came from, in one line — the modules you read it out of, or
    /// that it is the intended design rather than the current code.
    #[serde(default)]
    pub caption: Option<String>,
    /// The columns, left to right, in the order the work moves through them.
    /// Put what starts things in the first stage. Everything at the same depth
    /// belongs in the same stage, which is what makes the picture readable —
    /// so decide the stages first and fill them in, rather than listing nodes
    /// and hoping.
    pub stages: Vec<FlowStage>,
    /// The arrows. `from` and `to` are node labels, spelled exactly as in the
    /// stages. An arrow pointing back to an earlier stage is fine and is drawn
    /// as a loop — that is what a retry or a callback looks like.
    pub edges: Vec<FlowEdge>,
}

/// Draws a flow diagram in the transcript.
pub struct ShowFlow;

#[async_trait]
impl Tool for ShowFlow {
    fn name(&self) -> &str {
        SHOW_FLOW_TOOL
    }

    fn description(&self) -> &str {
        "Draw a flow diagram in the conversation — boxes in stages, arrows between them. Use it \
         when the answer is how a system is put together or how work moves through it: which \
         component talks to which, what a request passes through on its way to the database, the \
         branches and loops of a pipeline. You must group the nodes into stages yourself, left to \
         right, putting everything at the same depth in the same stage — that grouping is what \
         makes the picture readable, and you understand the system well enough to decide it. Use \
         show_sequence instead when the order of events over time is the point rather than the \
         shape of the connections, and use a sentence when there are two boxes and one arrow. Say \
         what the diagram shows in your own words as well."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ShowFlowInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Flow: {}",
            input.get("title").and_then(|t| t.as_str()).unwrap_or("?")
        )
    }

    fn view(&self, _id: &str, input: &serde_json::Value) -> Option<TranscriptView> {
        let input: ShowFlowInput = serde_json::from_value(input.clone()).ok()?;
        check_flow(&input).ok()?;
        Some(TranscriptView::Flow {
            title: input.title,
            caption: input.caption,
            stages: input.stages,
            edges: input.edges,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: ShowFlowInput = parse_input(input)?;
        check_flow(&input)?;
        let nodes: usize = input.stages.iter().map(|s| s.nodes.len()).sum();
        Ok(format!(
            "Drew '{}' — {nodes} nodes across {} stages, {} edges. The user can see it; do not \
             repeat the connections.",
            input.title,
            input.stages.len(),
            input.edges.len()
        )
        .into())
    }
}

/// Everything about a flow that has to hold before it is worth drawing.
///
/// Two of these matter. A duplicate label is refused because labels *are* the
/// identity here — two boxes sharing one would send every arrow to whichever
/// came first, silently. And an edge naming a node that does not exist is
/// refused rather than dropped, for the reason the sequence diagram gives:
/// it is usually one thing spelled two ways, and quietly leaving the arrow out
/// draws a system with a connection missing.
fn check_flow(input: &ShowFlowInput) -> Result<(), ToolError> {
    if input.stages.is_empty() {
        return Err(ToolError::InvalidInput(
            "a flow diagram needs stages: group the nodes by depth, left to right".into(),
        ));
    }
    if input.stages.len() > MAX_STAGES {
        return Err(ToolError::InvalidInput(format!(
            "{} stages is deeper than one diagram can show ({MAX_STAGES} at most); draw the part \
             that answers the question, or collapse a run of stages into one box",
            input.stages.len()
        )));
    }
    if let Some((n, _)) = input
        .stages
        .iter()
        .enumerate()
        .find(|(_, stage)| stage.nodes.is_empty())
    {
        return Err(ToolError::InvalidInput(format!(
            "stage {} has no nodes; every stage is a column and an empty one draws a gap",
            n + 1
        )));
    }

    let labels: Vec<&String> = input
        .stages
        .iter()
        .flat_map(|stage| stage.nodes.iter().map(|node| &node.label))
        .collect();

    if labels.len() < 2 {
        return Err(ToolError::InvalidInput(
            "a flow diagram needs at least two nodes; one box is a noun, not a diagram".into(),
        ));
    }
    if labels.len() > MAX_NODES {
        return Err(ToolError::InvalidInput(format!(
            "{} nodes is more than one diagram can hold ({MAX_NODES} at most); draw the part that \
             answers the question",
            labels.len()
        )));
    }
    if let Some((i, label)) = labels
        .iter()
        .enumerate()
        .find(|(i, label)| labels[..*i].contains(label))
    {
        let _ = i;
        return Err(ToolError::InvalidInput(format!(
            "'{label}' is the label of two different nodes; edges find their box by label, so \
             every one has to be unique — add what tells them apart, like 'Cache (read)' and \
             'Cache (write)'"
        )));
    }

    if input.edges.is_empty() {
        return Err(ToolError::InvalidInput(
            "a flow diagram needs edges; boxes with nothing between them are a list".into(),
        ));
    }
    if input.edges.len() > MAX_EDGES {
        return Err(ToolError::InvalidInput(format!(
            "{} edges is more than a diagram this size can show ({MAX_EDGES} at most)",
            input.edges.len()
        )));
    }
    for (n, edge) in input.edges.iter().enumerate() {
        for end in [&edge.from, &edge.to] {
            if !labels.contains(&end) {
                return Err(ToolError::InvalidInput(format!(
                    "edge {} names '{end}', which is not one of the nodes ({}). Every arrow runs \
                     between two declared nodes, spelled the same way.",
                    n + 1,
                    labels
                        .iter()
                        .map(|l| l.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        if edge.from == edge.to {
            return Err(ToolError::InvalidInput(format!(
                "edge {} runs from '{}' to itself. A box that calls itself is a detail of that \
                 box rather than a connection in the system; put it in the node's note.",
                n + 1,
                edge.from
            )));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ ask_user

#[derive(Deserialize, JsonSchema)]
pub struct AskUserInput {
    /// One to four questions. Ask everything you need in a single call — a
    /// second card after the first is answered reads as an interrogation.
    pub questions: Vec<Question>,
}

/// Puts a question card in the transcript and waits for the answer.
pub struct AskUser {
    asker: Arc<dyn Asker>,
}

impl AskUser {
    pub fn new(asker: Arc<dyn Asker>) -> Self {
        Self { asker }
    }
}

#[async_trait]
impl Tool for AskUser {
    fn name(&self) -> &str {
        ASK_USER_TOOL
    }

    fn description(&self) -> &str {
        "Ask the user to choose between options, and wait for the answer. This is the one \
         exception to working without stopping: use it only when the readings of the request \
         would lead to genuinely different work and picking wrong would waste most of it — which \
         module a rename lands in, whether to migrate the callers or keep a shim, which of three \
         schemas is the real one. Do not use it to confirm a plan, to report progress, to ask \
         whether to continue, or for anything you could settle by reading the code: those are \
         yours to decide, and asking about them is the interruption this tool exists to avoid. \
         Every question may be skipped, so write options you can proceed from and be ready to \
         decide for yourself. Ask before you start the work, not partway through it."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<AskUserInput>()
    }

    /// Nothing is touched, so nothing is gated. The card is its own prompt.
    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let n = input
            .get("questions")
            .and_then(|q| q.as_array())
            .map_or(0, Vec::len);
        format!("Ask {n} question{}", if n == 1 { "" } else { "s" })
    }

    fn view(&self, id: &str, input: &serde_json::Value) -> Option<TranscriptView> {
        let input: AskUserInput = serde_json::from_value(input.clone()).ok()?;
        check_questions(&input).ok()?;
        Some(TranscriptView::Questions {
            id: id.to_string(),
            questions: input.questions,
        })
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: AskUserInput = parse_input(input)?;
        check_questions(&input)?;

        // The card the user is looking at was drawn from this call's id, so the
        // wait has to be registered under the same one or the answer arrives
        // for a call nobody is holding.
        let id = ctx.call_id.as_deref().unwrap_or_default();

        // Without this, Stop leaves the turn parked on a question forever: the
        // card is the only thing still running, and cancelling the token is
        // exactly what the user just asked for.
        //
        // `biased`, so the token is read before the asker is polled at all.
        // Unbiased, the branch order is random — and [`Unattended`] answers
        // `None` on its first poll, so a turn already cancelled would come back
        // "nobody is available to answer" on roughly half of all piped runs
        // rather than stopping. Every other cancellable call here is written
        // this way.
        let answers = tokio::select! {
            biased;
            () = ctx.cancel.cancelled() => return Err(ToolError::Canceled),
            answers = self.asker.ask(id, &input.questions) => answers,
        };

        Ok(match answers {
            Some(answers) => render_answers(&input.questions, &answers).into(),
            // Not an error. A piped run, a git hook, or CI has nobody to ask,
            // and a turn that fails there because it wanted an opinion is worse
            // than one that decides and says so.
            None => "Nobody is available to answer. Decide each of these yourself, and say which \
                     way you went and why in your reply."
                .into(),
        })
    }
}

fn check_questions(input: &AskUserInput) -> Result<(), ToolError> {
    if input.questions.is_empty() {
        return Err(ToolError::InvalidInput("ask at least one question".into()));
    }
    if input.questions.len() > MAX_QUESTIONS {
        return Err(ToolError::InvalidInput(format!(
            "{} questions is more than one card should carry ({MAX_QUESTIONS} at most); ask about \
             the decisions that change the work and decide the rest yourself",
            input.questions.len()
        )));
    }
    if let Some(question) = input.questions.iter().find(|q| q.options.len() < 2) {
        return Err(ToolError::InvalidInput(format!(
            "'{}' offers {} option(s); a question with fewer than two is either a decision you \
             should make or a yes/no you should not be asking",
            question.prompt,
            question.options.len()
        )));
    }
    Ok(())
}

/// The answers as the model reads them.
///
/// Prose rather than JSON, and every question echoed with its answer rather
/// than the answers alone: the model wrote these questions several thousand
/// tokens ago, and a bare list of labels is a puzzle it has to solve before it
/// can act on them. A skipped question says so in words that tell it what to do
/// next, because "unanswered" and "decide it yourself" are the same instruction
/// here and only one of them is actionable.
fn render_answers(questions: &[Question], answers: &[Answer]) -> String {
    let mut out = String::from("The user answered:\n");
    for (i, question) in questions.iter().enumerate() {
        let answer = answers.get(i);
        let line = match answer {
            Some(answer) if !answer.is_empty() => answer.render(),
            _ => "skipped — decide this one yourself and say what you picked".to_string(),
        };
        out.push_str(&format!("{}. {} — {line}\n", i + 1, question.prompt));
    }
    out.push_str(
        "\nGet on with the work now. Do not thank them for answering and do not ask a follow-up \
         question about the same decision.",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_ctx;
    use crate::view::{ColumnKind, MessageKind, QuestionKind, QuestionOption, Unattended};

    fn table(rows: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "title": "Crates by build time",
            "columns": [{ "label": "Crate" }, { "label": "Time", "kind": "number" }],
            "rows": rows,
        })
    }

    #[tokio::test]
    async fn a_table_call_carries_the_table_it_drew() {
        // The view is the whole point of the tool; the string is an aside.
        let (ctx, _dir) = test_ctx();
        let input = table(serde_json::json!([["taurus-core", "42.1s"]]));

        let view = ShowTable.view("call-1", &input).unwrap();
        let result = ShowTable.execute(input, &ctx).await.unwrap();

        assert!(matches!(
            view,
            TranscriptView::Table { ref columns, ref rows, .. }
                if columns[1].kind == ColumnKind::Number && rows.len() == 1
        ));
        assert!(
            result.to_text().contains("do not repeat the rows"),
            "{result}"
        );
    }

    #[tokio::test]
    async fn a_ragged_row_is_refused_rather_than_padded() {
        let (ctx, _dir) = test_ctx();
        let ragged = table(serde_json::json!([["taurus-core"]]));
        let error = ShowTable.execute(ragged.clone(), &ctx).await.unwrap_err();

        assert!(error.to_string().contains("row 1"), "{error}");
        assert!(error.to_string().contains("2 columns"), "{error}");
        // And nothing is drawn on the way to that error. The view goes out
        // before the call runs, so a table that only `execute` rejects would
        // still have appeared, broken, for as long as the call took.
        assert!(ShowTable.view("call-1", &ragged).is_none());
    }

    #[tokio::test]
    async fn a_series_that_does_not_line_up_with_the_labels_is_refused() {
        let (ctx, _dir) = test_ctx();
        let error = ShowChart
            .execute(
                serde_json::json!({
                    "title": "Tool calls per turn",
                    "labels": ["t1", "t2", "t3"],
                    "series": [{ "name": "tool calls", "values": [4.0, 7.0] }],
                }),
                &ctx,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("tool calls"), "{error}");
        assert!(error.to_string().contains("3 labels"), "{error}");
    }

    fn sequence(messages: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "title": "Placing an order",
            "participants": ["Client", "API", "Store"],
            "messages": messages,
        })
    }

    #[tokio::test]
    async fn a_sequence_call_carries_the_diagram_it_drew() {
        let (ctx, _dir) = test_ctx();
        let input = sequence(serde_json::json!([
            { "from": "Client", "to": "API", "text": "POST /orders" },
            { "from": "API", "to": "Store", "text": "insert row" },
            { "from": "Store", "to": "API", "text": "ok", "kind": "return" },
        ]));

        let view = ShowSequence.view("call-1", &input).unwrap();
        let result = ShowSequence.execute(input, &ctx).await.unwrap();

        assert!(matches!(
            view,
            TranscriptView::Sequence { ref participants, ref messages, .. }
                if participants.len() == 3
                    && messages[2].kind == MessageKind::Return
                    // An omitted kind is a call, which is the common case and
                    // the one the model should not have to spell out.
                    && messages[0].kind == MessageKind::Call
        ));
        assert!(
            result.to_text().contains("do not repeat the steps"),
            "{result}"
        );
    }

    #[tokio::test]
    async fn an_arrow_to_an_undeclared_participant_is_refused() {
        // Usually a model that spelled one of its own participants two ways.
        // Adding the lane would draw that mistake as though it were the design.
        let (ctx, _dir) = test_ctx();
        let stray = sequence(serde_json::json!([
            { "from": "Client", "to": "Databse", "text": "insert row" },
        ]));

        let error = ShowSequence.execute(stray.clone(), &ctx).await.unwrap_err();

        assert!(error.to_string().contains("Databse"), "{error}");
        // The declared names are listed, so the misspelling is visible next to
        // what it should have been.
        assert!(error.to_string().contains("Store"), "{error}");
        // And nothing is drawn on the way there: the view goes out before the
        // call runs, so an arrow with nowhere to land must not appear at all.
        assert!(ShowSequence.view("call-1", &stray).is_none());
    }

    #[tokio::test]
    async fn one_participant_is_refused_as_a_list_rather_than_a_diagram() {
        let (ctx, _dir) = test_ctx();
        let error = ShowSequence
            .execute(
                serde_json::json!({
                    "title": "Startup",
                    "participants": ["Host"],
                    "messages": [{ "from": "Host", "to": "Host", "text": "load config" }],
                }),
                &ctx,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("numbered list"), "{error}");
    }

    #[tokio::test]
    async fn a_lane_named_twice_is_refused() {
        // Messages find their lane by name, so two lanes sharing one would send
        // every arrow to whichever was found first.
        let (ctx, _dir) = test_ctx();
        let error = ShowSequence
            .execute(
                serde_json::json!({
                    "title": "Retry",
                    "participants": ["API", "Store", "API"],
                    "messages": [{ "from": "API", "to": "Store", "text": "read" }],
                }),
                &ctx,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("'API'"), "{error}");
        assert!(error.to_string().contains("twice"), "{error}");
    }

    #[tokio::test]
    async fn a_participant_talking_to_itself_is_allowed() {
        // Work a participant does on its own is part of the order of events —
        // it is the self-arrow, not a mistake.
        let (ctx, _dir) = test_ctx();
        let input = sequence(serde_json::json!([
            { "from": "API", "to": "API", "text": "validate the body" },
            { "from": "API", "to": "Store", "text": "insert row" },
        ]));

        assert!(ShowSequence.execute(input.clone(), &ctx).await.is_ok());
        assert!(ShowSequence.view("call-1", &input).is_some());
    }

    fn flow(edges: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "title": "How a request reaches the database",
            "stages": [
                { "name": "Edge", "nodes": [{ "label": "Client" }] },
                { "name": "Service", "nodes": [{ "label": "API", "note": "axum" }, { "label": "Worker" }] },
                { "name": "Storage", "nodes": [{ "label": "Postgres" }] },
            ],
            "edges": edges,
        })
    }

    #[tokio::test]
    async fn a_flow_call_carries_the_stages_the_model_declared() {
        // The layering is the payload, not something recovered from the edges.
        let (ctx, _dir) = test_ctx();
        let input = flow(serde_json::json!([
            { "from": "Client", "to": "API", "label": "POST /orders" },
            { "from": "API", "to": "Postgres" },
        ]));

        let view = ShowFlow.view("call-1", &input).unwrap();
        let result = ShowFlow.execute(input, &ctx).await.unwrap();

        assert!(matches!(
            view,
            TranscriptView::Flow { ref stages, ref edges, .. }
                if stages.len() == 3
                    && stages[1].nodes.len() == 2
                    && stages[1].name.as_deref() == Some("Service")
                    && stages[1].nodes[0].note.as_deref() == Some("axum")
                    && edges[1].label.is_none()
        ));
        assert!(
            result.to_text().contains("4 nodes across 3 stages"),
            "{result}"
        );
    }

    #[tokio::test]
    async fn an_edge_naming_a_node_that_does_not_exist_is_refused() {
        let (ctx, _dir) = test_ctx();
        let stray = flow(serde_json::json!([
            { "from": "Client", "to": "Postgress" },
        ]));

        let error = ShowFlow.execute(stray.clone(), &ctx).await.unwrap_err();

        assert!(error.to_string().contains("Postgress"), "{error}");
        // The real names are listed, so the misspelling sits beside what it
        // should have been.
        assert!(
            error.to_string().contains("Client, API, Worker, Postgres"),
            "{error}"
        );
        assert!(ShowFlow.view("call-1", &stray).is_none());
    }

    #[tokio::test]
    async fn two_nodes_with_the_same_label_are_refused() {
        // Labels are the identity here, so a duplicate would send every arrow
        // to whichever box happened to come first — silently.
        let (ctx, _dir) = test_ctx();
        let error = ShowFlow
            .execute(
                serde_json::json!({
                    "title": "Caches",
                    "stages": [
                        { "nodes": [{ "label": "API" }] },
                        { "nodes": [{ "label": "Cache" }, { "label": "Cache" }] },
                    ],
                    "edges": [{ "from": "API", "to": "Cache" }],
                }),
                &ctx,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("'Cache'"), "{error}");
        assert!(error.to_string().contains("Cache (read)"), "{error}");
    }

    #[tokio::test]
    async fn an_edge_from_a_node_to_itself_is_refused() {
        let (ctx, _dir) = test_ctx();
        let error = ShowFlow
            .execute(
                flow(serde_json::json!([{ "from": "API", "to": "API" }])),
                &ctx,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("to itself"), "{error}");
        assert!(error.to_string().contains("note"), "{error}");
    }

    #[tokio::test]
    async fn an_edge_pointing_back_to_an_earlier_stage_is_allowed() {
        // A retry, a callback, a failure path. Refusing it would rule out half
        // the workflows worth drawing.
        let (ctx, _dir) = test_ctx();
        let input = flow(serde_json::json!([
            { "from": "Client", "to": "API" },
            { "from": "Worker", "to": "API", "label": "retry" },
        ]));

        assert!(ShowFlow.execute(input.clone(), &ctx).await.is_ok());
        assert!(ShowFlow.view("call-1", &input).is_some());
    }

    #[tokio::test]
    async fn an_empty_stage_is_refused_as_a_gap_in_the_picture() {
        let (ctx, _dir) = test_ctx();
        let error = ShowFlow
            .execute(
                serde_json::json!({
                    "title": "Gappy",
                    "stages": [
                        { "nodes": [{ "label": "Client" }] },
                        { "nodes": [] },
                        { "nodes": [{ "label": "API" }] },
                    ],
                    "edges": [{ "from": "Client", "to": "API" }],
                }),
                &ctx,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("stage 2"), "{error}");
    }

    fn questions() -> serde_json::Value {
        serde_json::json!({
            "questions": [{
                "prompt": "Where should the rename land first?",
                "options": [
                    { "label": "Settings panel only", "note": "2 files" },
                    { "label": "Every call site at once", "note": "11 files" },
                ],
            }],
        })
    }

    #[tokio::test]
    async fn the_question_card_is_keyed_to_the_call_that_is_waiting() {
        // If these two ever disagree, an answered card leaves the turn hanging.
        let tool = AskUser::new(Arc::new(Unattended));
        let view = tool.view("call-7", &questions()).unwrap();

        assert!(matches!(view, TranscriptView::Questions { id, .. } if id == "call-7"));
    }

    #[tokio::test]
    async fn nobody_to_ask_tells_the_model_to_decide_rather_than_failing() {
        let (ctx, _dir) = test_ctx();
        let tool = AskUser::new(Arc::new(Unattended));

        let result = tool.execute(questions(), &ctx).await.unwrap();

        assert!(
            result.to_text().contains("Decide each of these yourself"),
            "{result}"
        );
    }

    /// The case that made the branch order matter rather than merely be
    /// untidy: `Unattended` answers immediately, so an unbiased `select!` was a
    /// coin flip between stopping and telling the model to decide for itself —
    /// on every unattended run, not on some machines.
    #[tokio::test]
    async fn a_cancelled_turn_stops_rather_than_answering_for_itself() {
        let (ctx, _dir) = test_ctx();
        let tool = AskUser::new(Arc::new(Unattended));
        ctx.cancel.cancel();

        let error = tool.execute(questions(), &ctx).await.unwrap_err();

        assert!(matches!(error, ToolError::Canceled), "{error}");
    }

    #[tokio::test]
    async fn a_question_with_one_option_is_refused() {
        let (ctx, _dir) = test_ctx();
        let tool = AskUser::new(Arc::new(Unattended));

        let error = tool
            .execute(
                serde_json::json!({
                    "questions": [{
                        "prompt": "Shall I continue?",
                        "options": [{ "label": "Yes" }],
                    }],
                }),
                &ctx,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("Shall I continue?"), "{error}");
    }

    #[test]
    fn a_skipped_question_comes_back_as_an_instruction() {
        let questions = vec![
            Question {
                prompt: "Where first?".into(),
                kind: QuestionKind::Single,
                options: vec![QuestionOption {
                    label: "Settings".into(),
                    note: String::new(),
                }],
                allow_other: false,
            },
            Question {
                prompt: "Update what alongside?".into(),
                kind: QuestionKind::Multi,
                options: Vec::new(),
                allow_other: false,
            },
        ];
        let answers = vec![
            Answer {
                picked: vec!["Settings".into()],
                other: None,
            },
            Answer::default(),
        ];

        let rendered = render_answers(&questions, &answers);

        assert!(
            rendered.contains("1. Where first? — Settings"),
            "{rendered}"
        );
        assert!(rendered.contains("decide this one yourself"), "{rendered}");
        assert!(rendered.contains("Get on with the work"), "{rendered}");
    }

    #[test]
    fn an_answer_the_frontend_never_sent_reads_as_skipped() {
        // A short answer list is a frontend bug, but it must not be a panic:
        // the turn is blocked on this call and an index out of range would take
        // the whole session down with it.
        let questions = vec![Question {
            prompt: "Where first?".into(),
            kind: QuestionKind::Single,
            options: Vec::new(),
            allow_other: false,
        }];

        let rendered = render_answers(&questions, &[]);

        assert!(rendered.contains("decide this one yourself"), "{rendered}");
    }
}
