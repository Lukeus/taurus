//! Index a real workspace and ask it real questions.
//!
//! Everything the unit tests cover uses a fake embedder, because what they
//! assert is the walk, the staleness rule, and the ranking — none of which
//! needs a model. What none of them can show is whether the retrieval is any
//! *good*, which is the only question that matters for a search tool.
//!
//! ```sh
//! ollama pull nomic-embed-text
//! cargo run -p taurus-index --example probe -- . nomic-embed-text
//! ```
//!
//! It prints what the first pass cost, proves the second pass costs almost
//! nothing, and then runs a handful of questions whose answers a reader of this
//! repository can check by eye. Run it on something large before changing the
//! caps in `store.rs`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use taurus_index::{refresh, search, Index};
use taurus_provider::Provider;
use tokio_util::sync::CancellationToken;

/// Questions phrased the way someone actually asks them — no filenames, no
/// identifiers, nothing that grep could have found.
const QUESTIONS: &[&str] = &[
    "where the conversation transcript is written to disk",
    "how a turn can be undone",
    "the retry backoff when a request fails",
    "deciding whether a model supports tool calling",
];

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
    // A temporary index, so a probe never disturbs the one the app is using.
    let dir = tempfile::TempDir::new().expect("a temp dir");
    let index = Index::new(dir.path(), &root);
    let cancel = CancellationToken::new();

    println!("workspace: {}\nmodel:     {model}\n", root.display());

    let started = Instant::now();
    let (entries, report) = refresh(&index, &root, &provider, &model, &cancel, None)
        .await
        .unwrap_or_else(|e| panic!("indexing failed: {e}"));
    let first = started.elapsed();
    println!("first pass:  {first:>8.1?}  {}", report.summary());

    // The property the whole design turns on. If this is not near-instant, the
    // staleness rule is broken and every search pays the full cost.
    let started = Instant::now();
    let (_, again) = refresh(&index, &root, &provider, &model, &cancel, None)
        .await
        .expect("the second pass");
    println!(
        "second pass: {:>8.1?}  {}",
        started.elapsed(),
        again.summary()
    );
    assert_eq!(again.embedded, 0, "the staleness rule did not hold");

    let bytes = std::fs::metadata(index.path())
        .map(|m| m.len())
        .unwrap_or(0);
    println!(
        "\n{} passages, {:.1} MB on disk\n",
        entries.len(),
        bytes as f64 / (1024.0 * 1024.0)
    );

    for question in QUESTIONS {
        let started = Instant::now();
        let vector = provider
            .embed(&model, std::slice::from_ref(&question.to_string()))
            .await
            .unwrap_or_else(|e| panic!("could not embed the query: {e}"))
            .remove(0);
        let hits = search(&entries, &vector, 3, &root);
        let took = started.elapsed();

        println!("\"{question}\"  ({took:.1?})");
        for hit in &hits {
            println!(
                "   {:.3}  {}:{}-{}",
                hit.score, hit.path, hit.start_line, hit.end_line
            );
        }
        println!();
    }
}
