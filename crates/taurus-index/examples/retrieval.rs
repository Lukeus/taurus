//! How well does the index actually answer a question?
//!
//! ```sh
//! ollama pull nomic-embed-text
//! cargo run -p taurus-index --example retrieval -- . nomic-embed-text
//! ```
//!
//! With a reranker, both orders are scored in the same run, side by side:
//!
//! ```sh
//! llama-server --reranking --port 8012 -b 8192 -ub 8192 -hf gpustack/bge-reranker-v2-m3-GGUF
//! RERANK_URL=http://localhost:8012 RERANK_MODEL=bge-reranker-v2-m3 \
//!     cargo run -p taurus-index --example retrieval -- . nomic-embed-text
//! ```
//!
//! `index-probe` next door prints hits for a reader to judge by eye, which is the
//! right check for "is this any good at all" and no check at all for "is this
//! better than what it replaced". This answers the second question with a
//! number: run it, change something, run it again.
//!
//! It exists because this repository had already shipped one retrieval change
//! without that check: `rerank_model` is empty by default because the plan that
//! added reranking gated it on beating cosine. Setting `RERANK_URL` runs that
//! gate. The reranker is handed what `search_code` hands it — the best passage
//! of each of the top [`RERANK_CANDIDATES`] files by cosine — so the column it
//! adds is the order a search with reranking switched on would return.
//!
//! # What it measures
//!
//! Fifteen questions phrased the way somebody asks them, each with the file
//! that actually answers it. For each, the rank of that file in the results —
//! so **MRR** is how far down the list the answer usually is, and **hit@1** is
//! how often it is simply first. Every question's rank is printed as well as
//! the means, because a mean that moved is worth nothing next to knowing
//! *which* question got better.
//!
//! The questions name no file and no identifier. Anything grep could have found
//! is not a test of this.
//!
//! # What it is not
//!
//! Fifteen questions, one repository, one embedding model. That is enough to
//! catch a change that makes retrieval worse — the numbers are deterministic,
//! so a difference between two runs is a real difference — and it is not enough
//! to conclude much about a change that leaves them alone. It says nothing at
//! all about a workspace in another language.
//!
//! Answers are matched as a path prefix, so a question whose work is spread
//! across a directory can name the directory. Where the answer is genuinely in
//! two files, this scores the one a reader would open first, which is a
//! judgement and is why the questions are here to be argued with.
//!
//! # Compare within a run, not across two
//!
//! The corpus is the working tree, so the score moves when the tree does — and
//! it moves by more than you would guess. Editing a doc page between two runs
//! was measured shifting MRR by 0.03, which is the size of the differences this
//! is for detecting.
//!
//! So a comparison has to score both things against the same corpus. The
//! cleanest way is what the structure-chunking experiment did: read the files
//! once, chunk them every way under test in one process, and report the ranks
//! side by side. Stashing a change, running this, unstashing and running it
//! again gives two numbers that differ partly because of the change and partly
//! because the tree was not the same — including because the change itself is
//! in it.

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

/// One embedded passage of one file.
struct Passage {
    path: String,
    text: String,
    vector: Vec<f32>,
}

/// How many files the cosine pass hands the reranker. The same number
/// `search_code` uses, so the reranked column is what that tool would return.
const RERANK_CANDIDATES: usize = 30;

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

    let started = Instant::now();
    let passages = embed_corpus(&sources, &provider, &model).await;
    println!(
        "{} passages, embedded in {:.1?}\n",
        passages.len(),
        started.elapsed()
    );

    let orders: Vec<Vec<(&Passage, f32)>> = query_vectors
        .iter()
        .map(|vector| cosine_order(&passages, vector))
        .collect();
    let mut columns = vec![(
        "cosine".to_string(),
        orders
            .iter()
            .zip(QUERIES)
            .map(|(order, (_, expected))| rank_in(order.iter().map(|(p, _)| *p), expected))
            .collect::<Vec<_>>(),
    )];

    if let Ok(url) = std::env::var("RERANK_URL") {
        let model = std::env::var("RERANK_MODEL").unwrap_or_else(|_| "reranker".into());
        let reranker = taurus_provider_openai::OpenAiProvider::new(
            "rerank",
            url,
            None,
            taurus_provider_openai::OpenAiCapabilities::default(),
        );
        let started = Instant::now();
        let mut ranks = Vec::with_capacity(QUERIES.len());
        for (order, (question, expected)) in orders.iter().zip(QUERIES) {
            let candidates: Vec<&Passage> = order
                .iter()
                .take(RERANK_CANDIDATES)
                .map(|(p, _)| *p)
                .collect();
            let documents: Vec<String> = candidates.iter().map(|p| p.text.clone()).collect();
            let scores = reranker
                .rerank(&model, question, &documents)
                .await
                .unwrap_or_else(|e| panic!("reranking failed: {e}"));
            // Ordinal, the way `store::rerank` reads them: only the order of
            // the scores means anything. A candidate the server didn't score
            // keeps its cosine place after every one it did.
            let mut scored: Vec<(usize, f32)> = scores.iter().map(|s| (s.index, s.score)).collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            let mut reranked: Vec<&Passage> = Vec::with_capacity(order.len());
            for (index, _) in &scored {
                if let Some(p) = candidates.get(*index) {
                    if !reranked.iter().any(|r| std::ptr::eq(*r, *p)) {
                        reranked.push(p);
                    }
                }
            }
            for p in &candidates {
                if !reranked.iter().any(|r| std::ptr::eq(*r, *p)) {
                    reranked.push(p);
                }
            }
            // Below the candidates, the cosine order goes on unchanged, so a
            // miss is a miss in both columns rather than only in this one.
            reranked.extend(order.iter().skip(RERANK_CANDIDATES).map(|(p, _)| *p));
            ranks.push(rank_in(reranked.into_iter(), expected));
        }
        println!("reranked with {model} in {:.1?}\n", started.elapsed());
        columns.push(("rerank".to_string(), ranks));
    }

    report(&columns);
}

async fn embed_corpus(
    sources: &[(String, String)],
    provider: &Arc<dyn Provider>,
    model: &str,
) -> Vec<Passage> {
    let mut pending: Vec<(String, String)> = Vec::new();
    for (path, contents) in sources {
        for piece in chunk::split(contents) {
            pending.push((path.clone(), piece.text));
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
        for ((path, text), vector) in batch.iter().zip(vectors) {
            passages.push(Passage {
                path: path.clone(),
                text: text.clone(),
                vector,
            });
        }
    }
    passages
}

/// Every file's best passage for this query, best first.
///
/// Collapsed to one hit per file first, the way `store::search` does: a query
/// that matches a file usually matches three consecutive chunks of it, and
/// three windows of the same function is one answer three times.
fn cosine_order<'a>(passages: &'a [Passage], query: &[f32]) -> Vec<(&'a Passage, f32)> {
    let mut best: HashMap<&str, (&Passage, f32)> = HashMap::new();
    for passage in passages {
        let score = taurus_index::store::cosine(&passage.vector, query);
        let entry = best
            .entry(passage.path.as_str())
            .or_insert((passage, f32::MIN));
        if score > entry.1 {
            *entry = (passage, score);
        }
    }
    let mut ordered: Vec<(&Passage, f32)> = best.into_values().collect();
    ordered.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.path.cmp(&b.0.path)));
    ordered
}

/// Where the answering file lands in an order, 1-based, or `None` if it is
/// nowhere.
fn rank_in<'a>(order: impl Iterator<Item = &'a Passage>, expected: &str) -> Option<usize> {
    order
        .enumerate()
        .find(|(_, p)| p.path.starts_with(expected))
        .map(|(at, _)| at + 1)
}

fn report(columns: &[(String, Vec<Option<usize>>)]) {
    println!("rank of the answering file, per question\n");
    let header: String = columns
        .iter()
        .map(|(name, _)| format!("{name:>7}"))
        .collect();
    println!("{header}  question");
    for (index, (question, expected)) in QUERIES.iter().enumerate() {
        // A miss is distinguished from a bad rank rather than folded into one:
        // a file that is nowhere in the results is a different failure from one
        // that is ninth, and averaging them together hides which happened.
        let ranks: String = columns
            .iter()
            .map(|(_, ranks)| match ranks[index] {
                Some(rank) => format!("{rank:>7}"),
                None => format!("{:>7}", "—"),
            })
            .collect();
        println!("{ranks}  {:<52}  {expected}", truncate(question, 50));
    }

    println!();
    for (name, ranks) in columns {
        let mrr: f64 = ranks
            .iter()
            .map(|rank| rank.map_or(0.0, |r| 1.0 / r as f64))
            .sum::<f64>()
            / ranks.len() as f64;
        let at = |k: usize| {
            ranks.iter().filter(|r| r.is_some_and(|r| r <= k)).count() as f64 / ranks.len() as f64
        };
        println!(
            "{name:>7}  MRR {mrr:.3}   hit@1 {:.0}%   hit@5 {:.0}%   over {} questions",
            at(1) * 100.0,
            at(5) * 100.0,
            QUERIES.len()
        );
    }
    println!(
        "\nHigher is better everywhere. MRR is the one to watch: hit@1 moves in\n\
         whole questions, so it is coarse at this sample size."
    );
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars().take(width - 1).collect::<String>() + "…"
}
