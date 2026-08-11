//! Runs one real search and fetches the first result, through the registry.
//!
//! `cargo run -p taurus-web --example probe -- <path-to-search.json> "<query>"`
//!
//! The unit tests cover parsing against recorded response shapes, which is the
//! part that can be pinned. What they cannot check is whether a live backend
//! still answers in that shape, whether the key in your environment works, or
//! whether a real page survives conversion to something a model can read. That
//! is what this is for.

use std::sync::Arc;

use taurus_tools::{AllowAll, PermissionEngine, ToolContext, ToolRegistry};
use taurus_web::{FetchUrl, SearchFile, WebSearch};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: probe <search.json> [query]");
    let query = args.next().unwrap_or_else(|| "rust async book".to_string());

    let file: SearchFile = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let (backend, problems) = taurus_web::merge(vec![file]);
    for problem in &problems {
        println!("problem: {problem}");
    }
    let Some(backend) = backend else {
        println!("no usable backend; set `backend` in {path}");
        return Ok(());
    };
    println!("backend {} ({:?})\n", backend.id, backend.kind);

    // Registered exactly as the host does, so this exercises the permission
    // gate and the registry rather than the tools in isolation.
    let mut registry = ToolRegistry::with_builtins();
    registry.register(Arc::new(WebSearch::new(backend)));
    registry.register(Arc::new(FetchUrl));

    let workspace = std::env::current_dir()?.canonicalize()?;
    let ctx = ToolContext::new(
        workspace.clone(),
        Arc::new(PermissionEngine::new(
            &workspace,
            workspace.join(".taurus"),
            Box::new(AllowAll),
        )),
        CancellationToken::new(),
    );

    let results = registry
        .execute("web_search", serde_json::json!({ "query": query }), &ctx)
        .await?;
    println!("{results}\n");

    // Follow the first URL the search returned, which is the handoff the two
    // tools exist to make.
    let Some(url) = results
        .split_whitespace()
        .find(|word| word.starts_with("https://") || word.starts_with("http://"))
    else {
        println!("no URL in the results to follow");
        return Ok(());
    };

    println!("fetching {url}…\n");
    let page = registry
        .execute(
            "fetch_url",
            serde_json::json!({ "url": url, "max_chars": 2000 }),
            &ctx,
        )
        .await?;
    println!("{page}");

    Ok(())
}
