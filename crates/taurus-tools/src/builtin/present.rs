//! The three tools that address the person watching rather than the machine.
//!
//! Everything else here changes files, runs programs, or reads something back.
//! These produce nothing but a view: a table, a chart, a question. That makes
//! them the only tools whose *output* is not the point — what matters is what
//! [`crate::view::TranscriptView`] the call carries, and the string handed back
//! to the model is only there so it knows the drawing happened.
//!
//! Their input schemas are their view payloads, unchanged. See
//! [`crate::view`] for why that identity is worth preserving.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};
use crate::view::{Answer, Asker, Column, Question, Series, TranscriptView};

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

/// Questions one `ask_user` call may put.
///
/// Deliberately small. A card with eight questions on it is a form, and a form
/// is what the user opened an agent to avoid filling in.
const MAX_QUESTIONS: usize = 4;

/// Named as constants because these three are registered per turn rather than
/// into the shared registry, so `taurus-host` has to be able to name them
/// without a literal in two files that can drift apart.
pub const SHOW_TABLE_TOOL: &str = "show_table";
pub const SHOW_CHART_TOOL: &str = "show_chart";
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
        ))
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
        ))
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
        let answers = tokio::select! {
            answers = self.asker.ask(id, &input.questions) => answers,
            () = ctx.cancel.cancelled() => return Err(ToolError::Canceled),
        };

        Ok(match answers {
            Some(answers) => render_answers(&input.questions, &answers),
            // Not an error. A piped run, a git hook, or CI has nobody to ask,
            // and a turn that fails there because it wanted an opinion is worse
            // than one that decides and says so.
            None => "Nobody is available to answer. Decide each of these yourself, and say which \
                     way you went and why in your reply."
                .to_string(),
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
    use crate::view::{ColumnKind, QuestionKind, QuestionOption, Unattended};

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
        assert!(result.contains("do not repeat the rows"), "{result}");
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

        assert!(result.contains("Decide each of these yourself"), "{result}");
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
