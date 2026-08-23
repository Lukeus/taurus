//! The two tools the model calls: `load_dataset` and `profile_dataset`.
//!
//! Both are [`Effect::Read`], and both mean it. Loading writes one line into
//! the harness's own dataset list and touches nothing in the workspace — the
//! same distinction [`taurus_host::memory`]'s `remember` draws, and for the
//! same reason: a permission dialog in front of *looking at a CSV* would put a
//! decision where there is nothing to decide.
//!
//! # What they hand back, and what they do not
//!
//! Never rows. A tool result is context the model pays for on every subsequent
//! request of the turn, and a page of a dataset is the most expensive and least
//! useful thing that could go there — it is a sample the model will
//! over-generalize from, priced like a document. So these answer with *shape*:
//! how many rows, which columns, what is null, what the common values are. The
//! rows live in the pane, where looking at them costs nothing and the person
//! doing the looking can page.
//!
//! That is also why the column listing has a ceiling and the pane does not. See
//! [`MAX_LISTED_COLUMNS`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::view::TranscriptView;
use taurus_tools::{Effect, Tool, ToolContext, ToolError, ToolResult};

use crate::catalog::{self, Dataset};
use crate::df::TOP_VALUES_MAX_DISTINCT;
use crate::engine::{ColumnProfile, DataError, Distinct, Engine, Format, Profile, Schema, Source};

pub const LOAD_DATASET_TOOL: &str = "load_dataset";
pub const PROFILE_DATASET_TOOL: &str = "profile_dataset";

/// Columns one tool result may name.
///
/// A reading and a context limit at once, and the reason it can be this low is
/// that nothing is lost by it: the pane lists every column of every dataset,
/// with no cap, and the message says so. Sixty is where a listing stops being
/// something a person scans and starts being something they scroll — the same
/// number, for the same reason, as `present.rs`'s cap on table rows.
///
/// A wide feature matrix is the case this exists for. Five thousand column
/// names is not a schema summary, it is the schema, and pasting it into an 8k
/// context leaves no room for the question that was being asked about it.
pub const MAX_LISTED_COLUMNS: usize = 60;

impl From<DataError> for ToolError {
    /// Which failures the model can fix by calling again, and which it cannot.
    ///
    /// The split matters more than it looks: `InvalidInput` is rendered back
    /// with "check the tool's schema and retry", which is the right advice for
    /// a mistyped name and exactly the wrong advice for an unreadable file.
    fn from(error: DataError) -> Self {
        match error {
            DataError::UnknownFormat { .. }
            | DataError::NoSuchDataset { .. }
            | DataError::NotAFile(_)
            | DataError::BadName { .. }
            | DataError::NameTaken { .. } => ToolError::InvalidInput(error.to_string()),
            other => ToolError::Failed(other.to_string()),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct LoadDatasetInput {
    /// Path to the data file, relative to the workspace root — for example
    /// `data/events.csv`.
    pub path: String,
    /// What to call it afterwards. Defaults to the filename, lowercased.
    ///
    /// Pass one when two files share a filename, or when the filename is not
    /// what you would call the thing — `interactions` reads better than
    /// `export_2024_final`.
    #[serde(default)]
    pub name: Option<String>,
}

/// Registers a file as a dataset, and reads its columns.
pub struct LoadDataset {
    engine: Arc<dyn Engine>,
    /// This workspace's dataset list. Rebuilt with the tool when the workspace
    /// changes, so a call never has to ask which folder it is in.
    dir: PathBuf,
}

impl LoadDataset {
    pub fn new(engine: Arc<dyn Engine>, dir: impl Into<PathBuf>) -> Self {
        Self {
            engine,
            dir: dir.into(),
        }
    }
}

#[async_trait]
impl Tool for LoadDataset {
    fn name(&self) -> &str {
        LOAD_DATASET_TOOL
    }

    fn description(&self) -> &str {
        "Register a data file so it can be examined as a table. Takes a path to a .csv, .tsv, \
         .parquet, or newline-delimited .ndjson/.jsonl file in the workspace, reads its columns, \
         and gives it a short name the other data tools take. \
         Reach for this the moment a question is about the *contents* of a data file rather than \
         its text: opening a million-row CSV with read_file is the mistake this exists to \
         prevent, and it costs a whole context window to make. Loading is cheap — it reads the \
         header and the file's own metadata, never the rows."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<LoadDatasetInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("?");
        format!("Load {path} as a dataset")
    }

    fn view(&self, _id: &str, input: &serde_json::Value) -> Option<TranscriptView> {
        let input: LoadDatasetInput = serde_json::from_value(input.clone()).ok()?;
        // The same derivation `execute` runs, and it has to be — see
        // `catalog::suggest_name`, which is pure for exactly this reason. A
        // name that disagreed here would draw a card pointing at nothing.
        Some(TranscriptView::Dataset {
            name: chosen_name(&input).ok()?,
        })
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: LoadDatasetInput = parse_input(input)?;
        let name = chosen_name(&input)?;

        let path = ctx.resolve_read(&input.path)?;
        if !path.is_file() {
            return Err(DataError::NotAFile(ctx.display(&path)).into());
        }
        let source = Source::at(&path)?;
        let shown = ctx.display(&path);

        // Before the read rather than after it, so a name clash costs nothing
        // and the message arrives while the model is still deciding what to
        // call things.
        if let Some(existing) = catalog::taken_by(&self.dir, &name, &shown) {
            return Err(DataError::NameTaken {
                name,
                existing: existing.path,
                path: shown,
            }
            .into());
        }

        let schema = self.engine.schema(&source).await?;
        catalog::register(
            &self.dir,
            Dataset {
                name: name.clone(),
                path: shown.clone(),
                format: source.format,
            },
        )?;

        Ok(describe_schema(&name, &shown, source.format, &schema).into())
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ProfileDatasetInput {
    /// The dataset's name, as `load_dataset` reported it.
    pub name: String,
}

/// Reads a whole dataset and reports its shape.
pub struct ProfileDataset {
    engine: Arc<dyn Engine>,
    dir: PathBuf,
}

impl ProfileDataset {
    pub fn new(engine: Arc<dyn Engine>, dir: impl Into<PathBuf>) -> Self {
        Self {
            engine,
            dir: dir.into(),
        }
    }
}

#[async_trait]
impl Tool for ProfileDataset {
    fn name(&self) -> &str {
        PROFILE_DATASET_TOOL
    }

    fn description(&self) -> &str {
        "Describe a loaded dataset column by column: how many rows it has, what is missing, how \
         many different values each column holds, the range of the ordered ones, and the \
         commonest values of the categorical ones. \
         This is the first thing to do with data you have not seen. It answers the questions that \
         decide everything after them — which column is the key, which is a label, what needs \
         cleaning — and it answers them from the whole file rather than from a sample. \
         It reads every row, so it is slower than load_dataset and worth calling once rather \
         than between every step."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ProfileDatasetInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let name = input.get("name").and_then(|n| n.as_str()).unwrap_or("?");
        format!("Profile the {name} dataset")
    }

    fn view(&self, _id: &str, input: &serde_json::Value) -> Option<TranscriptView> {
        let input: ProfileDatasetInput = serde_json::from_value(input.clone()).ok()?;
        Some(TranscriptView::Dataset { name: input.name })
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: ProfileDatasetInput = parse_input(input)?;
        let dataset = catalog::find(&self.dir, &input.name)?;
        let source = Source::at(ctx.resolve_read(&dataset.path)?)?;

        ctx.report(format!("reading {}", dataset.path)).await;
        let profile = self.engine.profile(&source).await?;

        Ok(describe_profile(&dataset, &profile).into())
    }
}

/// The name a call will produce, from its input alone.
fn chosen_name(input: &LoadDatasetInput) -> Result<String, DataError> {
    match input.name.as_deref() {
        Some(raw) => catalog::normalize_name(raw),
        None => Ok(catalog::suggest_name(Path::new(&input.path))),
    }
}

/// What `load_dataset` says back.
fn describe_schema(name: &str, path: &str, format: Format, schema: &Schema) -> String {
    let size = bytes(schema.bytes);
    let shape = match schema.rows {
        Some(rows) => format!(
            "{} rows × {} columns",
            thousands(rows),
            schema.columns.len()
        ),
        // Not "unknown rows". The reason is specific and actionable, and a
        // model told only that the number is missing will go and count them
        // with a shell command.
        None => format!(
            "{} columns (a {} keeps no row count; profile_dataset counts them)",
            schema.columns.len(),
            format.label()
        ),
    };

    let mut out = format!("Loaded `{name}` from {path} — {shape}, {size}.\n\n");
    let width = schema
        .columns
        .iter()
        .take(MAX_LISTED_COLUMNS)
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(0);
    for column in schema.columns.iter().take(MAX_LISTED_COLUMNS) {
        out.push_str(&format!(
            "  {:width$}  {}\n",
            column.name,
            column.type_name,
            width = width
        ));
    }
    if let Some(rest) = schema
        .columns
        .len()
        .checked_sub(MAX_LISTED_COLUMNS)
        .filter(|n| *n > 0)
    {
        out.push_str(&format!(
            "  … and {} more. The Data pane lists every column.\n",
            thousands(rest as u64)
        ));
    }
    out
}

/// What `profile_dataset` says back.
fn describe_profile(dataset: &Dataset, profile: &Profile) -> String {
    let mut out = format!(
        "`{}` — {} rows × {} columns, from {}, profiled by {}.\n\n",
        dataset.name,
        thousands(profile.rows),
        profile.columns.len(),
        dataset.path,
        profile.engine,
    );

    let shown: Vec<&ColumnProfile> = profile.columns.iter().take(MAX_LISTED_COLUMNS).collect();
    let width = shown
        .iter()
        .map(|c| c.head.name.chars().count())
        .max()
        .unwrap_or(0);
    for column in &shown {
        out.push_str(&format!(
            "  {:width$}  {}\n",
            column.head.name,
            column_line(column, profile.rows),
            width = width
        ));
    }
    if let Some(rest) = profile
        .columns
        .len()
        .checked_sub(MAX_LISTED_COLUMNS)
        .filter(|n| *n > 0)
    {
        out.push_str(&format!(
            "  … and {} more. The Data pane profiles every column.\n",
            thousands(rest as u64)
        ));
    }
    out
}

/// One column, on one line.
fn column_line(column: &ColumnProfile, rows: u64) -> String {
    let mut parts = vec![column.head.type_name.clone()];

    parts.push(match column.distinct {
        Distinct::Exact { count } => format!("{} distinct", thousands(count)),
        Distinct::Unavailable => "nested".to_string(),
    });

    parts.push(if column.nulls == 0 {
        "no nulls".to_string()
    } else {
        format!(
            "{} nulls ({})",
            thousands(column.nulls),
            percent(column.nulls, rows)
        )
    });

    if let (Some(min), Some(max)) = (&column.min, &column.max) {
        parts.push(format!("{min} … {max}"));
    }

    if !column.common.is_empty() {
        let common: Vec<String> = column
            .common
            .iter()
            .map(|v| {
                let label = v.value.as_deref().unwrap_or("(null)");
                format!("{} {}", truncate(label), percent(v.count, rows))
            })
            .collect();
        parts.push(common.join(", "));
    } else if let Distinct::Exact { count } = column.distinct {
        // The one stated cap in this output. Without the reason, an empty
        // list reads as "no common values", which is the opposite of true.
        if count > TOP_VALUES_MAX_DISTINCT {
            parts.push("too many values to top".to_string());
        }
    }

    parts.join(" · ")
}

/// Keeps one common value from taking the whole line.
///
/// Free text lands in a categorical column more often than it should, and one
/// 400-character value would push the four beside it off the end.
fn truncate(value: &str) -> String {
    const MAX: usize = 24;
    if value.chars().count() <= MAX {
        return value.to_string();
    }
    format!("{}…", value.chars().take(MAX).collect::<String>())
}

/// A share of the whole, at the precision that share deserves.
///
/// Whole numbers above one percent, one decimal below it. A flat `0%` beside a
/// non-zero null count is the specific thing this avoids: "1,204 nulls (0%)"
/// reads as a rounding artefact and is the exact case somebody is scanning for.
fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "0%".into();
    }
    let share = (part as f64 / whole as f64) * 100.0;
    if share >= 1.0 || share == 0.0 {
        format!("{}%", share.round() as u64)
    } else {
        format!("{share:.1}%")
    }
}

/// Digits grouped, so a seven-figure row count can be read at a glance.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A file size as a person reads it.
fn bytes(n: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    const KB: u64 = 1024;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{} KB", n / KB)
    } else {
        format!("{n} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{ColumnHead, ColumnKind, ValueCount};

    fn head(name: &str, kind: ColumnKind, type_name: &str) -> ColumnHead {
        ColumnHead {
            name: name.into(),
            kind,
            type_name: type_name.into(),
            nullable: true,
        }
    }

    #[test]
    fn digits_are_grouped_from_the_right() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_240_913), "1,240,913");
    }

    /// The case the extra decimal exists for: a real null count that would
    /// otherwise render as the same `0%` a clean column shows.
    #[test]
    fn a_small_but_real_share_does_not_round_to_zero() {
        assert_eq!(percent(1_204, 1_240_913), "0.1%");
        assert_eq!(percent(0, 1_000), "0%");
        assert_eq!(percent(410, 1_000), "41%");
    }

    #[test]
    fn a_name_is_derived_from_the_path_when_none_is_given() {
        let input = LoadDatasetInput {
            path: "data/User Events.csv".into(),
            name: None,
        };
        assert_eq!(chosen_name(&input).unwrap(), "user_events");
    }

    /// The property the transcript card rests on: `view` and `execute` derive
    /// the same name from the same input, having consulted nothing else.
    #[test]
    fn the_card_names_what_the_call_will_produce() {
        let tool = LoadDataset::new(Arc::new(crate::df::DataFusionEngine::new()), "/tmp/nowhere");
        let input = serde_json::json!({ "path": "a/b/Events.csv" });
        let view = tool.view("call-1", &input).unwrap();
        let TranscriptView::Dataset { name } = view else {
            panic!("a load should draw a dataset card");
        };
        assert_eq!(name, "events");
        assert_eq!(
            name,
            chosen_name(&serde_json::from_value(input).unwrap()).unwrap()
        );
    }

    #[test]
    fn an_explicit_name_wins_and_is_normalized() {
        let input = LoadDatasetInput {
            path: "data/export_2024_final.parquet".into(),
            name: Some("Interactions".into()),
        };
        assert_eq!(chosen_name(&input).unwrap(), "interactions");
    }

    #[test]
    fn a_schema_with_no_row_count_says_why_rather_than_leaving_a_gap() {
        let schema = Schema {
            columns: vec![head("id", ColumnKind::Text, "Utf8")],
            rows: None,
            bytes: 2048,
        };
        let out = describe_schema("events", "data/events.csv", Format::Csv, &schema);
        assert!(out.contains("profile_dataset counts them"), "{out}");
        assert!(out.contains("2 KB"), "{out}");
    }

    #[test]
    fn a_wide_schema_is_capped_and_says_where_the_rest_is() {
        let columns: Vec<ColumnHead> = (0..MAX_LISTED_COLUMNS + 5)
            .map(|i| head(&format!("f{i}"), ColumnKind::Number, "Float64"))
            .collect();
        let schema = Schema {
            columns,
            rows: Some(10),
            bytes: 100,
        };
        let out = describe_schema("features", "f.parquet", Format::Parquet, &schema);
        assert!(out.contains("… and 5 more"), "{out}");
        assert!(out.contains("Data pane"), "{out}");
        assert!(!out.contains("f61"), "the cap did not hold: {out}");
    }

    #[test]
    fn a_profile_line_reads_as_a_sentence_about_the_column() {
        let column = ColumnProfile {
            head: head("event", ColumnKind::Text, "Utf8"),
            nulls: 0,
            distinct: Distinct::Exact { count: 4 },
            min: Some("click".into()),
            max: Some("view".into()),
            common: vec![
                ValueCount {
                    value: Some("view".into()),
                    count: 410,
                },
                ValueCount {
                    value: None,
                    count: 90,
                },
            ],
        };
        let line = column_line(&column, 1_000);
        assert!(line.contains("Utf8"), "{line}");
        assert!(line.contains("4 distinct"), "{line}");
        assert!(line.contains("no nulls"), "{line}");
        assert!(line.contains("click … view"), "{line}");
        assert!(line.contains("view 41%"), "{line}");
        assert!(line.contains("(null) 9%"), "{line}");
    }

    /// An empty `common` has two very different causes, and only one of them
    /// means "there is nothing to say".
    #[test]
    fn a_column_with_too_many_values_says_so_rather_than_going_quiet() {
        let column = ColumnProfile {
            head: head("user_id", ColumnKind::Text, "Utf8"),
            nulls: 0,
            distinct: Distinct::Exact {
                count: TOP_VALUES_MAX_DISTINCT + 1,
            },
            min: None,
            max: None,
            common: Vec::new(),
        };
        assert!(column_line(&column, 10_000).contains("too many values to top"));
    }

    #[test]
    fn a_nested_column_reports_what_it_can_and_claims_nothing_else() {
        let column = ColumnProfile {
            head: head("payload", ColumnKind::Nested, "Struct"),
            nulls: 12,
            distinct: Distinct::Unavailable,
            min: None,
            max: None,
            common: Vec::new(),
        };
        let line = column_line(&column, 100);
        assert!(line.contains("nested"), "{line}");
        assert!(line.contains("12 nulls (12%)"), "{line}");
        assert!(!line.contains("distinct"), "{line}");
    }

    #[test]
    fn a_long_common_value_is_cut_rather_than_taking_the_line() {
        let column = ColumnProfile {
            head: head("note", ColumnKind::Text, "Utf8"),
            nulls: 0,
            distinct: Distinct::Exact { count: 2 },
            min: None,
            max: None,
            common: vec![ValueCount {
                value: Some("x".repeat(200)),
                count: 5,
            }],
        };
        let line = column_line(&column, 10);
        assert!(line.contains('…'), "{line}");
        assert!(line.len() < 120, "{line}");
    }

    #[test]
    fn a_profile_names_the_engine_that_produced_it() {
        let dataset = Dataset {
            name: "events".into(),
            path: "data/events.csv".into(),
            format: Format::Csv,
        };
        let profile = Profile {
            rows: 1_240_913,
            columns: vec![ColumnProfile {
                head: head("id", ColumnKind::Text, "Utf8"),
                nulls: 0,
                distinct: Distinct::Exact { count: 12 },
                min: None,
                max: None,
                common: Vec::new(),
            }],
            engine: "DataFusion".into(),
        };
        let out = describe_profile(&dataset, &profile);
        assert!(out.contains("1,240,913 rows"), "{out}");
        assert!(out.contains("profiled by DataFusion"), "{out}");
        assert!(out.contains("data/events.csv"), "{out}");
    }
}

/// Both tools, end to end, against real files in a real workspace.
///
/// The unit tests above cover the words each one chooses. These cover the parts
/// only a whole call has: the path guard, the catalog write, the name clash,
/// and what a wrong name gets told.
#[cfg(test)]
mod calls {
    use super::*;
    use std::sync::Arc;

    use taurus_tools::{AllowAll, PermissionEngine, ToolContext};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    const EVENTS: &str = "id,event,price\n1,view,10.5\n2,click,\n3,view,7.25\n";

    /// A workspace with `data/events.csv` in it, and a context over it.
    ///
    /// The root is canonicalized because that is what a real call receives — on
    /// macOS a temp directory lives behind `/var -> /private/var`, and skipping
    /// it would have the tool record absolute paths where it records relative
    /// ones in the app.
    fn workspace() -> (ToolContext, TempDir, TempDir) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/events.csv"), EVENTS).unwrap();

        let engine = Arc::new(PermissionEngine::new(
            &root,
            root.join(".taurus"),
            Box::new(AllowAll),
        ));
        let home = TempDir::new().unwrap();
        (
            ToolContext::new(root, engine, CancellationToken::new()),
            dir,
            home,
        )
    }

    fn tools(home: &TempDir) -> (LoadDataset, ProfileDataset) {
        let engine = Arc::new(crate::df::DataFusionEngine::new());
        (
            LoadDataset::new(engine.clone(), home.path()),
            ProfileDataset::new(engine, home.path()),
        )
    }

    #[tokio::test]
    async fn loading_records_a_relative_path_and_reports_the_columns() {
        let (ctx, _dir, home) = workspace();
        let (load, _) = tools(&home);

        let out = load
            .execute(serde_json::json!({ "path": "data/events.csv" }), &ctx)
            .await
            .unwrap();
        let text = out.to_text();
        assert!(text.contains("`events`"), "{text}");
        assert!(text.contains("data/events.csv"), "{text}");
        assert!(text.contains("3 columns"), "{text}");
        // A CSV keeps no row count, and the message says what will answer that
        // rather than leaving a gap the model fills with a shell command.
        assert!(text.contains("profile_dataset counts them"), "{text}");

        let listed = catalog::load(home.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "events");
        // Relative, so the entry survives the folder being moved — and so
        // nothing in the list is a path out of the workspace.
        assert_eq!(listed[0].path, "data/events.csv");
    }

    #[tokio::test]
    async fn profiling_reads_the_file_the_entry_points_at() {
        let (ctx, _dir, home) = workspace();
        let (load, profile) = tools(&home);
        load.execute(serde_json::json!({ "path": "data/events.csv" }), &ctx)
            .await
            .unwrap();

        let out = profile
            .execute(serde_json::json!({ "name": "events" }), &ctx)
            .await
            .unwrap();
        let text = out.to_text();
        assert!(text.contains("3 rows"), "{text}");
        assert!(text.contains("1 nulls"), "the empty price: {text}");
        assert!(text.contains("view 67%"), "{text}");
    }

    /// A file edited between two loads should re-read, not be refused.
    #[tokio::test]
    async fn loading_the_same_file_again_replaces_its_entry() {
        let (ctx, dir, home) = workspace();
        let (load, _) = tools(&home);
        let call = serde_json::json!({ "path": "data/events.csv" });
        load.execute(call.clone(), &ctx).await.unwrap();

        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("data/events.csv"), "id,event\n1,view\n").unwrap();
        let out = load.execute(call, &ctx).await.unwrap();

        assert!(out.to_text().contains("2 columns"), "it re-read the file");
        assert_eq!(catalog::load(home.path()).len(), 1);
    }

    /// `train/data.csv` and `eval/data.csv` both want `data`. Refused rather
    /// than silently suffixed, because being asked produces `train` and `eval`
    /// and a suffix produces `data_2`.
    #[tokio::test]
    async fn two_files_wanting_one_name_are_refused_with_a_way_out() {
        let (ctx, dir, home) = workspace();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("eval")).unwrap();
        std::fs::write(root.join("eval/events.csv"), EVENTS).unwrap();

        let (load, _) = tools(&home);
        load.execute(serde_json::json!({ "path": "data/events.csv" }), &ctx)
            .await
            .unwrap();

        let error = load
            .execute(serde_json::json!({ "path": "eval/events.csv" }), &ctx)
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(matches!(error, ToolError::InvalidInput(_)), "{message}");
        assert!(message.contains("data/events.csv"), "{message}");
        assert!(message.contains("Pass a `name`"), "{message}");

        // And the way out works.
        let out = load
            .execute(
                serde_json::json!({ "path": "eval/events.csv", "name": "eval" }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("`eval`"));
        assert_eq!(catalog::load(home.path()).len(), 2);
    }

    #[tokio::test]
    async fn a_file_outside_the_workspace_is_refused_by_the_path_guard() {
        let (ctx, _dir, home) = workspace();
        let (load, _) = tools(&home);
        let error = load
            .execute(serde_json::json!({ "path": "../../etc/passwd" }), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ToolError::OutsideWorkspace { .. }),
            "{error}"
        );
        assert!(catalog::load(home.path()).is_empty());
    }

    #[tokio::test]
    async fn a_directory_is_refused_rather_than_read() {
        let (ctx, _dir, home) = workspace();
        let (load, _) = tools(&home);
        let error = load
            .execute(serde_json::json!({ "path": "data" }), &ctx)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Point this at one file"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn an_unreadable_extension_says_what_is_readable() {
        let (ctx, dir, home) = workspace();
        std::fs::write(dir.path().join("book.xlsx"), "not a spreadsheet").unwrap();
        let (load, _) = tools(&home);
        let error = load
            .execute(serde_json::json!({ "path": "book.xlsx" }), &ctx)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(".parquet"), "{error}");
    }

    /// A model that guesses a name should be one line from the right one.
    #[tokio::test]
    async fn profiling_an_unknown_name_lists_what_is_loaded() {
        let (ctx, _dir, home) = workspace();
        let (load, profile) = tools(&home);
        load.execute(serde_json::json!({ "path": "data/events.csv" }), &ctx)
            .await
            .unwrap();

        let error = profile
            .execute(serde_json::json!({ "name": "event" }), &ctx)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Loaded here: events"), "{error}");
    }

    /// Neither tool touches the workspace, and the permission layer has to
    /// agree — an `Effect::Write` here would put a dialog in front of looking
    /// at a CSV, and a diff of it in the Changes drawer afterwards.
    #[tokio::test]
    async fn neither_tool_asks_permission_to_read_a_file() {
        let (_ctx, _dir, home) = workspace();
        let (load, profile) = tools(&home);
        assert_eq!(load.effect(), Effect::Read);
        assert_eq!(profile.effect(), Effect::Read);
    }

    #[tokio::test]
    async fn forgetting_a_dataset_leaves_the_file_alone() {
        let (ctx, dir, home) = workspace();
        let (load, _) = tools(&home);
        load.execute(serde_json::json!({ "path": "data/events.csv" }), &ctx)
            .await
            .unwrap();

        assert!(catalog::forget(home.path(), "events").unwrap());
        assert!(catalog::load(home.path()).is_empty());
        let root = dir.path().canonicalize().unwrap();
        assert!(
            root.join("data/events.csv").exists(),
            "forgetting a pointer must not delete what it pointed at"
        );
    }
}
