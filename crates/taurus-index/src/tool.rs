//! `search_code`: the tool the model calls.
//!
//! Grep answers "where does this string appear". This answers "where is the
//! code that does this", which is the question someone actually has when they
//! arrive at an unfamiliar repository — and the one that costs a small model
//! the most, because the alternative is reading directories until it guesses
//! the right name.
//!
//! That matters more here than it would in a hosted tool. An 8k context cannot
//! afford three wrong `read_file` calls, and every one of them is a page of
//! tokens spent on a file that turned out not to be the answer. Better
//! retrieval is not a nicety at that size; it is what makes the difference
//! between a turn that finishes and a turn that runs out of room.
//!
//! # It refreshes before it searches
//!
//! Not on a timer and not in the background. A model that just wrote a file and
//! then searched for it should find it, and an index refreshed on a schedule
//! would answer from before the edit — which is worse than no index, because
//! the answer looks right. The refresh is cheap after the first one: only files
//! whose length or modification time moved are re-read.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use taurus_provider::Provider;
use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, Tool, ToolContext, ToolError, ToolResult};
use tracing::warn;

use crate::build::refresh;
use crate::store::{search, Index};

pub const SEARCH_CODE_TOOL: &str = "search_code";

/// Results one call may return.
///
/// Small because each carries an excerpt, and because a ranked list past about
/// five stops being a ranking — the sixth result is not evidence, it is the
/// model reading whatever came back.
const MAX_RESULTS: usize = 5;

/// Passages the cosine pass hands to the reranker, when one is configured.
///
/// Six times what survives, which is the shape a two-stage retrieval wants: the
/// first stage's job stops being "pick the answer" and becomes "do not lose
/// it", and it is much better at the second job than the first. Past roughly
/// this many the reranker is reading passages the cosine pass had already
/// ranked below things it got wrong, and the cost is linear in the count
/// because a cross-encoder scores every pair.
///
/// Unused when nothing reranks, and deliberately not the number returned then:
/// thirty excerpts is a context window, not an answer.
const RERANK_CANDIDATES: usize = 30;

/// Lines of an excerpt shown per hit.
///
/// Enough to tell whether the hit is the right place, not enough to be the
/// `read_file` that should follow it. A tool that returned the whole file would
/// spend the context window this exists to save.
const MAX_EXCERPT_LINES: usize = 24;

#[derive(Deserialize, JsonSchema)]
pub struct SearchCodeInput {
    /// What you are looking for, described the way you would say it out loud —
    /// `where sessions are written to disk`, `the retry backoff`. Not a regex
    /// and not a filename.
    pub query: String,
}

/// Finds code by meaning rather than by string.
pub struct SearchCode {
    provider: Arc<dyn Provider>,
    model: String,
    /// Where this workspace's index lives. Rebuilt with the tool whenever the
    /// workspace changes, so the tool never has to ask which one it is on.
    dir: std::path::PathBuf,
    /// Reranking model and the provider serving it. `None` means the cosine
    /// order is the answer.
    rerank: Option<(Arc<dyn Provider>, String)>,
}

impl SearchCode {
    pub fn new(
        provider: Arc<dyn Provider>,
        model: impl Into<String>,
        dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            dir: dir.into(),
            rerank: None,
        }
    }

    /// Adds a second retrieval stage that reorders the shortlist.
    ///
    /// Takes its own provider rather than reusing the embedding one, because
    /// they are usually not the same server. The backend most people embed on
    /// is Ollama, which has no reranking route at all; the ones that do are
    /// reached through the OpenAI-compatible adapter. Reusing the embedding
    /// provider would mean this feature could only ever be configured by
    /// somebody who had already given up on the common setup.
    #[must_use]
    pub fn with_rerank(mut self, provider: Arc<dyn Provider>, model: impl Into<String>) -> Self {
        let model = model.into();
        self.rerank = (!model.trim().is_empty()).then_some((provider, model));
        self
    }

    /// The shortlist, reordered if anything is configured to reorder it.
    ///
    /// A failure here is not a failed search. Reranking is an accuracy stage on
    /// top of a retrieval that already worked, so an unreachable server, a
    /// model that was never pulled, or a backend with no such route leaves the
    /// cosine order standing and logs why. Returning an error instead would
    /// mean an optional stage could take `search_code` away entirely — and it
    /// would do it at the exact moment the model was mid-turn and least able to
    /// recover.
    async fn reranked(&self, query: &str, hits: Vec<crate::store::Hit>) -> Vec<crate::store::Hit> {
        let Some((provider, model)) = &self.rerank else {
            let mut hits = hits;
            hits.truncate(MAX_RESULTS);
            return hits;
        };

        // The reranker scores the excerpt the model is about to read, not the
        // whole file and not the chunk as it was embedded. Judging anything
        // else would rank a passage on evidence the model never sees.
        let documents: Vec<String> = hits.iter().map(|h| h.text.clone()).collect();
        match provider.rerank(model, query, &documents).await {
            Ok(scores) => crate::store::rerank(hits, &scores, MAX_RESULTS),
            Err(e) => {
                warn!(
                    error = %e,
                    model = %model,
                    "reranking failed; falling back to similarity order"
                );
                let mut hits = hits;
                hits.truncate(MAX_RESULTS);
                hits
            }
        }
    }

    /// How many passages the cosine pass should hand on.
    fn candidates(&self) -> usize {
        if self.rerank.is_some() {
            RERANK_CANDIDATES
        } else {
            MAX_RESULTS
        }
    }
}

#[async_trait]
impl Tool for SearchCode {
    fn name(&self) -> &str {
        SEARCH_CODE_TOOL
    }

    fn description(&self) -> &str {
        "Find code by what it does, described in your own words — 'where sessions are written to \
         disk', 'the retry backoff', 'how permissions are checked'. Use it when you do not know \
         what the thing is called, which is exactly when grep cannot help: grep finds a string you \
         already know, and this finds the place you are looking for. Reach for it first on an \
         unfamiliar codebase, then read the files it names. Use grep instead when you know the \
         literal text — a function name, an error message, a config key — because for those grep \
         is exact and this is only close."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<SearchCodeInput>()
    }

    /// Reads files and talks to the embedding backend, which is the same
    /// machine the model is already on. Nothing changes and nothing leaves.
    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        match input.get("query").and_then(|q| q.as_str()) {
            Some(query) => format!("Search the codebase for: {query}"),
            None => "Search the codebase".into(),
        }
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: SearchCodeInput = parse_input(input)?;
        let query = input.query.trim();
        if query.is_empty() {
            return Err(ToolError::InvalidInput(
                "a search needs something to look for".into(),
            ));
        }

        let index = Index::new(&self.dir, &ctx.workspace);

        // Before the search, not on a timer: a model that just wrote a file and
        // then looked for it has to find it.
        ctx.report("indexing the workspace").await;
        let (entries, report) = refresh(
            &index,
            &ctx.workspace,
            &self.provider,
            &self.model,
            &ctx.cancel,
            Some(&Reporting(ctx)),
        )
        .await
        .map_err(ToolError::Failed)?;

        if entries.is_empty() {
            return Ok(format!(
                "Nothing indexed in this workspace, so there is nothing to search. {}",
                report.summary()
            ));
        }

        ctx.report("searching").await;
        let vector = self
            .provider
            .embed(&self.model, std::slice::from_ref(&query.to_string()))
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
            .into_iter()
            .next()
            .ok_or_else(|| ToolError::Failed("the backend returned no embedding".into()))?;

        let hits = search(&entries, &vector, self.candidates(), &ctx.workspace);
        if hits.is_empty() {
            return Ok(format!(
                "No match for '{query}' in {} indexed passages. Try describing it differently, or \
                 use grep if you know the literal text.",
                entries.len()
            ));
        }

        if self.rerank.is_some() {
            ctx.report("ranking the results").await;
        }
        let hits = self.reranked(query, hits).await;

        Ok(render(query, &hits, ctx.workspace.as_path()))
    }
}

/// Carries the refresh's progress into the transcript.
///
/// The first index of a repository takes the better part of a minute, and until
/// this existed the whole of it was one line saying "indexing the workspace"
/// followed by silence — which reads as a hung tool rather than a slow one, to
/// the person watching and to anyone deciding whether to press Stop.
struct Reporting<'a>(&'a ToolContext);

#[async_trait]
impl crate::build::IndexProgress for Reporting<'_> {
    async fn embedding(&self, done: usize, total: usize) {
        self.0
            .report(format!("indexing: {done} of {total} passages"))
            .await;
    }
}

/// The result as the model reads it.
///
/// Paths and line numbers first on each hit, because the next call is almost
/// always `read_file` on one of them and the model should not have to parse an
/// excerpt to find out where it came from.
fn render(query: &str, hits: &[crate::store::Hit], _workspace: &Path) -> String {
    let mut out = format!(
        "{} match{} for '{query}', best first.\n",
        hits.len(),
        if hits.len() == 1 { "" } else { "es" }
    );

    for hit in hits {
        // The scale is named per hit rather than once at the top, because a
        // reranked set can hold both: a server that honored a smaller `top_n`
        // than it was asked for leaves the tail carrying cosine numbers, and
        // one heading calling all of them "relevance" would be wrong about
        // exactly the hits the reranker declined to judge.
        out.push_str(&format!(
            "\n{}:{}-{} ({} {:.2})\n",
            hit.path,
            hit.start_line,
            hit.end_line,
            hit.ranking.label(),
            hit.score
        ));
        let lines: Vec<&str> = hit.text.lines().take(MAX_EXCERPT_LINES).collect();
        for (n, line) in lines.iter().enumerate() {
            out.push_str(&format!("{:>5}\t{line}\n", hit.start_line + n));
        }
        if hit.text.lines().count() > MAX_EXCERPT_LINES {
            out.push_str("      …\n");
        }
    }

    out.push_str(
        "\nThese are the closest passages, not necessarily the answer. Read the files before \
         acting on them.",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use taurus_provider::{Capabilities, ChatRequest, ModelInfo, StopReason, StreamEvent};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use crate::test_support::allowing_ctx;

    /// Embeds a text as a bag of its lowercase words, projected onto a fixed
    /// vocabulary. Crude, deterministic, and enough that a query about "retry
    /// backoff" scores highest against the chunk containing those words —
    /// which is the behaviour the tool is asserting, not the model's quality.
    struct Bagged;

    const VOCAB: &[&str] = &[
        "retry",
        "backoff",
        "session",
        "transcript",
        "disk",
        "permission",
        "checked",
        "sleep",
    ];

    #[async_trait]
    impl Provider for Bagged {
        fn id(&self) -> &str {
            "bagged"
        }
        async fn models(&self) -> taurus_provider::Result<Vec<ModelInfo>> {
            Ok(Vec::new())
        }
        async fn capabilities(&self, _: &str) -> taurus_provider::Result<Capabilities> {
            Ok(Capabilities::default())
        }
        async fn stream(
            &self,
            _: ChatRequest,
            _: mpsc::Sender<StreamEvent>,
            _: CancellationToken,
        ) -> taurus_provider::Result<StopReason> {
            Ok(StopReason::EndTurn)
        }
        async fn embed(
            &self,
            _: &str,
            inputs: &[String],
        ) -> taurus_provider::Result<Vec<Vec<f32>>> {
            Ok(inputs
                .iter()
                .map(|text| {
                    let lower = text.to_lowercase();
                    VOCAB
                        .iter()
                        .map(|word| lower.matches(word).count() as f32)
                        .collect()
                })
                .collect())
        }
    }

    fn tool(dir: &Path) -> SearchCode {
        SearchCode::new(Arc::new(Bagged), "test-embed", dir)
    }

    /// A reranker that promotes whichever passage mentions `favors`, and scores
    /// on llama.cpp's scale rather than a normalized one — negatives included,
    /// because that is the case a threshold somewhere would silently break.
    struct Promoting {
        favors: &'static str,
        /// Refuses instead of scoring, standing in for an unreachable server or
        /// a model that was never pulled.
        broken: bool,
    }

    #[async_trait]
    impl Provider for Promoting {
        fn id(&self) -> &str {
            "promoting"
        }
        async fn models(&self) -> taurus_provider::Result<Vec<ModelInfo>> {
            Ok(Vec::new())
        }
        async fn capabilities(&self, _: &str) -> taurus_provider::Result<Capabilities> {
            Ok(Capabilities::default())
        }
        async fn stream(
            &self,
            _: ChatRequest,
            _: mpsc::Sender<StreamEvent>,
            _: CancellationToken,
        ) -> taurus_provider::Result<StopReason> {
            Ok(StopReason::EndTurn)
        }
        async fn rerank(
            &self,
            _: &str,
            _: &str,
            documents: &[String],
        ) -> taurus_provider::Result<Vec<taurus_provider::RerankScore>> {
            if self.broken {
                return Err(taurus_provider::ProviderError::Protocol(
                    "no reranking model is loaded".into(),
                ));
            }
            Ok(documents
                .iter()
                .enumerate()
                .map(|(index, text)| taurus_provider::RerankScore {
                    index,
                    score: if text.contains(self.favors) {
                        2.5
                    } else {
                        -6.0
                    },
                })
                .collect())
        }
    }

    /// Two passages, each about a different thing, both indexed.
    async fn two_passage_workspace(ctx: &ToolContext) {
        write(
            &ctx.workspace,
            "src/net.rs",
            &format!(
                "{}// the retry backoff doubles on each attempt\nfn retry() {{ backoff(); retry(); }}\n{}",
                filler(3),
                filler(3)
            ),
        );
        write(
            &ctx.workspace,
            "src/store.rs",
            &format!(
                "{}// a session transcript is written to disk\nfn session() {{ transcript(); disk(); }}\n{}",
                filler(3),
                filler(3)
            ),
        );
    }

    #[tokio::test]
    async fn a_reranker_can_overturn_the_similarity_order() {
        let (ctx, _dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        two_passage_workspace(&ctx).await;

        // The embedding pass puts net.rs first for this query. The reranker
        // disagrees, and the reranker is the one that decides.
        let plain = tool(index_dir.path())
            .execute(serde_json::json!({ "query": "retry backoff" }), &ctx)
            .await
            .expect("search succeeds");
        assert!(
            plain.find("src/net.rs") < plain.find("src/store.rs"),
            "similarity should favour net.rs to begin with:\n{plain}"
        );

        let reranked = tool(index_dir.path())
            .with_rerank(
                Arc::new(Promoting {
                    favors: "transcript",
                    broken: false,
                }),
                "test-rerank",
            )
            .execute(serde_json::json!({ "query": "retry backoff" }), &ctx)
            .await
            .expect("search succeeds");

        assert!(
            reranked.find("src/store.rs") < reranked.find("src/net.rs"),
            "the reranker should have promoted store.rs:\n{reranked}"
        );
    }

    #[tokio::test]
    async fn a_reranked_result_says_relevance_rather_than_similarity() {
        // The scale changed, so the word beside the number has to change with
        // it — a reranker's score is not a cosine and is routinely negative.
        let (ctx, _dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        two_passage_workspace(&ctx).await;

        let out = tool(index_dir.path())
            .with_rerank(
                Arc::new(Promoting {
                    favors: "transcript",
                    broken: false,
                }),
                "test-rerank",
            )
            .execute(serde_json::json!({ "query": "retry backoff" }), &ctx)
            .await
            .expect("search succeeds");

        assert!(out.contains("(relevance "), "{out}");
        assert!(!out.contains("(similarity "), "{out}");
        assert!(
            out.contains("-6.00"),
            "a negative score is a ranking, not a result to hide:\n{out}"
        );
    }

    #[tokio::test]
    async fn a_reranker_that_fails_leaves_the_search_working() {
        // The whole reason this stage is allowed to exist. An accuracy pass on
        // top of a retrieval that already worked must never be able to take the
        // retrieval away — least of all mid-turn, which is when it would.
        let (ctx, _dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        two_passage_workspace(&ctx).await;

        let out = tool(index_dir.path())
            .with_rerank(
                Arc::new(Promoting {
                    favors: "transcript",
                    broken: true,
                }),
                "test-rerank",
            )
            .execute(serde_json::json!({ "query": "retry backoff" }), &ctx)
            .await
            .expect("a failed rerank is not a failed search");

        assert!(out.contains("src/net.rs"), "{out}");
        assert!(
            out.contains("(similarity "),
            "the fallback is the similarity order, labelled as such:\n{out}"
        );
    }

    #[tokio::test]
    async fn naming_no_rerank_model_leaves_the_tool_exactly_as_it_was() {
        // Empty is how the setting is turned off, and it has to mean off rather
        // than "rerank with a model called nothing".
        let (ctx, _dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        two_passage_workspace(&ctx).await;

        let out = tool(index_dir.path())
            .with_rerank(
                Arc::new(Promoting {
                    favors: "transcript",
                    broken: true,
                }),
                "   ",
            )
            .execute(serde_json::json!({ "query": "retry backoff" }), &ctx)
            .await
            .expect("search succeeds");

        assert!(out.contains("(similarity "), "{out}");
        assert!(out.find("src/net.rs") < out.find("src/store.rs"), "{out}");
    }

    fn write(root: &Path, name: &str, body: &str) {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    /// Padding, so a chunk clears the minimum-substance bar without adding any
    /// vocabulary words.
    fn filler(n: usize) -> String {
        (0..n)
            .map(|i| format!("// ordinary line number {i} with nothing notable in it\n"))
            .collect()
    }

    #[tokio::test]
    async fn it_finds_the_passage_that_is_about_the_query() {
        let (ctx, dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();

        write(
            &ctx.workspace,
            "src/net.rs",
            &format!(
                "{}// the retry backoff doubles on each attempt\nfn retry() {{ backoff(); retry(); }}\n{}",
                filler(3),
                filler(3)
            ),
        );
        write(
            &ctx.workspace,
            "src/store.rs",
            &format!(
                "{}// a session transcript is written to disk\nfn session() {{ transcript(); disk(); }}\n{}",
                filler(3),
                filler(3)
            ),
        );

        let out = tool(index_dir.path())
            .execute(serde_json::json!({ "query": "retry backoff" }), &ctx)
            .await
            .expect("a search");

        let net = out.find("src/net.rs").expect("net.rs in the results");
        let store = out.find("src/store.rs");
        assert!(
            store.is_none_or(|store| net < store),
            "the wrong file ranked first:\n{out}"
        );
        drop(dir);
    }

    #[tokio::test]
    async fn a_result_names_the_file_and_the_lines_before_the_excerpt() {
        // The next call is almost always `read_file` on one of these, and the
        // model should not have to parse an excerpt to find out where it is.
        let (ctx, dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        write(
            &ctx.workspace,
            "src/net.rs",
            &format!("{}fn retry() {{ backoff(); }}\n{}", filler(3), filler(3)),
        );

        let out = tool(index_dir.path())
            .execute(serde_json::json!({ "query": "retry backoff" }), &ctx)
            .await
            .unwrap();

        assert!(out.contains("src/net.rs:1-"), "{out}");
        assert!(out.contains("similarity"), "{out}");
        drop(dir);
    }

    #[tokio::test]
    async fn an_empty_workspace_says_so_rather_than_returning_nothing() {
        // A bare "no results" reads as "this code does not exist", which is a
        // different and much more misleading claim.
        let (ctx, dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();

        let out = tool(index_dir.path())
            .execute(serde_json::json!({ "query": "anything" }), &ctx)
            .await
            .unwrap();
        assert!(out.contains("Nothing indexed"), "{out}");
        drop(dir);
    }

    #[tokio::test]
    async fn a_file_written_a_moment_ago_is_searchable_immediately() {
        // The reason the refresh is not on a timer: a model that just wrote a
        // file and then looked for it has to find it. An index refreshed on a
        // schedule answers from before the edit, which is worse than no index
        // because the answer looks right.
        let (ctx, dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        let tool = tool(index_dir.path());

        write(&ctx.workspace, "src/old.rs", &filler(8));
        tool.execute(serde_json::json!({ "query": "session" }), &ctx)
            .await
            .unwrap();

        write(
            &ctx.workspace,
            "src/new.rs",
            &format!(
                "{}fn session() {{ transcript(); disk(); }}\n{}",
                filler(3),
                filler(3)
            ),
        );
        let out = tool
            .execute(
                serde_json::json!({ "query": "session transcript disk" }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(out.contains("src/new.rs"), "{out}");
        drop(dir);
    }

    #[tokio::test]
    async fn an_empty_query_is_refused_before_anything_is_indexed() {
        let (ctx, dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        let error = tool(index_dir.path())
            .execute(serde_json::json!({ "query": "   " }), &ctx)
            .await
            .expect_err("an empty query");
        assert!(error.to_string().contains("something to look for"));
        drop(dir);
    }

    #[tokio::test]
    async fn the_result_says_the_hits_are_leads_rather_than_answers() {
        // A ranked list reads as authoritative, and a small model will act on
        // the first hit without opening it unless told otherwise.
        let (ctx, dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        write(
            &ctx.workspace,
            "src/net.rs",
            &format!("{}fn retry() {{ backoff(); }}\n{}", filler(3), filler(3)),
        );

        let out = tool(index_dir.path())
            .execute(serde_json::json!({ "query": "retry" }), &ctx)
            .await
            .unwrap();
        assert!(out.contains("Read the files before acting"), "{out}");
        drop(dir);
    }
}
