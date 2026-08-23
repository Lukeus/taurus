//! Load, profile, and page a real data file, and say what each one cost.
//!
//! The unit tests read a five-row CSV and a round-tripped Parquet file. What
//! they cannot show is the only thing that decides whether this feature is any
//! good: how it behaves on a file somebody actually has — a million rows, forty
//! columns, a header with a space in it, a column that is 90% empty. Those are
//! properties of real data, and there is no fixture for them.
//!
//! ```sh
//! cargo run -p taurus-data --example probe -- ~/data/events.parquet
//! cargo run -p taurus-data --example probe -- ~/data/interactions.csv
//!
//! # With a query, which is the other half of what this checks. The table is
//! # named the way `load_dataset` would name it, so the SQL here is the SQL
//! # you would type in the pane.
//! cargo run -p taurus-data --example probe -- ~/data/interactions.csv \
//!   "SELECT category, count(*) AS n FROM interactions GROUP BY 1 ORDER BY n DESC"
//! ```
//!
//! Needs no provider and no model. Run it on something large before touching
//! the caps in `df.rs` or the two-pass arrangement in `profile` — the numbers
//! it prints are the only evidence that the second pass is worth what it costs.
//!
//! The three timings are the point:
//!
//! - **schema** is what `load_dataset` pays. It must stay flat as the file
//!   grows — it reads a header, or a Parquet footer, and nothing else. If this
//!   climbs with file size, something is scanning that should not be.
//! - **profile** is a full pass and is allowed to be slow. It is the number to
//!   watch when changing how the aggregate query is built.
//! - **page** must be flat in the *offset*, which is why it is measured twice:
//!   once at the top of the file and once deep into it. A page a thousand rows
//!   in that costs more than the first is a `LIMIT` being applied after the
//!   rows were collected rather than inside the query.
//! - **query**, when one is given, is the only number here that depends on
//!   what was asked. It also proves the refusal: hand it a `COPY … TO` and it
//!   should print the refusal rather than a file.

use std::path::PathBuf;
use std::time::Instant;

use taurus_data::{DataFusionEngine, Distinct, Engine, Source};

#[tokio::main]
async fn main() {
    let Some(argument) = std::env::args().nth(1) else {
        eprintln!(
            "usage: cargo run -p taurus-data --example probe -- <file> [sql]\n\n\
             Reads a .csv, .tsv, .parquet, .ndjson, .jsonl, or .json file and reports what\n\
             loading, profiling, and paging it cost. Point it at something large.\n\n\
             With a second argument, runs it as a query. The table is named the way\n\
             load_dataset would name it — the file's stem, lowercased."
        );
        std::process::exit(2);
    };
    let sql = std::env::args().nth(2);

    let path = PathBuf::from(&argument)
        .canonicalize()
        .unwrap_or_else(|e| panic!("{argument}: {e}"));
    let source = Source::at(&path).unwrap_or_else(|e| panic!("{e}"));
    let engine = DataFusionEngine::new();

    println!("file:   {}", path.display());
    println!("format: {}\n", source.format.label());

    // What `load_dataset` pays. Flat in the size of the file, or something is
    // scanning that should not be.
    let started = Instant::now();
    let schema = engine
        .schema(&source)
        .await
        .unwrap_or_else(|e| panic!("could not read it: {e}"));
    println!(
        "schema:   {:>9.1?}   {} columns, {:.1} MB{}",
        started.elapsed(),
        schema.columns.len(),
        schema.bytes as f64 / (1024.0 * 1024.0),
        match schema.rows {
            Some(rows) => format!(", {rows} rows from the footer"),
            None => String::new(),
        }
    );

    let started = Instant::now();
    let profile = engine
        .profile(&source)
        .await
        .unwrap_or_else(|e| panic!("could not profile it: {e}"));
    let profiled = started.elapsed();
    println!(
        "profile:  {profiled:>9.1?}   {} rows, two passes",
        profile.rows
    );

    // Flat in the offset. A deep page that costs more than the first one means
    // the limit is being applied after the rows were collected.
    let started = Instant::now();
    let first = engine.page(&source, 0, 100).await.expect("the first page");
    let near = started.elapsed();
    let deep_at = profile.rows.saturating_sub(100);
    let started = Instant::now();
    let _ = engine
        .page(&source, deep_at, 100)
        .await
        .expect("a deep page");
    println!(
        "page:     {near:>9.1?} at row 0, {:>9.1?} at row {deep_at}",
        started.elapsed()
    );

    println!(
        "\n{:<28} {:<18} {:>10} {:>12}",
        "column", "type", "nulls", "distinct"
    );
    for column in &profile.columns {
        let share = if profile.rows == 0 {
            0.0
        } else {
            (column.nulls as f64 / profile.rows as f64) * 100.0
        };
        println!(
            "{:<28} {:<18} {:>9.1}% {:>12}",
            cut(&column.head.name, 28),
            cut(&column.head.type_name, 18),
            share,
            match column.distinct {
                Distinct::Exact { count } => count.to_string(),
                Distinct::Unavailable => "—".into(),
            }
        );
        if !column.common.is_empty() {
            let common: Vec<String> = column
                .common
                .iter()
                .map(|v| {
                    format!(
                        "{} {}",
                        cut(v.value.as_deref().unwrap_or("(null)"), 20),
                        v.count
                    )
                })
                .collect();
            println!("{:<28} {}", "", common.join("  ·  "));
        }
    }

    if let Some(sql) = &sql {
        // Named the way `load_dataset` would name it, so the SQL that works
        // here is the SQL that works in the app.
        let table = taurus_data::catalog::suggest_name(&path);
        println!("\nquery, over `{table}`:\n  {sql}\n");

        // Not timed from out here: the result carries the engine's own
        // measurement, taken around execution rather than around the session
        // setup in front of it.
        match engine
            .query(&[(table, source.clone())], sql, taurus_data::MAX_QUERY_ROWS)
            .await
        {
            Ok(result) => {
                let widths: Vec<usize> = result
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, column)| {
                        result
                            .rows
                            .iter()
                            .map(|row| shown(row.get(i)).chars().count())
                            .chain(std::iter::once(column.name.chars().count()))
                            .max()
                            .unwrap_or(8)
                            .min(24)
                    })
                    .collect();

                let heads: Vec<String> = result
                    .columns
                    .iter()
                    .zip(&widths)
                    .map(|(c, w)| format!("{:w$}", cut(&c.name, *w), w = w))
                    .collect();
                println!("  {}", heads.join("  "));
                for row in &result.rows {
                    let cells: Vec<String> = widths
                        .iter()
                        .enumerate()
                        .map(|(i, w)| format!("{:w$}", cut(&shown(row.get(i)), *w), w = w))
                        .collect();
                    println!("  {}", cells.join("  "));
                }
                println!(
                    "\n  {} row{}{} · {} ms",
                    result.rows.len(),
                    if result.rows.len() == 1 { "" } else { "s" },
                    if result.truncated { " (capped)" } else { "" },
                    result.took_ms
                );
            }
            // Printed rather than panicked, because a refusal is a correct
            // outcome here and the message is the thing being checked.
            Err(e) => println!("  refused: {e}"),
        }
    }

    // Printed last because it is the part a reader checks by eye: the types
    // being right, and the first rows looking like what the file actually
    // holds, is the half no timing can tell you.
    println!("\nfirst three rows:");
    let names: Vec<&str> = first.columns.iter().map(|c| c.name.as_str()).collect();
    println!("  {}", names.join(" | "));
    for row in first.rows.iter().take(3) {
        let cells: Vec<String> = row
            .iter()
            .map(|cell| match cell {
                // The distinction the whole payload type exists for, and the
                // one worth being able to see here too.
                None => "‹null›".to_string(),
                Some(value) if value.is_empty() => "‹empty›".to_string(),
                Some(value) => cut(value, 20),
            })
            .collect();
        println!("  {}", cells.join(" | "));
    }
}

/// A cell, with null and the empty string told apart.
fn shown(cell: Option<&Option<String>>) -> String {
    match cell {
        Some(Some(value)) if value.is_empty() => "‹empty›".to_string(),
        Some(Some(value)) => value.clone(),
        _ => "‹null›".to_string(),
    }
}

/// Keeps one long value from wrapping every row in an 80-column terminal.
fn cut(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    format!("{}…", value.chars().take(width - 1).collect::<String>())
}
