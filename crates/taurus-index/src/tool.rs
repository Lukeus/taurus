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
//!
//! # But not for minutes
//!
//! A first index on the built-in engine can take many minutes. The search
//! waits [`WAIT_FOR_INDEX`] for it, then answers from what's embedded so far
//! and says so, leaving the refresh running. Files go in most recently
//! changed first, so the file just written is among those searched, and the
//! promise above holds for it.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use taurus_provider::Provider;
use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, OutputBudget, Tool, ToolContext, ToolError, ToolResult};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::build::refresh;
use crate::store::{search, Index};

pub const SEARCH_CODE_TOOL: &str = "search_code";

/// Results one call may return, as a share of what has to hold them.
///
/// Small because each carries an excerpt, and because a ranked list past about
/// five stops being a ranking — the sixth result is not evidence, it is the
/// model reading whatever came back. That is an argument about ranking and it
/// does not scale without limit, which is what the ceiling here says: a wider
/// window buys a few more excerpts, not a different tool.
///
/// At [`OutputBudget::ANCHOR_WINDOW`] the share is the five this was, against
/// an excerpt costing [`MAX_EXCERPT_LINES`] lines of about forty bytes.
const RESULT_SHARE: f32 = 0.006;
const EXCERPT_BYTES: usize = MAX_EXCERPT_LINES * 40;
/// Below three a ranked list is not a list.
const MIN_RESULTS: usize = 3;
/// Past this the answer is a reading assignment rather than a ranking.
const MAX_RESULTS: usize = 24;

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

/// How long a search waits for the index to be brought up to date before
/// answering from what's embedded so far.
///
/// Long enough for every refresh but a large one: a few changed files take
/// well under a second on any engine. Short enough that a first index, which
/// can be many minutes on the built-in engine, doesn't hold the turn: the
/// model gets an answer from the files it reached, newest first, and the
/// refresh carries on behind it.
const WAIT_FOR_INDEX: Duration = Duration::from_secs(10);

/// Passages past which a refresh plainly won't finish inside
/// [`WAIT_FOR_INDEX`], so the search stops waiting as soon as the scan says
/// so rather than sitting out the ten seconds first. The built-in engine on
/// a CPU embeds about ten a second (measured on an M4 Max, slower elsewhere)
/// and Ollama on a GPU about thirty-five, so this is several times what the
/// first manages in ten seconds and about what the second does.
const LONG_REFRESH: usize = 256;

/// How long a search past [`WAIT_FOR_INDEX`] gives the refresh to write down
/// what it has finished. A batch on the CPU takes a few seconds.
const FLUSH_WAIT: Duration = Duration::from_secs(5);

/// What the refresh before a search left to search.
struct Refreshed {
    entries: Vec<crate::Entry>,
    /// The refresh's own line, for when nothing is indexed at all.
    summary: String,
    /// Whether the index is still being built, so a result has to say what
    /// it might be missing. `summary` says how much.
    partial: bool,
}

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
    /// The refresh already running for this workspace, if the caller keeps
    /// track of one. `None` where nothing else could be indexing — a test, an
    /// example, a tool built on its own.
    indexing: Option<Arc<crate::Indexing>>,
    /// How long a search waits for a refresh before answering from what's
    /// embedded so far. See [`WAIT_FOR_INDEX`].
    wait: Duration,
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
            indexing: None,
            wait: WAIT_FOR_INDEX,
        }
    }

    /// Joins the workspace's one refresh, so a search stops the warm-up rather
    /// than embedding the same passages beside it.
    ///
    /// Taking over rather than waiting: the search has a turn behind it, and
    /// what it interrupts has already written down everything it embedded, so
    /// the refresh that follows carries on from there. See
    /// [`crate::inflight`].
    #[must_use]
    pub fn with_indexing(mut self, indexing: Arc<crate::Indexing>) -> Self {
        self.indexing = Some(indexing);
        self
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
    async fn reranked(
        &self,
        query: &str,
        hits: Vec<crate::store::Hit>,
        keep: usize,
    ) -> Vec<crate::store::Hit> {
        let Some((provider, model)) = &self.rerank else {
            let mut hits = hits;
            hits.truncate(keep);
            return hits;
        };

        // The reranker scores the excerpt the model is about to read, not the
        // whole file and not the chunk as it was embedded. Judging anything
        // else would rank a passage on evidence the model never sees.
        let documents: Vec<String> = hits.iter().map(|h| h.text.clone()).collect();
        match provider.rerank(model, query, &documents).await {
            Ok(scores) => crate::store::rerank(hits, &scores, keep),
            Err(e) => {
                warn!(
                    error = %e,
                    model = %model,
                    "reranking failed; falling back to similarity order"
                );
                let mut hits = hits;
                hits.truncate(keep);
                hits
            }
        }
    }

    /// Refreshes, waiting however long it takes. For a tool with no
    /// [`crate::Indexing`] to leave a refresh running under.
    async fn refresh_to_the_end(
        &self,
        index: &Index,
        ctx: &ToolContext,
    ) -> Result<Refreshed, ToolError> {
        let stop = ctx.cancel.child_token();
        let (entries, report) = refresh(
            index,
            &ctx.workspace,
            &self.provider,
            &self.model,
            &stop,
            Some(&Reporting(ctx)),
        )
        .await
        .map_err(ToolError::Failed)?;
        Ok(Refreshed {
            entries,
            summary: report.summary(),
            partial: false,
        })
    }

    /// Refreshes, waiting up to [`SearchCode::wait`], and leaves a longer
    /// refresh running.
    ///
    /// Takes over whatever was refreshing, as it always has: the scan that
    /// starts again is what finds a file written since, and what the old
    /// refresh had embedded is already written down. The refresh runs as its
    /// own task, registered with `flight`, so it can outlive this call. A
    /// refresh that finishes in time is the whole answer, which is every
    /// refresh except a first index or a large catch-up. One that won't is
    /// asked to write down what it has finished, and the search answers from
    /// that and says it's partial. The scan put the most recently changed
    /// files first, so a file the model just wrote is among them.
    ///
    /// Except when a long refresh is already running. Then this joins it
    /// rather than restarting it: only whole files are written down, so a
    /// restart throws away the file in progress, and on a first index whose
    /// newest files are large, a search every turn would keep it from ever
    /// getting past them.
    ///
    /// Independent of the turn: Stop ends the wait, not the refresh, the same
    /// as the warm-up a turn starts. Leaving the workspace stops it.
    async fn refresh_with_deadline(
        &self,
        index: &Index,
        flight: &Arc<crate::Indexing>,
        ctx: &ToolContext,
    ) -> Result<Refreshed, ToolError> {
        if let Some(live) = flight
            .running()
            .filter(|live| live.planned().is_some_and(|n| n > LONG_REFRESH))
        {
            return self.answer_from_part(index, &live, true).await;
        }

        let (ticket, stop, live) = flight.take_over_live(&CancellationToken::new());
        let mut task = {
            let index = index.clone();
            let workspace = ctx.workspace.clone();
            let provider = Arc::clone(&self.provider);
            let model = self.model.clone();
            let flight = Arc::clone(flight);
            let live = Arc::clone(&live);
            let stop = stop.clone();
            tokio::spawn(async move {
                let refreshed =
                    refresh(&index, &workspace, &provider, &model, &stop, Some(&*live)).await;
                flight.finished(ticket);
                refreshed
            })
        };

        let deadline = tokio::time::sleep(self.wait);
        tokio::pin!(deadline);
        let mut progress = live.progress.subscribe();
        let mut planned = live.planned.subscribe();
        loop {
            tokio::select! {
                biased;
                _ = ctx.cancel.cancelled() => {
                    return Err(ToolError::Failed(
                        "the search was stopped; indexing carries on in the background".into(),
                    ));
                }
                joined = &mut task => {
                    let refreshed = joined.map_err(|e| {
                        ToolError::Failed(format!("indexing failed to run: {e}"))
                    })?;
                    return match refreshed {
                        Ok((entries, report)) => Ok(Refreshed {
                            entries,
                            summary: report.summary(),
                            partial: false,
                        }),
                        // Another refresh took this one over (a second search
                        // in the same round, or Build index) and is finishing
                        // the work. What this one embedded is written down, so
                        // search the index as it stands rather than fail a
                        // question the index can mostly answer.
                        Err(_) if stop.is_cancelled() => {
                            let entries = self.load(index).await?;
                            Ok(Refreshed {
                                entries,
                                summary: "Another refresh of the index took over from this one \
                                          and is still running, so files it hasn't reached yet \
                                          won't show up here; grep searches everything."
                                    .to_string(),
                                partial: true,
                            })
                        }
                        Err(e) => Err(ToolError::Failed(e)),
                    };
                }
                Ok(()) = progress.changed() => {
                    let (done, total) = *progress.borrow_and_update();
                    ctx.report(format!("indexing: {done} of {total} passages")).await;
                }
                Ok(()) = planned.changed() => {
                    if planned.borrow_and_update().1 > LONG_REFRESH {
                        break;
                    }
                }
                _ = &mut deadline => break,
            }
        }
        self.answer_from_part(index, &live, false).await
    }

    /// Searches what a running refresh has finished, and says so.
    ///
    /// Asks it to write down what it has first, and gives the batches in
    /// flight a moment to land. `joined` is for a refresh this search found
    /// running rather than started, whose scan came before any file written
    /// since.
    async fn answer_from_part(
        &self,
        index: &Index,
        live: &crate::Live,
        joined: bool,
    ) -> Result<Refreshed, ToolError> {
        live.request_flush();
        let _ = tokio::time::timeout(FLUSH_WAIT, live.flushed.notified()).await;
        let entries = self.load(index).await?;
        // Counted from what was searched, not from the progress reports,
        // which come in steps and lag behind what was written down.
        let (kept, total) = *live.planned.borrow();
        let so_far = if total > 0 {
            format!(
                "{} of {} passages searchable so far",
                entries.len(),
                kept + total
            )
        } else {
            format!("{} passages searchable so far", entries.len())
        };
        let since = if joined {
            " Files changed since it started aren't in it yet."
        } else {
            ""
        };
        let summary = format!(
            "The index is still being built: {so_far}, most recently changed files first, and \
             it carries on in the background. Files it hasn't reached yet won't show up here; \
             grep searches everything.{since}"
        );
        Ok(Refreshed {
            entries,
            summary,
            partial: true,
        })
    }

    /// The index as written down, off the runtime.
    async fn load(&self, index: &Index) -> Result<Vec<crate::Entry>, ToolError> {
        let index = index.clone();
        let key = crate::prefix::key(&self.model);
        tokio::task::spawn_blocking(move || index.load(&key))
            .await
            .map_err(|e| ToolError::Failed(format!("the index couldn't be read: {e}")))
    }

    /// How many passages the cosine pass should hand on.
    fn candidates(&self, keep: usize) -> usize {
        if self.rerank.is_some() {
            RERANK_CANDIDATES.max(keep)
        } else {
            keep
        }
    }

    /// How many results this model's window has room for.
    fn keep(budget: OutputBudget) -> usize {
        budget.count(RESULT_SHARE, EXCERPT_BYTES, MIN_RESULTS, MAX_RESULTS)
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
        let Refreshed {
            entries,
            summary,
            partial,
        } = match &self.indexing {
            Some(flight) => self.refresh_with_deadline(&index, flight, ctx).await?,
            None => self.refresh_to_the_end(&index, ctx).await?,
        };
        // Said under every answer from an index that isn't finished, because
        // a search that misses reads as "there's nothing like that here"
        // otherwise, and that's not what it means.
        let caveat = if partial {
            format!("\n\n{summary}")
        } else {
            String::new()
        };

        if entries.is_empty() {
            return Ok(format!(
                "Nothing indexed in this workspace, so there is nothing to search. {}",
                summary
            )
            .into());
        }

        ctx.report("searching").await;
        let vector = self
            .provider
            .embed(
                &self.model,
                &[format!(
                    "{}{query}",
                    crate::prefix::prefixes(&self.model).query
                )],
            )
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
            .into_iter()
            .next()
            .ok_or_else(|| ToolError::Failed("the backend returned no embedding".into()))?;

        let keep = Self::keep(ctx.budget);
        let candidates = self.candidates(keep);
        let passages = entries.len();
        let workspace = ctx.workspace.clone();
        // Scored and excerpted on a blocking thread: every vector in the index
        // is decoded and compared, and each hit's excerpt read from disk — a
        // few hundred milliseconds on a large index, which on the runtime would
        // be taken from the stream and every command beside it.
        let hits =
            tokio::task::spawn_blocking(move || search(&entries, &vector, candidates, &workspace))
                .await
                .map_err(|e| {
                    ToolError::Failed(format!("the search stopped before it answered: {e}"))
                })?;
        if hits.is_empty() {
            return Ok(format!(
                "No match for '{query}' in {passages} indexed passages. Try describing it \
                 differently, or use grep if you know the literal text.{caveat}"
            )
            .into());
        }

        if self.rerank.is_some() {
            ctx.report("ranking the results").await;
        }
        let hits = self.reranked(query, hits, keep).await;

        Ok(format!("{}{caveat}", render(query, &hits, ctx.workspace.as_path())).into())
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
    fn two_passage_workspace(ctx: &ToolContext) {
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
        two_passage_workspace(&ctx);

        // The embedding pass puts net.rs first for this query. The reranker
        // disagrees, and the reranker is the one that decides.
        let plain = tool(index_dir.path())
            .execute(serde_json::json!({ "query": "retry backoff" }), &ctx)
            .await
            .expect("search succeeds");
        assert!(
            plain.to_text().find("src/net.rs") < plain.to_text().find("src/store.rs"),
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
            reranked.to_text().find("src/store.rs") < reranked.to_text().find("src/net.rs"),
            "the reranker should have promoted store.rs:\n{reranked}"
        );
    }

    #[tokio::test]
    async fn a_reranked_result_says_relevance_rather_than_similarity() {
        // The scale changed, so the word beside the number has to change with
        // it — a reranker's score is not a cosine and is routinely negative.
        let (ctx, _dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        two_passage_workspace(&ctx);

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

        assert!(out.to_text().contains("(relevance "), "{out}");
        assert!(!out.to_text().contains("(similarity "), "{out}");
        assert!(
            out.to_text().contains("-6.00"),
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
        two_passage_workspace(&ctx);

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

        assert!(out.to_text().contains("src/net.rs"), "{out}");
        assert!(
            out.to_text().contains("(similarity "),
            "the fallback is the similarity order, labelled as such:\n{out}"
        );
    }

    #[tokio::test]
    async fn naming_no_rerank_model_leaves_the_tool_exactly_as_it_was() {
        // Empty is how the setting is turned off, and it has to mean off rather
        // than "rerank with a model called nothing".
        let (ctx, _dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        two_passage_workspace(&ctx);

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

        assert!(out.to_text().contains("(similarity "), "{out}");
        assert!(
            out.to_text().find("src/net.rs") < out.to_text().find("src/store.rs"),
            "{out}"
        );
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

        let net = out
            .to_text()
            .find("src/net.rs")
            .expect("net.rs in the results");
        let store = out.to_text().find("src/store.rs");
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

        assert!(out.to_text().contains("src/net.rs:1-"), "{out}");
        assert!(out.to_text().contains("similarity"), "{out}");
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
        assert!(out.to_text().contains("Nothing indexed"), "{out}");
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

        assert!(out.to_text().contains("src/new.rs"), "{out}");
        drop(dir);
    }

    /// [`Bagged`], slowly: every request waits before answering, the way the
    /// built-in engine does on a CPU.
    struct Slow(Duration);

    #[async_trait]
    impl Provider for Slow {
        fn id(&self) -> &str {
            "slow"
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
            model: &str,
            inputs: &[String],
        ) -> taurus_provider::Result<Vec<Vec<f32>>> {
            tokio::time::sleep(self.0).await;
            Bagged.embed(model, inputs).await
        }
    }

    /// Sets a file's modification time `ago` seconds into the past.
    fn age(path: &Path, ago: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(ago))
            .unwrap();
    }

    #[tokio::test]
    async fn a_long_first_index_answers_from_the_newest_files_and_carries_on() {
        let (ctx, _dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        // Forty old files of several chunks each, then the one just written:
        // about 500 passages, more than `LONG_REFRESH`, at 1.5 s a request,
        // so the whole refresh takes longer than the ten-second wait. The
        // search keeps that real wait, and shouldn't spend it: the scan says
        // at once that this won't finish in time.
        for n in 0..40 {
            let name = format!("src/old_{n:02}.rs");
            write(&ctx.workspace, &name, &filler(400));
            age(&ctx.workspace.join(&name), 100_000);
        }
        write(
            &ctx.workspace,
            "src/zz_new.rs",
            &format!(
                "{}// the retry backoff\nfn retry() {{ backoff(); }}\n",
                filler(3)
            ),
        );

        let flight = Arc::new(crate::Indexing::new());
        let search = SearchCode::new(
            Arc::new(Slow(Duration::from_millis(1500))),
            "test-embed",
            index_dir.path(),
        )
        .with_indexing(Arc::clone(&flight));

        let started = std::time::Instant::now();
        let out = search
            .execute(serde_json::json!({ "query": "retry backoff" }), &ctx)
            .await
            .expect("a partial index still answers")
            .to_text()
            .to_string();
        assert!(out.contains("still being built"), "{out}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waited {:?} for a refresh that was never going to finish",
            started.elapsed()
        );
        assert!(
            out.contains("src/zz_new.rs"),
            "the newest file comes first:\n{out}"
        );
        assert!(
            flight.busy(),
            "the refresh should carry on after the search"
        );

        // And it carries on: more is written down a little later. Stopped
        // there rather than waited out, which would cost the test a quarter
        // of a minute to show what the next write already shows.
        let key = crate::prefix::key("test-embed");
        let index = Index::new(index_dir.path(), &ctx.workspace);
        let at_answer = index.load(&key).len();
        tokio::time::sleep(Duration::from_millis(3500)).await;
        // Still running, and a stopped refresh writes down every file it
        // finished, which is how to see what it did since the answer.
        assert!(flight.busy());
        flight.stop();
        tokio::time::timeout(Duration::from_secs(10), async {
            while flight.busy() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .ok();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let later = index.load(&key).len();
        assert!(
            later > at_answer,
            "{later} passages now, {at_answer} at the answer"
        );
    }

    #[tokio::test]
    async fn a_search_joins_a_long_refresh_rather_than_restarting_it() {
        let (ctx, _dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        for n in 0..40 {
            write(&ctx.workspace, &format!("src/f{n:02}.rs"), &filler(400));
        }
        let provider: Arc<dyn Provider> = Arc::new(Slow(Duration::from_millis(300)));

        // A warm-up, the way the host starts one.
        let flight = Arc::new(crate::Indexing::new());
        let (ticket, stop, live) = flight.take_over_live(&CancellationToken::new());
        {
            let index = Index::new(index_dir.path(), &ctx.workspace);
            let workspace = ctx.workspace.clone();
            let provider = Arc::clone(&provider);
            let flight = Arc::clone(&flight);
            let live = Arc::clone(&live);
            let stop = stop.clone();
            tokio::spawn(async move {
                let _ = refresh(
                    &index,
                    &workspace,
                    &provider,
                    "test-embed",
                    &stop,
                    Some(&*live),
                )
                .await;
                flight.finished(ticket);
            });
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while live.planned().is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the scan finishes");

        let out = SearchCode::new(provider, "test-embed", index_dir.path())
            .with_indexing(Arc::clone(&flight))
            .execute(serde_json::json!({ "query": "ordinary line" }), &ctx)
            .await
            .unwrap()
            .to_text()
            .to_string();
        assert!(out.contains("still being built"), "{out}");
        assert!(out.contains("changed since it started"), "{out}");
        assert!(!stop.is_cancelled(), "the running refresh was restarted");
        flight.stop();
    }

    #[tokio::test]
    async fn a_refresh_that_finishes_in_time_is_the_whole_answer() {
        let (ctx, _dir) = allowing_ctx();
        let index_dir = tempfile::TempDir::new().unwrap();
        two_passage_workspace(&ctx);
        let flight = Arc::new(crate::Indexing::new());

        let out = tool(index_dir.path())
            .with_indexing(Arc::clone(&flight))
            .execute(serde_json::json!({ "query": "retry backoff" }), &ctx)
            .await
            .unwrap()
            .to_text()
            .to_string();
        assert!(!out.contains("still being built"), "{out}");
        assert!(out.contains("src/net.rs"), "{out}");
        assert!(!flight.busy());
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
        assert!(
            out.to_text().contains("Read the files before acting"),
            "{out}"
        );
        drop(dir);
    }
}
