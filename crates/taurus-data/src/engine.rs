//! What a data engine has to do, and the vocabulary it answers in.
//!
//! Three methods. That is deliberate and it is the whole point of this file:
//! the engine underneath is a decision made once, on evidence that does not
//! exist yet, and nothing outside this crate should be able to tell which one
//! was picked. A tool, a command, and a pane all speak [`Schema`], [`Profile`],
//! and [`Page`] — none of them names a `SessionContext`, a `RecordBatch`, or a
//! dialect of SQL.
//!
//! The narrowness is load-bearing rather than tidy. Phase 1 reads and
//! describes; the phase after it transforms, and a transformation is where an
//! engine's identity actually leaks — its SQL dialect becomes the language the
//! recipe is written in, and the recipe is a file on disk that outlives the
//! choice. Keeping the read half behind a trait is what buys the right to make
//! that call later against a real dataset instead of now against an argument.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A file an engine can read, and what it is.
///
/// The format is resolved once, here, rather than sniffed by each engine —
/// two engines disagreeing about what a `.json` file is would be a bug that
/// only appeared when the engine changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Absolute, already resolved against the workspace by the caller.
    pub path: PathBuf,
    pub format: Format,
}

impl Source {
    /// Reads a path's extension, or explains what is readable.
    pub fn at(path: impl Into<PathBuf>) -> Result<Self, DataError> {
        let path = path.into();
        let format = Format::of(&path)?;
        Ok(Self { path, format })
    }
}

/// The file layouts this understands.
///
/// A short list on purpose. Every entry here is one an engine reads natively
/// and a test covers; a format that needed a conversion step first would be a
/// pipeline stage wearing a loader's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "DataFormat")]
pub enum Format {
    Csv,
    /// Tab-separated. Its own variant rather than a delimiter field on `Csv`,
    /// because the delimiter is the only thing that ever differs and a field
    /// would invite the rest of a dialect to follow it in.
    Tsv,
    Parquet,
    /// One JSON object per line. A `.json` file holding a single array is
    /// **not** this, and says so when it fails to read — see [`DataError`].
    Ndjson,
}

impl Format {
    /// What the extension says this file is.
    pub fn of(path: &Path) -> Result<Self, DataError> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match extension.as_str() {
            "csv" => Ok(Self::Csv),
            "tsv" | "tab" => Ok(Self::Tsv),
            "parquet" | "pq" => Ok(Self::Parquet),
            "ndjson" | "jsonl" | "json" => Ok(Self::Ndjson),
            _ => Err(DataError::UnknownFormat {
                path: path.display().to_string(),
            }),
        }
    }

    /// The name a message uses for this format.
    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Tsv => "TSV",
            Self::Parquet => "Parquet",
            Self::Ndjson => "newline-delimited JSON",
        }
    }
}

/// What a column holds, reduced to what changes how it is read.
///
/// Not the engine's type — [`ColumnHead::type_name`] carries that, unshortened.
/// This is the question the UI actually asks: does it right-align, do minimum
/// and maximum mean anything, is a distinct count worth showing. Three
/// behaviours is all those questions have between them, which is why widening
/// this enum should feel like it needs an argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "DataColumnKind")]
pub enum ColumnKind {
    Text,
    Number,
    Boolean,
    /// A date, a time, or an instant. Ordered like a number and read like text.
    Temporal,
    /// A list, a struct, a map — anything whose cells are not one value.
    ///
    /// Kept rather than refused: a nested column is extremely common in
    /// exported JSON, and a loader that rejected the file would be refusing
    /// the other thirteen columns over one. It profiles as present-or-null and
    /// nothing more, which is honest about what can be said without flattening
    /// it first.
    Nested,
}

impl ColumnKind {
    /// Whether minimum and maximum mean anything for this column.
    pub fn is_ordered(self) -> bool {
        matches!(self, Self::Number | Self::Temporal | Self::Text)
    }
}

/// One column, before anything has been counted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "DataColumn")]
pub struct ColumnHead {
    pub name: String,
    pub kind: ColumnKind,
    /// The engine's own name for the type — `Int64`, `Utf8`, `Timestamp(us)`.
    ///
    /// Shown beside the kind rather than instead of it. The kind is what the
    /// UI acts on; this is what a person needs when the kind is not the whole
    /// story, and the only place in the surface where the engine is allowed to
    /// speak in its own words.
    pub type_name: String,
    /// Whether the engine believes this column may hold nulls.
    ///
    /// A belief, not a count: it comes from the file's declared schema, which
    /// Parquet carries and CSV does not. [`ColumnProfile::nulls`] is the
    /// measured answer, and the two disagreeing is ordinary rather than wrong.
    pub nullable: bool,
}

/// The columns of a source, without reading all of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "DataSchema")]
pub struct Schema {
    pub columns: Vec<ColumnHead>,
    /// Rows, when the format can say so without a scan.
    ///
    /// Parquet keeps a count in its footer, so this is free there. A CSV has
    /// to be read to be counted, and guessing from the file size would be a
    /// number that is wrong in a way nobody can see — so it is `None`, and the
    /// profile is what answers it.
    /// Declared to TypeScript as `number` rather than the `bigint` a `u64`
    /// becomes by default. A double counts every integer exactly up to nine
    /// quadrillion; no file has that many rows, and `bigint` would cost every
    /// reader a conversion — and forbid mixing it with a percentage — to buy
    /// precision nothing here can reach.
    #[ts(type = "number")]
    pub rows: Option<u64>,
    /// The file, in bytes, as it sits on disk.
    /// Declared to TypeScript as `number` rather than the `bigint` a `u64`
    /// becomes by default. A double counts every integer exactly up to nine
    /// quadrillion; no file has that many rows, and `bigint` would cost every
    /// reader a conversion — and forbid mixing it with a percentage — to buy
    /// precision nothing here can reach.
    #[ts(type = "number")]
    pub bytes: u64,
}

/// How many different values a column holds.
///
/// An enum rather than an `Option<u64>` because the two answers mean different
/// things and a `None` would flatten them: "there are no distinct values to
/// count" and "this kind of column cannot be counted that way" read the same
/// as an absent number, and only one of them is worth a dash on screen.
///
/// There is deliberately no approximate variant. See the note at the top of
/// the engine implementation for why the count is exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, rename = "DataDistinct")]
pub enum Distinct {
    Exact {
        #[ts(type = "number")]
        count: u64,
    },
    /// Not a question this can answer for this column — a nested one, whose
    /// cells are not single values to compare.
    Unavailable,
}

/// One value and how often it occurs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "DataValueCount")]
pub struct ValueCount {
    /// Rendered. `None` is the null group, which is worth showing beside the
    /// real values rather than only as a percentage further up.
    pub value: Option<String>,
    /// Declared to TypeScript as `number` rather than the `bigint` a `u64`
    /// becomes by default. A double counts every integer exactly up to nine
    /// quadrillion; no file has that many rows, and `bigint` would cost every
    /// reader a conversion — and forbid mixing it with a percentage — to buy
    /// precision nothing here can reach.
    #[ts(type = "number")]
    pub count: u64,
}

/// Everything a full pass can say about one column.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "DataColumnProfile")]
pub struct ColumnProfile {
    pub head: ColumnHead,
    /// Rows where this column is null. Counted, not declared.
    /// Declared to TypeScript as `number` rather than the `bigint` a `u64`
    /// becomes by default. A double counts every integer exactly up to nine
    /// quadrillion; no file has that many rows, and `bigint` would cost every
    /// reader a conversion — and forbid mixing it with a percentage — to buy
    /// precision nothing here can reach.
    #[ts(type = "number")]
    pub nulls: u64,
    /// How many different values it holds, nulls excluded.
    pub distinct: Distinct,
    /// Smallest and largest, rendered. `None` where the kind has no order, or
    /// where every row is null.
    pub min: Option<String>,
    pub max: Option<String>,
    /// The most common values, most common first, at most [`TOP_VALUES`].
    ///
    /// Empty for a column with nothing worth topping: one that is entirely
    /// null, one whose kind cannot be grouped, or one with more distinct values
    /// than a top five says anything about — an identifier's five most common
    /// values are five arbitrary rows wearing the clothes of a finding. The
    /// last of those is a stated limit rather than a silent one: `distinct`
    /// still carries the real number, and both the pane and the summary handed
    /// to the model say that is why the list is empty.
    pub common: Vec<ValueCount>,
}

/// How many values a column's `common` list holds.
///
/// Five. Enough to see a skew or a dominant default; short enough that the
/// profile of a forty-column table is still a page. A longer list is a
/// `GROUP BY`, which is what the phase after this one is for.
pub const TOP_VALUES: usize = 5;

/// A full pass over a source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "DataProfile")]
pub struct Profile {
    /// Declared to TypeScript as `number` rather than the `bigint` a `u64`
    /// becomes by default. A double counts every integer exactly up to nine
    /// quadrillion; no file has that many rows, and `bigint` would cost every
    /// reader a conversion — and forbid mixing it with a percentage — to buy
    /// precision nothing here can reach.
    #[ts(type = "number")]
    pub rows: u64,
    pub columns: Vec<ColumnProfile>,
    /// Which engine produced these numbers.
    ///
    /// On the payload rather than assumed, because the engine is a decision
    /// still being made and a profile that does not say where it came from is
    /// a number nobody can reproduce. The pane shows it in the caption, the
    /// same way [`taurus_tools::view::TranscriptView::Table`] carries one.
    pub engine: String,
}

/// A window of rows, rendered for reading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "DataPage")]
pub struct Page {
    pub columns: Vec<ColumnHead>,
    /// Cells as strings, one inner vector per row, in column order.
    ///
    /// `None` is null, and that is the reason these are not plain strings. A
    /// null and an empty string are the same three pixels on screen and
    /// entirely different problems in a dataset — telling them apart is most
    /// of what looking at raw rows is *for*.
    pub rows: Vec<Vec<Option<String>>>,
    /// Declared to TypeScript as `number` rather than the `bigint` a `u64`
    /// becomes by default. A double counts every integer exactly up to nine
    /// quadrillion; no file has that many rows, and `bigint` would cost every
    /// reader a conversion — and forbid mixing it with a percentage — to buy
    /// precision nothing here can reach.
    #[ts(type = "number")]
    pub offset: u64,
    /// Rows in the whole source, so a grid can say where in it this is.
    /// Declared to TypeScript as `number` rather than the `bigint` a `u64`
    /// becomes by default. A double counts every integer exactly up to nine
    /// quadrillion; no file has that many rows, and `bigint` would cost every
    /// reader a conversion — and forbid mixing it with a percentage — to buy
    /// precision nothing here can reach.
    #[ts(type = "number")]
    pub total: u64,
}

/// What a query answered with.
///
/// Its own type rather than a [`Page`], because the two are different
/// questions. A page is a window into a known table and says where in it you
/// are; a query result is however many rows the query produced, and `offset`
/// and `total` have no meaning for it — a `GROUP BY` over a million rows has a
/// total of six.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "DataQueryResult")]
pub struct QueryResult {
    pub columns: Vec<ColumnHead>,
    /// Cells as strings, `None` for null — the same arrangement [`Page`] uses,
    /// for the same reason.
    pub rows: Vec<Vec<Option<String>>>,
    /// Whether [`MAX_QUERY_ROWS`] cut the answer short.
    ///
    /// Carried rather than inferred from the row count. A query that returned
    /// exactly the cap and a query that was truncated at it look identical from
    /// outside, and only one of them is the whole answer.
    pub truncated: bool,
    /// How long the engine took, in milliseconds.
    ///
    /// Measured around execution rather than around the whole call, so it is
    /// the query's cost and not the IPC hop's. Worth showing: iterating on a
    /// query over a large file is the one place in this feature where somebody
    /// is waiting on a number they can change.
    #[ts(type = "number")]
    pub took_ms: u64,
}

/// Rows one [`Engine::query`] call may return.
///
/// Smaller than [`MAX_PAGE`], and for a different reason. A page is scrolled
/// by a person and costs nothing but pixels; a query result is read by the
/// *model*, and every row is context it pays for on every later request of the
/// turn. Thirty is enough to see the shape of an answer and few enough that a
/// wrong query costs a paragraph rather than a page. A query that wants more
/// rows than this wants to be a recipe writing a file.
pub const MAX_QUERY_ROWS: u64 = 30;

/// Rows one [`Engine::page`] call may return.
///
/// A reading limit, and a smaller one than it looks: the pane pages, so this is
/// how much is in flight at once rather than how much can be seen. Every cell
/// crosses the IPC channel as a string, and a thousand rows of a forty-column
/// table is a payload measured in megabytes for a screen that shows thirty.
pub const MAX_PAGE: u64 = 500;

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error(
        "{path} is not a file Taurus can read as data. It reads .csv, .tsv, .parquet, and \
         newline-delimited .ndjson/.jsonl/.json."
    )]
    UnknownFormat { path: String },

    #[error("{path} could not be read as {format}: {detail}")]
    Unreadable {
        path: String,
        format: &'static str,
        detail: String,
    },

    #[error("no dataset named '{name}' in this workspace. {available}")]
    NoSuchDataset {
        name: String,
        /// What *is* loaded, phrased as a sentence. See `catalog::available`.
        available: String,
    },

    #[error("{0} is a directory. Point this at one file.")]
    NotAFile(String),

    #[error(
        "'{raw}' has no letters or digits in it, so it cannot name a dataset. Use something like \
         'events' or 'user_profiles'."
    )]
    BadName { raw: String },

    #[error(
        "the name '{name}' is already loaded here, from {existing}. Pass a `name` to load {path} \
         alongside it — something that says which is which, like 'train' and 'eval'."
    )]
    NameTaken {
        name: String,
        /// The path the name is already pointing at.
        existing: String,
        /// The path that wanted it.
        path: String,
    },

    #[error("could not write the dataset list at {path}: {detail}")]
    NotSaved { path: String, detail: String },

    #[error(
        "that is not a read-only query. `query_data` runs SELECT and nothing else — no {kind}. \
         Writing a table is what a recipe does."
    )]
    NotReadOnly {
        /// What the statement actually was, in the engine's own words.
        kind: String,
    },

    #[error("that query will not run: {detail}")]
    BadQuery { detail: String },

    #[error("{0}")]
    Failed(String),
}

impl DataError {
    /// Turns an engine's own error into one that names the file.
    ///
    /// Engines report in their own vocabulary — a schema error, a parse error
    /// at some byte offset — and none of them mention which of the four files
    /// in the folder it was. The path is the first thing a person needs and
    /// the one thing the engine never says.
    pub fn unreadable(source: &Source, detail: impl std::fmt::Display) -> Self {
        Self::Unreadable {
            path: source.path.display().to_string(),
            format: source.format.label(),
            detail: detail.to_string(),
        }
    }
}

/// Reading and describing tabular data.
///
/// Everything here is read-only, and that is the whole of phase 1. Writing —
/// a transformed table, a recipe, a split — is deliberately absent rather than
/// unimplemented: a method nothing calls is a decision made early and quietly,
/// and the shape of the write half depends on things this phase exists to find
/// out.
#[async_trait]
pub trait Engine: Send + Sync {
    /// How the profile names this engine. Shown, not switched on.
    fn name(&self) -> &'static str;

    /// The columns, read as cheaply as the format allows.
    ///
    /// This is what stands between a typo and a dataset entry pointing at
    /// something unreadable, so it has to actually open the file rather than
    /// trust the extension.
    async fn schema(&self, source: &Source) -> Result<Schema, DataError>;

    /// A full pass: counts, distincts, extremes, the common values.
    async fn profile(&self, source: &Source) -> Result<Profile, DataError>;

    /// A window of rows, at most [`MAX_PAGE`] of them.
    async fn page(&self, source: &Source, offset: u64, limit: u64) -> Result<Page, DataError>;

    /// Runs one read-only query over the given tables.
    ///
    /// Takes a list rather than a source because the useful questions span
    /// datasets — how many of the items in the catalogue were never
    /// interacted with is a join, and a query tool that could not join would
    /// be answering the easy half. Each entry is the name the SQL calls it by
    /// and the file behind it.
    ///
    /// **Read-only is a guarantee this makes, not a hope.** The tool above it
    /// is [`taurus_tools::Effect::Read`], which means no permission prompt, so
    /// an implementation that let `COPY … TO` or `CREATE TABLE` through would
    /// be an unprompted write. See [`DataError::NotReadOnly`], and the
    /// implementation's own note on how it is enforced.
    async fn query(
        &self,
        tables: &[(String, Source)],
        sql: &str,
        limit: u64,
    ) -> Result<QueryResult, DataError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_comes_from_the_extension_whatever_its_case() {
        assert_eq!(Format::of(Path::new("a/b.CSV")).unwrap(), Format::Csv);
        assert_eq!(
            Format::of(Path::new("a/b.parquet")).unwrap(),
            Format::Parquet
        );
        assert_eq!(Format::of(Path::new("a/b.jsonl")).unwrap(), Format::Ndjson);
        assert_eq!(Format::of(Path::new("a/b.tab")).unwrap(), Format::Tsv);
    }

    /// The message is the feature. Somebody who pointed this at a `.xlsx` needs
    /// to learn what *is* readable, not that their file is not.
    #[test]
    fn an_unreadable_extension_lists_the_readable_ones() {
        let error = Format::of(Path::new("book.xlsx")).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("book.xlsx"), "{message}");
        assert!(message.contains(".csv"), "{message}");
        assert!(message.contains(".parquet"), "{message}");
    }

    #[test]
    fn a_file_with_no_extension_at_all_is_refused_rather_than_guessed() {
        assert!(matches!(
            Format::of(Path::new("data/export")),
            Err(DataError::UnknownFormat { .. })
        ));
    }

    #[test]
    fn only_the_kinds_with_an_order_claim_one() {
        assert!(ColumnKind::Number.is_ordered());
        assert!(ColumnKind::Temporal.is_ordered());
        assert!(!ColumnKind::Nested.is_ordered());
        assert!(!ColumnKind::Boolean.is_ordered());
    }
}
