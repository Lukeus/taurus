//! The engine, today: Apache DataFusion.
//!
//! Nothing outside this file names it. That is the arrangement [`crate::engine`]
//! exists to hold, and the reason the type below is the only `pub` thing here.
//!
//! # What a profile costs
//!
//! Two passes over the source, and never more, however wide the table is.
//!
//! - One SQL query counts rows, nulls, and distincts and finds the extremes for
//!   every column at once.
//! - One streaming pass collects the common values for the columns that have
//!   few enough of them to have any.
//!
//! The second pass is written by hand rather than as `GROUP BY` because SQL
//! cannot group by forty columns in one query — it would be forty queries and
//! forty scans, and on a file large enough to want profiling that is the
//! difference between a wait and a walk away. Doing it in one pass costs a
//! bounded map per qualifying column and nothing else.
//!
//! Distincts are counted exactly rather than estimated. `approx_distinct` is
//! right there and would be cheaper, but "about 1,200" cannot answer the
//! question a distinct count is actually asked: whether a column is unique. A
//! profile that takes longer is a cost somebody can see; one that is quietly
//! approximate is a number they will act on.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Instant;

use async_trait::async_trait;
use datafusion::arrow::array::Array;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::common::tree_node::{TreeNode, TreeNodeRecursion};
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::{
    CsvReadOptions, DataFrame, JsonReadOptions, ParquetReadOptions, SessionConfig, SessionContext,
};
use futures::StreamExt;

use crate::engine::{
    ColumnHead, ColumnKind, ColumnProfile, DataError, Distinct, Engine, Format, Page, Profile,
    QueryResult, Schema, Source, ValueCount, MAX_PAGE, MAX_QUERY_ROWS, TOP_VALUES,
};

/// Distinct values above which a column has no "most common" worth showing.
///
/// A column with more than this many different values is an identifier, a
/// timestamp, or free text — and the top five rows of an identifier are five
/// arbitrary values that read like a finding. A thousand is well past any
/// categorical column and well short of the cardinality where the map that
/// collects them would matter.
///
/// Stated rather than silent: a column over the ceiling reports its exact
/// distinct count and an empty `common`, and both the pane and the model's
/// summary say why. See [`crate::engine::ColumnProfile::common`].
pub const TOP_VALUES_MAX_DISTINCT: u64 = 1_000;

/// What one query may hold in memory before it is refused.
///
/// A ceiling rather than a target. DataFusion spills sorts and joins to disk,
/// so an honest query over a large file does not come near this; what does is
/// the accident — a cross join, a `GROUP BY` on a column with a million
/// distinct values — and without a limit that accident is the whole app being
/// killed by the operating system rather than one query failing. Failing is
/// recoverable and says which query did it.
const QUERY_MEMORY_BYTES: usize = 512 * 1024 * 1024;

/// The name every source is registered under inside its own session.
///
/// One context per call, so there is never a second table to collide with —
/// and a fixed name means the SQL below is not assembled around a caller's
/// string.
const TABLE: &str = "src";

/// Reads and describes tabular files with DataFusion.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataFusionEngine;

impl DataFusionEngine {
    pub fn new() -> Self {
        Self
    }

    /// A session with this source registered as [`TABLE`].
    ///
    /// Built per call rather than cached. A cached context would hold the
    /// schema it saw the first time, so a file rewritten between two questions
    /// would answer the second one from before the rewrite — the same argument
    /// `search_code` makes for refreshing its index before it searches, and the
    /// same failure if it is got wrong, which is an answer that looks right.
    async fn open(&self, source: &Source) -> Result<SessionContext, DataError> {
        let ctx = SessionContext::new();
        self.register(&ctx, TABLE, source).await?;
        Ok(ctx)
    }

    /// Registers one file under one name.
    async fn register(
        &self,
        ctx: &SessionContext,
        table: &str,
        source: &Source,
    ) -> Result<(), DataError> {
        let path = source.path.to_str().ok_or_else(|| {
            DataError::Failed(format!("{} is not valid UTF-8", source.path.display()))
        })?;

        // Bound before the options, which borrow it rather than owning it.
        let suffix = extension(source);

        let registered = match source.format {
            Format::Csv | Format::Tsv => {
                let mut options = CsvReadOptions::new()
                    .has_header(true)
                    .file_extension(&suffix);
                if source.format == Format::Tsv {
                    options = options.delimiter(b'\t');
                }
                ctx.register_csv(table, path, options).await
            }
            Format::Parquet => {
                ctx.register_parquet(
                    table,
                    path,
                    ParquetReadOptions::default().file_extension(&suffix),
                )
                .await
            }
            Format::Ndjson => {
                ctx.register_json(
                    table,
                    path,
                    JsonReadOptions::default().file_extension(&suffix),
                )
                .await
            }
        };

        registered.map_err(|e| DataError::unreadable(source, e))
    }

    /// A session holding every named table a query may reach.
    ///
    /// All of them, not just the one being asked about, because the questions
    /// worth asking span datasets — how many catalogue items were never
    /// interacted with is a join, and a query tool that could not join would
    /// answer only the easy half. Registration reads a header or a footer per
    /// file, so a workspace with a handful of datasets pays milliseconds for
    /// the ones the query does not touch.
    ///
    /// The memory ceiling is set here rather than on the profiling sessions
    /// above: those run SQL this crate wrote, and this one runs SQL a model
    /// wrote.
    async fn session_for(&self, tables: &[(String, Source)]) -> Result<SessionContext, DataError> {
        let runtime = RuntimeEnvBuilder::new()
            .with_memory_limit(QUERY_MEMORY_BYTES, 1.0)
            .build_arc()
            .map_err(|e| DataError::Failed(e.to_string()))?;
        let ctx = SessionContext::new_with_config_rt(SessionConfig::new(), runtime);

        for (name, source) in tables {
            // Registered under the dataset's own name, which is what the model
            // will write and what `load_dataset` told it. Those names are
            // already reduced to word characters by the catalog, so nothing
            // here has to quote or sanitize a table name.
            self.register(&ctx, name, source).await?;
        }
        Ok(ctx)
    }

    /// The registered table's columns.
    async fn heads(
        &self,
        ctx: &SessionContext,
        source: &Source,
    ) -> Result<Vec<ColumnHead>, DataError> {
        let table = ctx
            .table(TABLE)
            .await
            .map_err(|e| DataError::unreadable(source, e))?;
        Ok(heads_of(&table.schema().as_arrow().clone()))
    }
}

#[async_trait]
impl Engine for DataFusionEngine {
    fn name(&self) -> &'static str {
        "DataFusion"
    }

    async fn schema(&self, source: &Source) -> Result<Schema, DataError> {
        let ctx = self.open(source).await?;
        let columns = self.heads(&ctx, source).await?;
        let bytes = std::fs::metadata(&source.path)
            .map(|m| m.len())
            .unwrap_or(0);

        // Free for Parquet, which keeps the count in its footer, and refused
        // for everything else rather than guessed from the file size.
        let rows = match source.format {
            Format::Parquet => {
                let count = one_row(&ctx, source, &format!("SELECT count(*) FROM {TABLE}")).await?;
                as_u64(&count, 0)
            }
            _ => None,
        };

        Ok(Schema {
            columns,
            rows,
            bytes,
        })
    }

    async fn profile(&self, source: &Source) -> Result<Profile, DataError> {
        let ctx = self.open(source).await?;
        let heads = self.heads(&ctx, source).await?;
        if heads.is_empty() {
            return Err(DataError::unreadable(source, "it has no columns"));
        }

        let batch = one_row(&ctx, source, &aggregate_sql(&heads)).await?;
        let rows = as_u64(&batch, 0).unwrap_or(0);

        let mut columns = Vec::with_capacity(heads.len());
        for (i, head) in heads.iter().enumerate() {
            let at = 1 + i * 4;
            let present = as_u64(&batch, at).unwrap_or(0);
            let distinct = match head.kind {
                ColumnKind::Nested => Distinct::Unavailable,
                _ => as_u64(&batch, at + 1)
                    .map(|count| Distinct::Exact { count })
                    .unwrap_or(Distinct::Unavailable),
            };
            let (min, max) = if head.kind.is_ordered() {
                (cell(&batch, at + 2, 0), cell(&batch, at + 3, 0))
            } else {
                (None, None)
            };
            columns.push(ColumnProfile {
                head: head.clone(),
                nulls: rows.saturating_sub(present),
                distinct,
                min,
                max,
                common: Vec::new(),
            });
        }

        // Only the columns that can have a meaningful top five, so the second
        // pass reads the fewest columns it can rather than all of them.
        let wanted: Vec<usize> = columns
            .iter()
            .enumerate()
            .filter(|(_, c)| match c.distinct {
                Distinct::Exact { count } => count > 0 && count <= TOP_VALUES_MAX_DISTINCT,
                Distinct::Unavailable => false,
            })
            .map(|(i, _)| i)
            .collect();

        if !wanted.is_empty() {
            let names: Vec<&str> = wanted.iter().map(|&i| heads[i].name.as_str()).collect();
            let counted = count_values(&ctx, source, &names).await?;
            for (slot, counts) in wanted.into_iter().zip(counted) {
                columns[slot].common = top(counts);
            }
        }

        Ok(Profile {
            rows,
            columns,
            engine: self.name().to_string(),
        })
    }

    async fn page(&self, source: &Source, offset: u64, limit: u64) -> Result<Page, DataError> {
        let limit = limit.clamp(1, MAX_PAGE);
        let ctx = self.open(source).await?;
        let columns = self.heads(&ctx, source).await?;

        let total = one_row(&ctx, source, &format!("SELECT count(*) FROM {TABLE}"))
            .await
            .ok()
            .and_then(|b| as_u64(&b, 0))
            .unwrap_or(0);

        // `OFFSET` before `LIMIT` is the order DataFusion's parser wants, and
        // both are inside the query rather than applied to collected batches so
        // that a page past the tenth costs the same as the first.
        let sql = format!("SELECT * FROM {TABLE} LIMIT {limit} OFFSET {offset}");
        let batches = collect(&ctx, source, &sql).await?;

        let mut rows = Vec::new();
        for batch in &batches {
            let formatters = formatters(batch, source)?;
            for row in 0..batch.num_rows() {
                rows.push(
                    batch
                        .columns()
                        .iter()
                        .zip(&formatters)
                        .map(|(array, format)| {
                            (!array.is_null(row)).then(|| format.value(row).to_string())
                        })
                        .collect(),
                );
            }
        }

        Ok(Page {
            columns,
            rows,
            offset,
            total,
        })
    }

    async fn query(
        &self,
        tables: &[(String, Source)],
        sql: &str,
        limit: u64,
    ) -> Result<QueryResult, DataError> {
        let limit = limit.clamp(1, MAX_QUERY_ROWS);
        let ctx = self.session_for(tables).await?;

        // Planned before it is run, and refused here rather than after. This is
        // the whole of the read-only guarantee — see `writes`.
        let plan = ctx
            .state()
            .create_logical_plan(sql)
            .await
            .map_err(|e| DataError::BadQuery {
                detail: readable(&e.to_string()),
            })?;
        if let Some(kind) = writes(&plan) {
            return Err(DataError::NotReadOnly { kind });
        }

        // One past the cap, so a result that filled it exactly can be told from
        // one that was cut short. Wrapped around the plan rather than appended
        // to the text: a query with its own LIMIT or ORDER BY keeps meaning
        // what it said, and nothing is pasted into SQL somebody else wrote.
        //
        // Except over an `EXPLAIN`, which is not a relation and cannot be
        // limited — DataFusion refuses the plan outright. Its output is a
        // handful of rows by construction, so there is nothing to cap.
        let explains = matches!(plan, LogicalPlan::Explain(_) | LogicalPlan::Analyze(_));
        let frame = DataFrame::new(ctx.state(), plan);
        let frame = if explains {
            frame
        } else {
            frame
                .limit(0, Some(limit as usize + 1))
                .map_err(|e| DataError::BadQuery {
                    detail: readable(&e.to_string()),
                })?
        };

        let started = Instant::now();
        let batches = frame.collect().await.map_err(|e| DataError::BadQuery {
            detail: readable(&e.to_string()),
        })?;
        let took_ms = started.elapsed().as_millis() as u64;

        let columns = batches
            .first()
            .map(|batch| heads_of(batch.schema_ref()))
            .unwrap_or_default();

        let mut rows = Vec::new();
        for batch in &batches {
            let formatters = formatters_for(batch)?;
            for row in 0..batch.num_rows() {
                rows.push(
                    batch
                        .columns()
                        .iter()
                        .zip(&formatters)
                        .map(|(array, format)| {
                            (!array.is_null(row)).then(|| format.value(row).to_string())
                        })
                        .collect(),
                );
            }
        }

        let truncated = rows.len() as u64 > limit;
        rows.truncate(limit as usize);

        Ok(QueryResult {
            columns,
            rows,
            truncated,
            took_ms,
        })
    }
}

/// Whether this plan does anything but read, and what it is called if so.
///
/// **An exhaustive match, deliberately.** `query_data` is
/// [`taurus_tools::Effect::Read`], which means it runs with no permission
/// prompt, so a statement that slipped through here would be an unprompted
/// write to the user's disk — `COPY … TO 'anywhere.parquet'` is one line of
/// SQL. A `_ => None` arm would let a future DataFusion release add a writing
/// variant and have this quietly wave it through. Written out, the same
/// release fails to compile until somebody classifies it.
///
/// The whole tree is walked rather than just the root, because `EXPLAIN
/// ANALYZE` carries its subject as a child *and runs it*.
fn writes(plan: &LogicalPlan) -> Option<String> {
    let mut found = None;
    let _ = plan.apply(|node| {
        if let Some(kind) = writes_here(node) {
            found = Some(kind);
            return Ok(TreeNodeRecursion::Stop);
        }
        Ok(TreeNodeRecursion::Continue)
    });
    found
}

fn writes_here(plan: &LogicalPlan) -> Option<String> {
    match plan {
        LogicalPlan::Ddl(statement) => Some(as_sql(statement.name())),
        LogicalPlan::Dml(statement) => Some(as_sql(&statement.op.to_string())),
        LogicalPlan::Copy(_) => Some("COPY".to_string()),
        // `SET`, `PREPARE`, transaction control. None of them writes a file,
        // and all of them change the session out from under a tool that is
        // meant to answer one question and leave nothing behind.
        LogicalPlan::Statement(statement) => Some(as_sql(statement.name())),

        // Reads. Listed rather than defaulted; see the note above.
        LogicalPlan::Projection(_)
        | LogicalPlan::Filter(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Aggregate(_)
        | LogicalPlan::Sort(_)
        | LogicalPlan::Join(_)
        | LogicalPlan::Repartition(_)
        | LogicalPlan::Union(_)
        | LogicalPlan::TableScan(_)
        | LogicalPlan::EmptyRelation(_)
        | LogicalPlan::Subquery(_)
        | LogicalPlan::SubqueryAlias(_)
        | LogicalPlan::Limit(_)
        | LogicalPlan::Values(_)
        | LogicalPlan::Explain(_)
        | LogicalPlan::Analyze(_)
        | LogicalPlan::Extension(_)
        | LogicalPlan::Distinct(_)
        | LogicalPlan::DescribeTable(_)
        | LogicalPlan::Unnest(_)
        | LogicalPlan::RecursiveQuery(_) => None,
    }
}

/// An engine's name for a statement, as the SQL somebody wrote.
///
/// DataFusion calls a `CREATE TABLE … AS SELECT` a `CreateMemoryTable`, which
/// is accurate about the plan and unhelpful in a refusal — the reader wrote
/// SQL and should be told about SQL. Derived rather than mapped, so a
/// statement kind this has never seen still comes out readable instead of
/// falling through to a name nobody typed.
fn as_sql(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut previous = ' ';
    for c in name.chars() {
        // Only at a camelCase hump. Splitting on every capital would turn a
        // name that is already SQL — `COPY` — into `C O P Y`.
        if c.is_uppercase() && (previous.is_lowercase() || previous.is_numeric()) {
            out.push(' ');
        }
        out.extend(c.to_uppercase());
        previous = c;
    }
    out.trim().to_string()
}

/// An engine error, reduced to one line without losing the useful half.
///
/// This was `lines().next()` and that was wrong in a way worth recording. A
/// DataFusion schema error is two lines, and the *second* one is the part that
/// helps:
///
/// ```text
/// Schema error: No field named nope.
/// Valid fields are events.id, events.event, events.price, events.active.
/// ```
///
/// Keeping only the first threw away the column list — from the one message
/// whose whole job is to put a wrong name one step from a right one, and for
/// the reader least able to go and look it up. So what is dropped is the
/// backtrace note DataFusion appends and nothing else; the rest is flowed onto
/// one line, because this goes into a tool result the model pays for on every
/// later request of the turn.
fn readable(message: &str) -> String {
    message
        .lines()
        .take_while(|line| {
            !line
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("backtrace")
        })
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Column heads from an Arrow schema.
fn heads_of(schema: &datafusion::arrow::datatypes::Schema) -> Vec<ColumnHead> {
    schema
        .fields()
        .iter()
        .map(|field| ColumnHead {
            name: field.name().clone(),
            kind: kind_of(field.data_type()),
            type_name: field.data_type().to_string(),
            nullable: field.is_nullable(),
        })
        .collect()
}

/// One formatter per column of a batch, where there is no source to blame.
fn formatters_for(batch: &RecordBatch) -> Result<Vec<ArrayFormatter<'_>>, DataError> {
    let options = FormatOptions::default().with_null("");
    batch
        .columns()
        .iter()
        .map(|array| {
            ArrayFormatter::try_new(array.as_ref(), &options)
                .map_err(|e| DataError::Failed(e.to_string()))
        })
        .collect()
}

/// `SELECT count(*)`, then four expressions per column.
///
/// Four and always four, even for the columns that cannot answer all of them,
/// so the reader can find a column's results by arithmetic rather than by
/// tracking which ones were skipped. A `NULL` literal costs nothing and keeps
/// the two halves from being able to disagree about the layout.
///
/// Every expression is aliased by position. Not cosmetic: two bare `NULL`s in
/// one projection are two columns with the same name, which DataFusion refuses
/// to plan — so a table with a boolean column and a nested one next to each
/// other failed outright, and the aliases are what make the placeholders
/// distinct.
fn aggregate_sql(heads: &[ColumnHead]) -> String {
    let mut sql = String::from("SELECT count(*) AS a0");
    let mut slot = 0;
    let mut push = |sql: &mut String, expression: &str| {
        slot += 1;
        let _ = write!(sql, ", {expression} AS a{slot}");
    };

    for head in heads {
        let column = quoted(&head.name);
        push(&mut sql, &format!("count({column})"));
        match head.kind {
            ColumnKind::Nested => push(&mut sql, "NULL"),
            _ => push(&mut sql, &format!("count(DISTINCT {column})")),
        }
        if head.kind.is_ordered() {
            push(&mut sql, &format!("CAST(min({column}) AS VARCHAR)"));
            push(&mut sql, &format!("CAST(max({column}) AS VARCHAR)"));
        } else {
            push(&mut sql, "NULL");
            push(&mut sql, "NULL");
        }
    }
    let _ = write!(sql, " FROM {TABLE}");
    sql
}

/// Counts every value of the named columns in one pass.
///
/// Streamed rather than collected: the point of doing this by hand is that the
/// whole table never has to be in memory at once, and `collect` would undo
/// that. What is held is one map per column, and every column here was chosen
/// because it has at most [`TOP_VALUES_MAX_DISTINCT`] keys to put in one.
async fn count_values(
    ctx: &SessionContext,
    source: &Source,
    names: &[&str],
) -> Result<Vec<HashMap<Option<String>, u64>>, DataError> {
    let projection = names
        .iter()
        .map(|n| quoted(n))
        .collect::<Vec<_>>()
        .join(", ");
    let df = ctx
        .sql(&format!("SELECT {projection} FROM {TABLE}"))
        .await
        .map_err(|e| DataError::unreadable(source, e))?;
    let mut stream = df
        .execute_stream()
        .await
        .map_err(|e| DataError::unreadable(source, e))?;

    let mut maps = vec![HashMap::new(); names.len()];
    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(|e| DataError::unreadable(source, e))?;
        let formatters = formatters(&batch, source)?;
        for (slot, (array, format)) in batch.columns().iter().zip(&formatters).enumerate() {
            let map = &mut maps[slot];
            for row in 0..batch.num_rows() {
                let key = (!array.is_null(row)).then(|| format.value(row).to_string());
                *map.entry(key).or_insert(0) += 1;
            }
        }
    }
    Ok(maps)
}

/// The [`TOP_VALUES`] most common, most common first.
///
/// Ties break on the value itself so the list is the same on every run. Without
/// it a map's iteration order decides, and a profile that reshuffles between
/// two looks at the same unchanged file is one nobody can trust.
fn top(counts: HashMap<Option<String>, u64>) -> Vec<ValueCount> {
    let mut all: Vec<ValueCount> = counts
        .into_iter()
        .map(|(value, count)| ValueCount { value, count })
        .collect();
    all.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    all.truncate(TOP_VALUES);
    all
}

/// One formatter per column of a batch, so a cell costs no setup.
fn formatters<'a>(
    batch: &'a RecordBatch,
    source: &Source,
) -> Result<Vec<ArrayFormatter<'a>>, DataError> {
    // Null renders as the empty string because nothing reads it: every caller
    // checks `is_null` first and keeps `None`. Setting it explicitly stops a
    // future default of "NULL" from arriving as a literal cell value.
    let options = FormatOptions::default().with_null("");
    batch
        .columns()
        .iter()
        .map(|array| {
            ArrayFormatter::try_new(array.as_ref(), &options)
                .map_err(|e| DataError::unreadable(source, e))
        })
        .collect()
}

async fn collect(
    ctx: &SessionContext,
    source: &Source,
    sql: &str,
) -> Result<Vec<RecordBatch>, DataError> {
    ctx.sql(sql)
        .await
        .map_err(|e| DataError::unreadable(source, e))?
        .collect()
        .await
        .map_err(|e| DataError::unreadable(source, e))
}

/// Runs a query that answers with exactly one row.
async fn one_row(
    ctx: &SessionContext,
    source: &Source,
    sql: &str,
) -> Result<RecordBatch, DataError> {
    let batches = collect(ctx, source, sql).await?;
    batches
        .into_iter()
        .find(|b| b.num_rows() > 0)
        .ok_or_else(|| DataError::unreadable(source, "the file held no rows to describe"))
}

/// One cell of a batch, rendered, or `None` where it is null.
fn cell(batch: &RecordBatch, column: usize, row: usize) -> Option<String> {
    let array = batch.columns().get(column)?;
    if array.is_null(row) {
        return None;
    }
    let options = FormatOptions::default().with_null("");
    ArrayFormatter::try_new(array.as_ref(), &options)
        .ok()
        .map(|f| f.value(row).to_string())
}

/// A count, read back off the aggregate row.
///
/// Through the rendered string rather than by downcasting to the array type
/// that `count` happens to produce. The width of that type is DataFusion's
/// business and has changed before; the decimal digits it prints have not.
fn as_u64(batch: &RecordBatch, column: usize) -> Option<u64> {
    cell(batch, column, 0)?.parse().ok()
}

/// A column name, safe to drop into SQL.
///
/// Doubling the quote is the standard escape and the only one needed: a
/// delimited identifier ends at its closing quote, so a name containing one
/// closes the identifier early and everything after it is parsed as syntax.
/// Column names come out of file headers, which nobody validates.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The extension a reader should accept, taken from the file itself.
///
/// DataFusion filters candidate paths by extension, and the default is the
/// format's usual one — `.csv`, `.json`. A `.tsv` handed to the CSV reader or a
/// `.jsonl` handed to the NDJSON reader matches nothing and registers an empty
/// table, which is the worst shape of failure available here: no error, no
/// rows, and a profile that confidently reports zero. Reading the spelling off
/// the source is what lets [`Format`] own the list of accepted ones instead of
/// it being written out a second time.
fn extension(source: &Source) -> String {
    source
        .path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default()
}

/// Which of the three behaviours an Arrow type has.
fn kind_of(data_type: &DataType) -> ColumnKind {
    use DataType::*;
    match data_type {
        Boolean => ColumnKind::Boolean,
        Int8
        | Int16
        | Int32
        | Int64
        | UInt8
        | UInt16
        | UInt32
        | UInt64
        | Float16
        | Float32
        | Float64
        | Decimal128(_, _)
        | Decimal256(_, _) => ColumnKind::Number,
        Date32 | Date64 | Time32(_) | Time64(_) | Timestamp(_, _) | Duration(_) | Interval(_) => {
            ColumnKind::Temporal
        }
        Utf8 | LargeUtf8 | Utf8View => ColumnKind::Text,
        List(_)
        | LargeList(_)
        | ListView(_)
        | LargeListView(_)
        | FixedSizeList(_, _)
        | Struct(_)
        | Map(_, _)
        | Union(_, _)
        | Dictionary(_, _)
        | RunEndEncoded(_, _) => ColumnKind::Nested,
        // Binary and null. Neither orders usefully and neither is nested; text
        // is the honest bucket, and `type_name` carries what it actually is.
        _ => ColumnKind::Text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A workspace with one file in it, written and handed back as a source.
    fn file(dir: &TempDir, name: &str, contents: &str) -> Source {
        let path = dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        Source::at(path).unwrap()
    }

    const EVENTS: &str = "\
id,event,price,active
1,view,10.5,true
2,click,20.0,false
3,view,,true
4,view,5.25,true
5,purchase,99.99,false
";

    fn column<'a>(profile: &'a Profile, name: &str) -> &'a ColumnProfile {
        profile
            .columns
            .iter()
            .find(|c| c.head.name == name)
            .unwrap_or_else(|| panic!("no column {name} in {:?}", profile.columns))
    }

    #[tokio::test]
    async fn a_csv_reports_its_columns_and_refuses_to_guess_its_row_count() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);
        let schema = DataFusionEngine::new().schema(&source).await.unwrap();

        let names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "event", "price", "active"]);
        assert_eq!(column_kind(&schema, "id"), ColumnKind::Number);
        assert_eq!(column_kind(&schema, "event"), ColumnKind::Text);
        assert_eq!(column_kind(&schema, "price"), ColumnKind::Number);
        assert_eq!(column_kind(&schema, "active"), ColumnKind::Boolean);
        // The header is all a CSV can be asked for. Counting rows means
        // reading them, and a guess would be a number nobody could see was
        // wrong.
        assert_eq!(schema.rows, None);
        assert!(schema.bytes > 0);
    }

    fn column_kind(schema: &Schema, name: &str) -> ColumnKind {
        schema.columns.iter().find(|c| c.name == name).unwrap().kind
    }

    #[tokio::test]
    async fn a_profile_counts_rows_nulls_distincts_and_extremes() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);
        let profile = DataFusionEngine::new().profile(&source).await.unwrap();

        assert_eq!(profile.rows, 5);
        assert_eq!(profile.engine, "DataFusion");

        let event = column(&profile, "event");
        assert_eq!(event.nulls, 0);
        assert_eq!(event.distinct, Distinct::Exact { count: 3 });
        assert_eq!(event.min.as_deref(), Some("click"));
        assert_eq!(event.max.as_deref(), Some("view"));

        // The one row with no price. A null count that came from the declared
        // schema rather than the data would say 0 here.
        let price = column(&profile, "price");
        assert_eq!(price.nulls, 1);
        assert_eq!(price.distinct, Distinct::Exact { count: 4 });

        // Booleans have no order worth reporting, and claiming one would put a
        // meaningless `false … true` on every flag column in the pane.
        let active = column(&profile, "active");
        assert_eq!(active.min, None);
        assert_eq!(active.max, None);
        assert_eq!(active.distinct, Distinct::Exact { count: 2 });
    }

    #[tokio::test]
    async fn the_common_values_are_counted_and_ordered() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);
        let profile = DataFusionEngine::new().profile(&source).await.unwrap();

        let event = column(&profile, "event");
        assert_eq!(
            event.common,
            vec![
                ValueCount {
                    value: Some("view".into()),
                    count: 3
                },
                ValueCount {
                    value: Some("click".into()),
                    count: 1
                },
                ValueCount {
                    value: Some("purchase".into()),
                    count: 1
                },
            ],
            "most common first, ties broken by value so two runs agree"
        );

        // Nulls are a group like any other. Seeing "40% null" at the top of a
        // distribution is most of why somebody looks at one.
        let price = column(&profile, "price");
        assert!(
            price.common.contains(&ValueCount {
                value: None,
                count: 1
            }),
            "{:?}",
            price.common
        );
    }

    #[tokio::test]
    async fn a_column_with_more_values_than_a_top_five_says_anything_about_gets_none() {
        let dir = TempDir::new().unwrap();
        let mut csv = String::from("id\n");
        for i in 0..(TOP_VALUES_MAX_DISTINCT + 10) {
            csv.push_str(&format!("k{i}\n"));
        }
        let source = file(&dir, "ids.csv", &csv);
        let profile = DataFusionEngine::new().profile(&source).await.unwrap();

        let id = column(&profile, "id");
        // The count is still exact — the ceiling withholds the list, never the
        // number, which is what lets the summary say *why* the list is empty.
        assert_eq!(
            id.distinct,
            Distinct::Exact {
                count: TOP_VALUES_MAX_DISTINCT + 10
            }
        );
        assert!(id.common.is_empty());
    }

    #[tokio::test]
    async fn a_page_keeps_null_and_the_empty_string_apart() {
        // The reason a cell is `Option<String>` rather than `String`. Both draw
        // as nothing, and telling them apart is most of what looking at raw
        // rows is for — so this uses NDJSON, where the file itself can hold one
        // of each in the same column.
        let dir = TempDir::new().unwrap();
        let source = file(
            &dir,
            "rows.ndjson",
            "{\"id\":1,\"name\":\"alice\"}\n{\"id\":2,\"name\":null}\n{\"id\":3,\"name\":\"\"}\n",
        );
        let page = DataFusionEngine::new().page(&source, 0, 10).await.unwrap();

        assert_eq!(page.total, 3);
        let names: Vec<Option<String>> = page
            .rows
            .iter()
            .map(|row| row[name_at(&page)].clone())
            .collect();
        assert_eq!(
            names,
            vec![Some("alice".to_string()), None, Some(String::new())]
        );
    }

    fn name_at(page: &Page) -> usize {
        page.columns.iter().position(|c| c.name == "name").unwrap()
    }

    #[tokio::test]
    async fn a_page_windows_the_rows_and_says_how_many_there_are_in_all() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);
        let engine = DataFusionEngine::new();

        let page = engine.page(&source, 1, 2).await.unwrap();
        assert_eq!(page.total, 5, "the total is the file's, not the page's");
        assert_eq!(page.offset, 1);
        assert_eq!(page.rows.len(), 2);
        assert_eq!(page.rows[0][0].as_deref(), Some("2"));
        assert_eq!(page.rows[1][0].as_deref(), Some("3"));

        // Past the end is an empty page rather than an error: a grid asking for
        // a window that a shrunken file no longer has should draw nothing, not
        // raise.
        let past = engine.page(&source, 500, 10).await.unwrap();
        assert!(past.rows.is_empty());
        assert_eq!(past.total, 5);
    }

    #[tokio::test]
    async fn a_page_is_capped_however_much_is_asked_for() {
        let dir = TempDir::new().unwrap();
        let mut csv = String::from("id\n");
        for i in 0..(MAX_PAGE + 50) {
            csv.push_str(&format!("{i}\n"));
        }
        let source = file(&dir, "many.csv", &csv);
        let page = DataFusionEngine::new()
            .page(&source, 0, MAX_PAGE * 4)
            .await
            .unwrap();
        assert_eq!(page.rows.len() as u64, MAX_PAGE);
        assert_eq!(page.total, MAX_PAGE + 50);
    }

    /// Tab-separated files, and the reason `file_extension` is set at all: the
    /// reader filters candidates by extension, so a `.tsv` handed to it with
    /// the default `.csv` matches nothing and registers an empty table. No
    /// error, no rows, and a profile that confidently reports zero.
    #[tokio::test]
    async fn a_tsv_reads_with_its_own_delimiter_and_its_own_extension() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.tsv", "id\tevent\n1\tview\n2\tclick\n");
        let profile = DataFusionEngine::new().profile(&source).await.unwrap();

        assert_eq!(profile.rows, 2, "an empty table would report 0 rows here");
        let names: Vec<&str> = profile
            .columns
            .iter()
            .map(|c| c.head.name.as_str())
            .collect();
        assert_eq!(names, ["id", "event"], "the tab was read as a delimiter");
    }

    /// The same trap on the other reader: `.jsonl` against a default of
    /// `.json`.
    #[tokio::test]
    async fn newline_delimited_json_reads_under_every_spelling() {
        let dir = TempDir::new().unwrap();
        for name in ["rows.ndjson", "rows.jsonl", "rows.json"] {
            let source = file(&dir, name, "{\"a\":1}\n{\"a\":2}\n");
            let profile = DataFusionEngine::new().profile(&source).await.unwrap();
            assert_eq!(profile.rows, 2, "{name} read as an empty table");
        }
    }

    #[tokio::test]
    async fn a_parquet_file_knows_its_row_count_without_being_scanned() {
        let dir = TempDir::new().unwrap();
        let csv = file(&dir, "events.csv", EVENTS);
        let parquet = write_parquet(&dir, &csv).await;

        let schema = DataFusionEngine::new().schema(&parquet).await.unwrap();
        // The half a CSV cannot answer. It is in the footer, so it costs
        // nothing, and `load_dataset` reports it straight away.
        assert_eq!(schema.rows, Some(5));

        let profile = DataFusionEngine::new().profile(&parquet).await.unwrap();
        assert_eq!(profile.rows, 5);
        assert_eq!(column(&profile, "price").nulls, 1);
    }

    /// Round-trips the CSV through DataFusion's own writer, so the test does
    /// not have to hand-assemble a Parquet file to read one back.
    async fn write_parquet(dir: &TempDir, from: &Source) -> Source {
        let engine = DataFusionEngine::new();
        let ctx = engine.open(from).await.unwrap();
        let out: PathBuf = dir.path().join("events.parquet");
        ctx.sql(&format!("SELECT * FROM {TABLE}"))
            .await
            .unwrap()
            .write_parquet(out.to_str().unwrap(), Default::default(), None)
            .await
            .unwrap();
        Source::at(out).unwrap()
    }

    #[tokio::test]
    async fn a_file_that_is_not_what_it_claims_names_itself_in_the_error() {
        // An engine reports a parse failure in its own vocabulary and never
        // mentions which of the four files in the folder it was.
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "broken.parquet", "this is not a parquet file");
        let error = DataFusionEngine::new()
            .schema(&source)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("broken.parquet"), "{error}");
        assert!(error.contains("Parquet"), "{error}");
    }

    /// Column names come out of file headers, which nobody validates. A name
    /// holding a quote closes the delimited identifier early and everything
    /// after it parses as syntax.
    #[tokio::test]
    async fn a_column_name_holding_a_quote_does_not_break_the_query() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "odd.csv", "\"we\"\"ird\",b\n1,2\n");
        let profile = DataFusionEngine::new().profile(&source).await.unwrap();
        assert_eq!(profile.rows, 1);
        assert!(
            profile.columns.iter().any(|c| c.head.name.contains('"')),
            "{:?}",
            profile.columns
        );
    }

    async fn ask(source: &Source, sql: &str) -> Result<QueryResult, DataError> {
        DataFusionEngine::new()
            .query(
                &[("events".to_string(), source.clone())],
                sql,
                MAX_QUERY_ROWS,
            )
            .await
    }

    #[tokio::test]
    async fn a_query_answers_with_its_own_columns_and_rows() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);

        let result = ask(
            &source,
            "SELECT event, count(*) AS n FROM events GROUP BY event ORDER BY n DESC, event",
        )
        .await
        .unwrap();

        let names: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["event", "n"]);
        assert_eq!(result.rows[0][0].as_deref(), Some("view"));
        assert_eq!(result.rows[0][1].as_deref(), Some("3"));
        assert!(!result.truncated);
    }

    /// The reason `query` takes a list. Half the useful questions about a
    /// dataset are about how it lines up with another one.
    #[tokio::test]
    async fn a_query_can_join_two_datasets() {
        let dir = TempDir::new().unwrap();
        let events = file(&dir, "events.csv", EVENTS);
        let items = file(&dir, "items.csv", "id,label\n1,alpha\n2,beta\n3,gamma\n");

        let result = DataFusionEngine::new()
            .query(
                &[
                    ("events".to_string(), events),
                    ("items".to_string(), items),
                ],
                "SELECT i.label, e.event FROM events e JOIN items i ON e.id = i.id ORDER BY i.label",
                MAX_QUERY_ROWS,
            )
            .await
            .unwrap();

        // Three, not five: the catalogue has ids 1..3 and the events have
        // 1..5, which is the shape of the question — how much of one side has
        // no match on the other.
        assert_eq!(result.rows.len(), 3);
        assert_eq!(result.rows[0][0].as_deref(), Some("alpha"));
    }

    /// The guarantee the whole tool rests on.
    ///
    /// `query_data` is `Effect::Read`, so it runs with no permission prompt.
    /// Every one of these is a statement that would touch the disk or the
    /// session, and `COPY … TO` in particular is one line of SQL away from an
    /// unprompted write anywhere the process can reach.
    #[tokio::test]
    async fn nothing_but_a_read_gets_through() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);
        let out = dir.path().join("escaped.parquet");

        let refused = [
            format!("COPY events TO '{}'", out.display()),
            format!(
                "COPY (SELECT * FROM events) TO '{}' STORED AS PARQUET",
                out.display()
            ),
            "CREATE TABLE sneaky AS SELECT * FROM events".to_string(),
            "CREATE EXTERNAL TABLE t STORED AS CSV LOCATION '/etc/passwd'".to_string(),
            "DROP TABLE events".to_string(),
            "INSERT INTO events VALUES (9, 'x', 1.0, true)".to_string(),
            "SET datafusion.execution.batch_size = 1".to_string(),
            // The reason the whole tree is walked rather than the root: this
            // one *runs* what it is explaining.
            format!("EXPLAIN ANALYZE COPY events TO '{}'", out.display()),
        ];

        for sql in refused {
            let error = ask(&source, &sql).await.unwrap_err();
            assert!(
                matches!(error, DataError::NotReadOnly { .. }),
                "{sql} was not refused as a write: {error}"
            );
        }

        assert!(!out.exists(), "a refused statement still wrote a file");
    }

    /// `EXPLAIN` on its own plans and does not run, so it stays allowed — it is
    /// the thing somebody reaches for when a query is slow.
    #[tokio::test]
    async fn explaining_a_select_is_still_a_read() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);
        assert!(ask(&source, "EXPLAIN SELECT * FROM events").await.is_ok());
    }

    /// DataFusion can be configured to treat a quoted path in `FROM` as a
    /// table. It is off unless `enable_url_table` is called, and nothing here
    /// calls it — which is the only thing standing between a read-only query
    /// and every file on the machine, so it is worth a test rather than a
    /// memory of a default.
    #[tokio::test]
    async fn a_file_path_is_not_a_table() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);
        let elsewhere = file(&dir, "secret.csv", "a\n1\n");

        let error = ask(
            &source,
            &format!("SELECT * FROM '{}'", elsewhere.path.display()),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DataError::BadQuery { .. }), "{error}");
    }

    #[tokio::test]
    async fn a_query_that_will_not_plan_says_why_in_one_line() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);

        let error = ask(&source, "SELECT nope FROM events").await.unwrap_err();
        let message = error.to_string();
        assert!(message.contains("nope"), "{message}");
        // The half that matters, and the half a `lines().next()` threw away:
        // the columns that *do* exist. This is the message whose whole job is
        // to put a wrong name one step from a right one.
        assert!(message.contains("events.price"), "{message}");
        // Still one line — this goes into a tool result the model pays for.
        assert_eq!(message.lines().count(), 1, "{message}");
    }

    #[tokio::test]
    async fn a_name_that_is_not_a_table_is_a_query_error_not_a_crash() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);
        assert!(matches!(
            ask(&source, "SELECT * FROM nosuchtable").await.unwrap_err(),
            DataError::BadQuery { .. }
        ));
    }

    /// A result that filled the cap exactly and one that was cut short look
    /// identical from outside, and only one of them is the whole answer.
    #[tokio::test]
    async fn a_result_says_whether_it_was_cut_short() {
        let dir = TempDir::new().unwrap();
        let mut csv = String::from("id\n");
        for i in 0..(MAX_QUERY_ROWS + 10) {
            csv.push_str(&format!("{i}\n"));
        }
        let source = file(&dir, "events.csv", &csv);

        let all = ask(&source, "SELECT * FROM events").await.unwrap();
        assert_eq!(all.rows.len() as u64, MAX_QUERY_ROWS);
        assert!(all.truncated);

        let few = ask(&source, "SELECT * FROM events LIMIT 3").await.unwrap();
        assert_eq!(few.rows.len(), 3);
        assert!(!few.truncated, "three rows is the whole answer");
    }

    /// The cap is applied around the plan, not pasted onto the text, so a
    /// query that ordered its own rows still gets the ones it asked for.
    #[tokio::test]
    async fn the_cap_does_not_disturb_an_order_the_query_asked_for() {
        let dir = TempDir::new().unwrap();
        let source = file(&dir, "events.csv", EVENTS);
        let result = ask(&source, "SELECT id FROM events ORDER BY id DESC")
            .await
            .unwrap();
        assert_eq!(result.rows[0][0].as_deref(), Some("5"));
    }

    #[test]
    fn a_statement_is_refused_in_the_words_somebody_typed() {
        // The reader wrote SQL; `CreateMemoryTable` is the plan's name for it.
        assert_eq!(as_sql("CreateMemoryTable"), "CREATE MEMORY TABLE");
        assert_eq!(as_sql("DropTable"), "DROP TABLE");
        assert_eq!(as_sql("SetVariable"), "SET VARIABLE");
        assert_eq!(as_sql("Insert Into"), "INSERT INTO");
        assert_eq!(as_sql("COPY"), "COPY");
    }

    #[test]
    fn an_error_keeps_its_advice_and_drops_its_backtrace() {
        let message = readable(
            "Schema error: No field named nope.\nValid fields are events.id, events.price.\n\nbacktrace: 0: std::backtrace",
        );
        assert_eq!(
            message,
            "Schema error: No field named nope. Valid fields are events.id, events.price."
        );
    }

    #[test]
    fn an_identifier_doubles_the_quotes_inside_it() {
        assert_eq!(quoted("plain"), "\"plain\"");
        assert_eq!(quoted("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn the_aggregate_row_has_four_slots_per_column_whatever_the_column_is() {
        // The reader finds a column's results by arithmetic. A kind that
        // emitted three expressions would shift every column after it.
        let heads = vec![
            ColumnHead {
                name: "a".into(),
                kind: ColumnKind::Number,
                type_name: "Int64".into(),
                nullable: true,
            },
            ColumnHead {
                name: "b".into(),
                kind: ColumnKind::Boolean,
                type_name: "Boolean".into(),
                nullable: true,
            },
            ColumnHead {
                name: "c".into(),
                kind: ColumnKind::Nested,
                type_name: "Struct".into(),
                nullable: true,
            },
        ];
        let sql = aggregate_sql(&heads);
        assert_eq!(sql.matches(',').count(), 3 * 4, "{sql}");
        // And every slot is named, or the planner refuses two `NULL`s in one
        // projection — which is exactly how this was found.
        for slot in 0..=(3 * 4) {
            assert!(
                sql.contains(&format!(" AS a{slot}")),
                "slot {slot} unnamed: {sql}"
            );
        }
    }
}
