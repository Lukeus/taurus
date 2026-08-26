//! Does cutting at structure actually retrieve better than cutting on a line
//! count?
//!
//! ```sh
//! ollama pull nomic-embed-text
//! cargo run -p taurus-index --example retrieval -- . nomic-embed-text
//! ```
//!
//! `probe` next door prints hits for a reader to judge by eye, which is the
//! right check for "is this any good at all" and no check at all for "is this
//! better than what it replaced". This answers the second question with a
//! number, because the alternative is shipping a change to how every vector in
//! the index is built on the strength of it sounding sensible — and this
//! repository has already been caught doing that once. `rerank_model` is empty
//! by default for exactly this reason: the plan that added reranking gated it
//! on beating cosine, and nobody ran the gate.
//!
//! # What it compares
//!
//! Three variants, so the two halves of the change can be told apart:
//!
//! - **lines** — forty-line windows with ten lines of overlap, embedding the
//!   body and nothing else. What the index did before.
//! - **structure** — cuts snapped to where a top-level thing starts, still
//!   embedding the body alone. What snapping is worth on its own.
//! - **structure+heading** — the same cuts, embedding the file's path and the
//!   definitions the chunk sits inside as well. What ships.
//!
//! # What it measures
//!
//! Questions phrased the way somebody asks them, each with the file that
//! actually answers it. For each, the rank of that file in the results — so
//! **MRR** is how far down the list the answer usually is, and **hit@1** is how
//! often it is simply first. Both are reported per variant, and every query's
//! rank is printed, because a mean that moved is worth nothing next to knowing
//! *which* question got better.
//!
//! The queries name no file and no identifier. Anything grep could have found
//! is not a test of this.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use taurus_index::build::walk;
use taurus_index::chunk;
use taurus_provider::Provider;

/// A question, and the path that answers it. Matched as a prefix, so a
/// directory is a fair answer where the work is spread across one.
const QUERIES: &[(&str, &str)] = &[
    (
        "where the conversation transcript is written to disk",
        "crates/taurus-host/src/sessions.rs",
    ),
    (
        "putting back every file a turn changed",
        "crates/taurus-tools/src/checkpoint.rs",
    ),
    (
        "cutting a file into pieces small enough to embed",
        "crates/taurus-index/src/chunk.rs",
    ),
    (
        "how close together two vectors are",
        "crates/taurus-index/src/store.rs",
    ),
    (
        "asking the person to approve something before it runs",
        "crates/taurus-tools/src/permission.rs",
    ),
    (
        "deciding whether a cloned repository may configure the agent",
        "crates/taurus-host/src/trust.rs",
    ),
    (
        "reading the instructions a project already has for other agents",
        "crates/taurus-host/src/instructions.rs",
    ),
    (
        "shortening the older messages when the window fills up",
        "crates/taurus-core/src/agent.rs",
    ),
    (
        "running a program so that it believes it has a terminal",
        "crates/taurus-tools/src/builtin/pty.rs",
    ),
    (
        "describing every column of a tabular file",
        "crates/taurus-data/",
    ),
    ("colouring code as it is typed", "src/lib/"),
    (
        "finding an old conversation by something said in it",
        "crates/taurus-host/src/search.rs",
    ),
    (
        "accounting for what each tool call cost",
        "crates/taurus-host/src/usage.rs",
    ),
    ("talking to a tool server over a pipe", "crates/taurus-mcp/"),
    (
        "turning markdown into what a terminal prints",
        "crates/taurus-cli/src/markdown.rs",
    ),
];

/// How the corpus was cut, and what went into each vector.
#[derive(Clone, Copy, PartialEq)]
enum Variant {
    Lines,
    Structure,
    StructureWithOverlap,
    StructureWithHeading,
}

impl Variant {
    fn name(self) -> &'static str {
        match self {
            Variant::Lines => "lines",
            Variant::Structure => "structure",
            Variant::StructureWithOverlap => "structure+overlap",
            Variant::StructureWithHeading => "structure+heading",
        }
    }
}

/// Every variant, in the order they are reported.
const VARIANTS: [Variant; 4] = [
    Variant::Lines,
    Variant::Structure,
    Variant::StructureWithOverlap,
    Variant::StructureWithHeading,
];

/// One embedded passage of one file.
struct Passage {
    path: String,
    vector: Vec<f32>,
}

/*
 * The chunker as it stood before this change, kept here rather than behind a
 * flag in `chunk.rs`.
 *
 * A production module carrying two ways to do its job so a test can compare
 * them is a module with a setting nobody sets, and the setting outlives the
 * comparison. This is a copy of code that no longer exists, in the one place
 * that has any use for it, and it is allowed to go stale the moment the
 * comparison stops being interesting.
 */
const OLD_CHUNK_LINES: usize = 40;
const OLD_OVERLAP_LINES: usize = 10;
const OLD_MAX_LINE_CHARS: usize = 500;
const OLD_MIN_CHUNK_CHARS: usize = 40;

fn split_by_lines(contents: &str) -> Vec<String> {
    let lines: Vec<&str> = contents.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let stride = OLD_CHUNK_LINES.saturating_sub(OLD_OVERLAP_LINES).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let end = (start + OLD_CHUNK_LINES).min(lines.len());
        let text = lines[start..end]
            .iter()
            .map(|line| {
                if line.chars().count() <= OLD_MAX_LINE_CHARS {
                    line.to_string()
                } else {
                    line.chars().take(OLD_MAX_LINE_CHARS).collect::<String>() + " …"
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.chars().filter(|c| !c.is_whitespace()).count() >= OLD_MIN_CHUNK_CHARS {
            chunks.push(text);
        }
        if end == lines.len() {
            break;
        }
        start += stride;
    }
    chunks
}

fn texts_for(variant: Variant, path: &str, contents: &str) -> Vec<String> {
    match variant {
        Variant::Lines => split_by_lines(contents),
        Variant::Structure => chunk::split(contents)
            .into_iter()
            .map(|piece| piece.text)
            .collect(),
        // Snapped cuts, but every chunk still reaches back over the one before
        // it. The confound this exists to remove: snapping drops the overlap,
        // which drops the passage count by about a seventh, and a corpus with
        // fewer passages in it gives every file fewer chances to be the best
        // match for anything. Without this variant a loss to `lines` cannot be
        // told apart from a loss to *having fewer vectors*.
        Variant::StructureWithOverlap => {
            let lines: Vec<&str> = contents.lines().collect();
            chunk::split(contents)
                .iter()
                .map(|piece| {
                    let from = piece.start_line.saturating_sub(1 + OLD_OVERLAP_LINES);
                    lines[from..piece.end_line].join("\n")
                })
                .collect()
        }
        Variant::StructureWithHeading => chunk::split(contents)
            .iter()
            .map(|piece| piece.passage(path))
            .collect(),
    }
}

#[tokio::main]
async fn main() {
    let root: PathBuf = std::env::args().nth(1).unwrap_or_else(|| ".".into()).into();
    let root = root
        .canonicalize()
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()));
    let model = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "nomic-embed-text".into());
    let base = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
    let provider: Arc<dyn Provider> = Arc::new(taurus_provider_ollama::OllamaProvider::new(base));

    let (files, caveat) = walk(&root);
    if let Some(caveat) = caveat {
        println!("note: {caveat}");
    }
    // Read once and reused across the variants, so all three are scored on
    // exactly the same corpus. Reading per variant would compare the file
    // lists as much as the chunking.
    let sources: Vec<(String, String)> = files
        .into_iter()
        .filter_map(|(relative, absolute)| {
            std::fs::read_to_string(absolute)
                .ok()
                .map(|c| (relative, c))
        })
        .collect();

    println!(
        "workspace: {}\nmodel:     {model}\nfiles:     {}\n",
        root.display(),
        sources.len()
    );

    // Every query embedded once, with the same model, and reused for all three
    // variants: a question is a question regardless of how the corpus was cut.
    let questions: Vec<String> = QUERIES.iter().map(|(q, _)| q.to_string()).collect();
    let query_vectors = provider
        .embed(&model, &questions)
        .await
        .unwrap_or_else(|e| panic!("could not embed the queries: {e}"));

    let mut ranks: HashMap<&str, Vec<Option<usize>>> = HashMap::new();

    for variant in VARIANTS {
        let started = Instant::now();
        let passages = embed_corpus(&sources, variant, &provider, &model).await;
        println!(
            "{:<18} {:>6} passages  {:>8.1?}",
            variant.name(),
            passages.len(),
            started.elapsed()
        );

        let found = query_vectors
            .iter()
            .zip(QUERIES)
            .map(|(vector, (_, expected))| rank_of(&passages, vector, expected))
            .collect();
        ranks.insert(variant.name(), found);
    }

    report(&ranks);
}

async fn embed_corpus(
    sources: &[(String, String)],
    variant: Variant,
    provider: &Arc<dyn Provider>,
    model: &str,
) -> Vec<Passage> {
    let mut pending: Vec<(String, String)> = Vec::new();
    for (path, contents) in sources {
        for text in texts_for(variant, path, contents) {
            pending.push((path.clone(), text));
        }
    }

    let mut passages = Vec::with_capacity(pending.len());
    // The same batch size the real refresh uses, so the timings above are
    // comparable to what indexing a workspace actually costs.
    for batch in pending.chunks(16) {
        let texts: Vec<String> = batch.iter().map(|(_, text)| text.clone()).collect();
        let vectors = provider
            .embed(model, &texts)
            .await
            .unwrap_or_else(|e| panic!("embedding failed: {e}"));
        for ((path, _), vector) in batch.iter().zip(vectors) {
            passages.push(Passage {
                path: path.clone(),
                vector,
            });
        }
    }
    passages
}

/// Where the answering file lands in the results, 1-based, or `None` if it is
/// nowhere.
///
/// Collapsed to one hit per file first, the way `store::search` does: a query
/// that matches a file usually matches three consecutive chunks of it, and
/// three windows of the same function is one answer three times.
fn rank_of(passages: &[Passage], query: &[f32], expected: &str) -> Option<usize> {
    let mut best: HashMap<&str, f32> = HashMap::new();
    for passage in passages {
        let score = taurus_index::store::cosine(&passage.vector, query);
        let entry = best.entry(passage.path.as_str()).or_insert(f32::MIN);
        if score > *entry {
            *entry = score;
        }
    }
    let mut ordered: Vec<(&str, f32)> = best.into_iter().collect();
    ordered.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    ordered
        .iter()
        .position(|(path, _)| path.starts_with(expected))
        .map(|at| at + 1)
}

fn report(ranks: &HashMap<&str, Vec<Option<usize>>>) {
    println!("\nrank of the answering file, per question\n");
    print!("{:<50}", "");
    for variant in VARIANTS {
        print!("{:>19}", variant.name());
    }
    println!();
    for (index, (question, _)) in QUERIES.iter().enumerate() {
        print!("{:<50}", truncate(question, 48));
        for variant in VARIANTS {
            // A miss is distinguished from a bad rank rather than folded into
            // one: a file that is nowhere in the results is a different
            // failure from one that is ninth, and averaging them together
            // hides which happened.
            match ranks[variant.name()][index] {
                Some(rank) => print!("{rank:>19}"),
                None => print!("{:>19}", "—"),
            }
        }
        println!();
    }

    println!("\n{:<20} {:>6} {:>8} {:>8}", "", "MRR", "hit@1", "hit@5");
    for name in VARIANTS.map(|v| v.name()) {
        let found = &ranks[name];
        let mrr: f64 = found
            .iter()
            .map(|rank| rank.map_or(0.0, |r| 1.0 / r as f64))
            .sum::<f64>()
            / found.len() as f64;
        let at = |k: usize| {
            found.iter().filter(|r| r.is_some_and(|r| r <= k)).count() as f64 / found.len() as f64
        };
        println!(
            "{name:<20} {mrr:>6.3} {:>7.0}% {:>7.0}%",
            at(1) * 100.0,
            at(5) * 100.0
        );
    }
    println!(
        "\n{} questions. Higher is better everywhere; MRR is the one to watch,\n\
         because hit@1 moves in whole questions.",
        QUERIES.len()
    );
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars().take(width - 1).collect::<String>() + "…"
}
