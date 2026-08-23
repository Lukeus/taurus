//! Run a real recipe over real data, and say what each step did to it.
//!
//! The other half of what `probe` checks. `probe` answers questions about a
//! file; this one *writes* one, which is the only part of this crate that
//! changes anything the user can see — and the part whose failure modes are
//! properties of real data rather than of a fixture. A step that drops every
//! row because a column arrived as text, a join that fans out because a key is
//! not unique, an output that turns out to be a directory of partitions: none
//! of those show up in a five-row CSV.
//!
//! ```sh
//! # A recipe that names its own files — `source: data/interactions.csv` — is
//! # self-contained and needs no arguments after the recipe.
//! cargo run -p taurus-data --example recipe -- .taurus/recipes/clean.sql
//!
//! # A recipe that names a loaded dataset instead needs the file behind it.
//! # Each one is registered under the name `load_dataset` would give it — the
//! # stem, lowercased — so the recipe here is the recipe the app runs.
//! cargo run -p taurus-data --example recipe -- \
//!   .taurus/recipes/clean.sql ~/data/interactions.csv ~/data/items.csv
//! ```
//!
//! Needs no provider, no model, and no catalog. What it will not check is the
//! permission prompt and the rewind entry, which live above this crate — for
//! those, `taurus data run <recipe>` is the same run with the harness around
//! it.
//!
//! The output goes where the recipe's `output:` says, relative to the current
//! directory. It is a real write: point it somewhere you do not mind.

use std::path::{Path, PathBuf};

use taurus_data::{catalog, recipe, DataFusionEngine, Engine, Source};

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let Some(recipe_path) = arguments.next() else {
        eprintln!(
            "usage: cargo run -p taurus-data --example recipe -- <recipe.sql> <file> [file …]\n\n\
             Runs a recipe. Each file given is registered under the name load_dataset\n\
             would give it — its filename, lowercased — standing in for the workspace's\n\
             dataset list. A recipe that names its own files needs none of them.\n\
             Writes where its `output:` says, relative to the current directory."
        );
        std::process::exit(2);
    };
    let files: Vec<PathBuf> = arguments.map(PathBuf::from).collect();

    let path = Path::new(&recipe_path);
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{recipe_path}: {e}"));
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recipe");
    let recipe = recipe::parse(&text, name, &recipe_path).unwrap_or_else(|e| panic!("{e}"));

    let tables: Vec<(String, Source)> = files
        .iter()
        .map(|file| {
            let path = file
                .canonicalize()
                .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            let source = Source::at(&path).unwrap_or_else(|e| panic!("{e}"));
            (catalog::suggest_name(&path), source)
        })
        .collect();

    // Through the same resolver the tool and the pane use, so a recipe that
    // works here works there — including one that names its own files and so
    // needs nothing loaded at all.
    let (tables, start) =
        recipe::resolve(&recipe, Path::new("."), tables).unwrap_or_else(|e| panic!("{e}"));

    println!("recipe: {}", recipe.path);
    println!("source: {} (as `{start}`)", recipe.source);
    println!("output: {}", recipe.output);
    println!(
        "tables: {}\n",
        tables
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let steps: Vec<(String, String)> = recipe
        .steps
        .iter()
        .map(|s| (s.title.clone(), s.sql.clone()))
        .collect();

    let run = match DataFusionEngine::new()
        .materialize(&tables, &start, &steps, Path::new(&recipe.output))
        .await
    {
        Ok(run) => run,
        // Printed rather than panicked: a refusal is a correct outcome here,
        // and the message is the thing being checked. Point this at a recipe
        // with a `COPY … TO` in it and the refusal should name the step.
        Err(error) => {
            println!("refused: {error}");
            std::process::exit(1);
        }
    };

    // The deltas are the point. A step meant to drop a hundred duplicates that
    // dropped four hundred thousand rows is invisible in the SQL and unmissable
    // in this column — which is the whole argument for reporting per step.
    let width = run
        .steps
        .iter()
        .map(|s| s.title.chars().count())
        .max()
        .unwrap_or(0);
    println!(
        "{:>13}  {:>10}  before any step",
        thousands(run.started_with),
        ""
    );
    let mut previous = run.started_with;
    for (index, step) in run.steps.iter().enumerate() {
        println!(
            "{:>13}  {:>10}  {}. {:width$}  {:>6} ms",
            thousands(step.rows),
            delta(previous, step.rows),
            index + 1,
            step.title,
            step.took_ms,
            width = width
        );
        previous = step.rows;
    }

    println!(
        "\nwrote {} rows × {} columns, {:.1} MB, in {:.1} s",
        thousands(run.rows),
        run.columns.len(),
        run.bytes as f64 / (1024.0 * 1024.0),
        run.took_ms as f64 / 1000.0
    );
    // Read back off the file, so the types are the ones a later step or a
    // `load_dataset` will actually see rather than the ones the plan intended.
    for column in &run.columns {
        println!("  {:<28} {}", column.name, column.type_name);
    }
}

fn delta(before: u64, after: u64) -> String {
    match after.cmp(&before) {
        std::cmp::Ordering::Equal => "—".to_string(),
        std::cmp::Ordering::Less => format!("−{}", thousands(before - after)),
        std::cmp::Ordering::Greater => format!("+{}", thousands(after - before)),
    }
}

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
