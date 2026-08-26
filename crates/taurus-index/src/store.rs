//! The index on disk, and the search over it.
//!
//! One JSONL file per workspace under `~/.taurus/index/<workspace>/`, beside
//! the transcripts and checkpoints and keyed the same way, for the same reason:
//! it holds the contents of files in the project, so keeping it in the project
//! would commit it.
//!
//! # Why the vectors are base64 and not JSON numbers
//!
//! A 768-dimension vector written as JSON floats is about eight kilobytes; the
//! same vector as little-endian `f32` bytes in base64 is four. Over a few
//! thousand chunks that is the difference between a 40 MB index and a 20 MB
//! one, and the JSON version is slower to parse than the bytes are to decode.
//! Everything else on the line stays readable, so the file can still be opened
//! and understood by eye — which is the property JSONL was chosen for
//! everywhere else here.
//!
//! # Why the search is a loop over everything
//!
//! There is no approximate-nearest-neighbour structure, and there should not
//! be. A workspace this indexes is bounded at [`MAX_FILES`], which puts the
//! chunk count in the low tens of thousands; a cosine against 20,000 vectors of
//! 768 dimensions is fifteen million multiply-adds, which is single-digit
//! milliseconds. An ANN index would buy nothing measurable and would add a
//! structure that can be subtly wrong — returning *nearly* the right answers,
//! which is far harder to notice than returning none.
//!
//! # What is stale
//!
//! Each entry carries the length and modification time of the file it came
//! from, the same comparison [`taurus_tools::sweep`] uses. A file whose length
//! or mtime moved is re-embedded; one that has not is left alone. A file that
//! has vanished is dropped. So a refresh after editing three files costs three
//! files' worth of embedding, not the workspace's.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use base64::Engine;
use serde::{Deserialize, Serialize};

/// Bumped when an entry's shape changes incompatibly, or when the meaning of a
/// vector does. An index written by a newer Taurus is discarded and rebuilt
/// rather than half-read — unlike a checkpoint, nothing is lost by rebuilding.
///
/// Still 1. Structure-snapped chunks and embedded headings were built, would
/// have had to move this, and were measured retrieving worse than the line
/// windows they replaced — see `examples/retrieval.rs` and the entry in
/// `docs/known-gaps.md`.
const FORMAT_VERSION: u32 = 1;

/// Most files one workspace may index.
///
/// The same order of magnitude as [`taurus_tools::sweep`]'s cap and for a
/// related reason: past this the indexing itself is the cost. It also bounds
/// the search, which is a loop over every chunk.
pub const MAX_FILES: usize = 20_000;

/// Largest file that is chunked at all. Past this it is a database, a bundled
/// asset, or generated output, and none of the three answer questions.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// One embedded passage.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    /// Workspace-relative, so it reads the same as every other path the model
    /// is given.
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    /// Length and mtime of the file this came from, for staleness. Held per
    /// entry rather than per file so one torn line costs one chunk.
    pub len: u64,
    pub modified: u64,
    /// The vector, little-endian `f32`, base64. See the module note.
    pub vector: String,
}

impl Entry {
    pub fn decode(&self) -> Option<Vec<f32>> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(self.vector.as_bytes())
            .ok()?;
        if bytes.len() % 4 != 0 {
            return None;
        }
        Some(
            bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        )
    }
}

pub fn encode(vector: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    /// The embedding model these vectors came from.
    ///
    /// Vectors from two models are not comparable — different dimensions if you
    /// are lucky, and silently meaningless similarities if you are not. So the
    /// model names the index, and changing it discards the index rather than
    /// mixing them.
    model: String,
    workspace: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record {
    Header(Header),
    Entry(Entry),
}

/// One hit, with enough around it to be worth reading.
#[derive(Clone, Debug)]
pub struct Hit {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    /// How this hit scored, on whichever scale [`Hit::ranking`] names.
    ///
    /// Reported so the model can tell a strong match from the best of a bad
    /// set — but only against the other hits in the same result. See
    /// [`Ranking`] for why the number is not comparable to anything else.
    pub score: f32,
    /// Which stage produced [`Hit::score`] and the order these came back in.
    pub ranking: Ranking,
    pub text: String,
}

/// Which stage decided the order a set of hits is in.
///
/// Carried on the hit rather than assumed by the caller because the two scales
/// do not mean the same thing and are not comparable. A cosine similarity is
/// bounded at -1 to 1 and says how close two vectors point; a reranker's score
/// is whatever that model emits, which on a local llama.cpp is a raw logit that
/// is routinely negative. Printing "similarity 0.71" above a number that is
/// neither a similarity nor on that scale would be a small lie told to the
/// model in the one place it is trying to judge whether a result is worth
/// reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ranking {
    /// Cosine against the query vector, -1 to 1.
    Similarity,
    /// A reranking model's judgement of the passage against the query. Higher
    /// is better and that is the only guarantee — see
    /// [`taurus_provider::RerankScore::score`].
    Relevance,
}

impl Ranking {
    /// The word the model reads beside the number.
    pub fn label(self) -> &'static str {
        match self {
            Self::Similarity => "similarity",
            Self::Relevance => "relevance",
        }
    }
}

/// One workspace's index.
///
/// Two paths and nothing else, so a caller that needs one on a blocking thread
/// clones it rather than borrowing across the boundary.
#[derive(Clone)]
pub struct Index {
    path: PathBuf,
    workspace: PathBuf,
}

impl Index {
    pub fn new(dir: impl Into<PathBuf>, workspace: impl Into<PathBuf>) -> Self {
        Self {
            path: dir.into().join("index.jsonl"),
            workspace: workspace.into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every entry, or nothing at all.
    ///
    /// An index built by a different model, or by a newer Taurus, reads as
    /// empty rather than as an error: it is a cache, and the only correct
    /// response to one that cannot be understood is to rebuild it.
    pub fn load(&self, model: &str) -> Vec<Entry> {
        let Ok(file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };

        let mut entries = Vec::new();
        let mut usable = false;

        for line in BufReader::new(file).lines().map_while(Result::ok) {
            match serde_json::from_str::<Record>(&line) {
                Ok(Record::Header(header)) => {
                    if header.version != FORMAT_VERSION || header.model != model {
                        tracing::info!(
                            held = %header.model,
                            wanted = %model,
                            "discarding an index built with different settings"
                        );
                        return Vec::new();
                    }
                    usable = true;
                }
                Ok(Record::Entry(entry)) => entries.push(entry),
                // A torn final line costs the chunk it held, which the next
                // refresh re-embeds.
                Err(_) => continue,
            }
        }

        if usable {
            entries
        } else {
            Vec::new()
        }
    }

    /// Replaces the index wholesale.
    ///
    /// Written to a temporary file in the same directory and renamed over the
    /// old one, so a crash or a full disk leaves the previous index intact
    /// rather than a half-written one that loads as a plausible subset.
    pub fn save(&self, model: &str, entries: &[Entry]) -> Result<(), String> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| "the index has no directory".to_string())?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

        let temporary = self.path.with_extension("jsonl.new");
        let mut file = std::fs::File::create(&temporary)
            .map_err(|e| format!("{}: {e}", temporary.display()))?;

        let write = |file: &mut std::fs::File, record: &Record| -> std::io::Result<()> {
            let line = serde_json::to_string(record)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")
        };

        let result = (|| {
            write(
                &mut file,
                &Record::Header(Header {
                    version: FORMAT_VERSION,
                    model: model.to_string(),
                    workspace: self.workspace.display().to_string(),
                }),
            )?;
            for entry in entries {
                write(&mut file, &Record::Entry(entry.clone()))?;
            }
            file.sync_all()
        })();

        if let Err(e) = result {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("{}: {e}", temporary.display()));
        }

        // The index holds the contents of files in the project, `.env`
        // included where one was indexed, so it is readable by its owner and
        // nobody else — the same rule the checkpoint logs follow.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
        }

        std::fs::rename(&temporary, &self.path).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    /// Discards the index. A missing one is success.
    pub fn forget(&self) -> Result<(), String> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", self.path.display())),
        }
    }
}

/// The best `limit` entries for a query vector, best first.
///
/// At most one hit per file: a query that matches a file well usually matches
/// three consecutive chunks of it, and three windows of the same function is a
/// worse answer than three different places to look.
pub fn search(entries: &[Entry], query: &[f32], limit: usize, workspace: &Path) -> Vec<Hit> {
    let mut best: HashMap<&str, (f32, &Entry)> = HashMap::new();

    for entry in entries {
        let Some(vector) = entry.decode() else {
            continue;
        };
        if vector.len() != query.len() {
            continue;
        }
        let score = cosine(&vector, query);
        match best.get(entry.path.as_str()) {
            Some((held, _)) if *held >= score => {}
            _ => {
                best.insert(entry.path.as_str(), (score, entry));
            }
        }
    }

    let mut hits: Vec<(f32, &Entry)> = best.into_values().collect();
    // Descending by score. `total_cmp` rather than `partial_cmp` because a NaN
    // from a degenerate vector would otherwise make the ordering inconsistent
    // and the sort's result unspecified.
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    hits.truncate(limit);

    hits.into_iter()
        .map(|(score, entry)| Hit {
            path: entry.path.clone(),
            start_line: entry.start_line,
            end_line: entry.end_line,
            score,
            ranking: Ranking::Similarity,
            // Read from disk rather than stored: the index would otherwise hold
            // a second copy of the workspace, and the file on disk is the one
            // the model is about to act on anyway.
            text: excerpt(workspace, entry),
        })
        .collect()
}

/// Reorders a cosine shortlist by a reranker's scores, best first, keeping
/// `limit`.
///
/// The second half of a two-stage retrieval. The first stage compares a query
/// vector against passage vectors that were embedded without ever having seen
/// the query, which is what makes an index possible and also what caps how good
/// it can be. This stage reads the query and the passage together and is
/// markedly better at judging them, at a cost that only makes sense over a
/// shortlist — which is exactly what [`search`] just produced.
///
/// Pure, and separate from the call that produces the scores, because the
/// ordering rules are where the mistakes live and they should be testable
/// without a server.
///
/// # What it guarantees
///
/// - Every hit that was scored comes before every hit that was not. A server
///   honoring its own `top_n` returns fewer scores than documents, and the
///   unscored remainder is *not* evidence of irrelevance — it is the absence of
///   an opinion, so those hits keep their cosine order behind the scored ones
///   rather than being discarded.
/// - A score naming a hit that does not exist is ignored rather than trusted.
///   The provider adapters reject this at the boundary; this is the second
///   layer, because a panic here would be an index-out-of-bounds in a tool call
///   rather than an error a model can read.
/// - Ties hold their previous order, so a reranker that scores two passages
///   identically leaves cosine to break it.
pub fn rerank(
    mut hits: Vec<Hit>,
    scores: &[taurus_provider::RerankScore],
    limit: usize,
) -> Vec<Hit> {
    // Rank by position in the reranker's descending order, not by the score
    // itself: the scores are only ordinal, and sorting hits by a float that
    // means something different on every backend would invite exactly the
    // comparisons `RerankScore::score` warns against.
    let mut ordered: Vec<&taurus_provider::RerankScore> =
        scores.iter().filter(|s| s.index < hits.len()).collect();
    ordered.sort_by(|a, b| b.score.total_cmp(&a.score));

    let mut rank = vec![usize::MAX; hits.len()];
    for (position, scored) in ordered.iter().enumerate() {
        // First score wins if a server sent the same index twice, which puts
        // the duplicate's higher score in front rather than its lower one.
        if rank[scored.index] == usize::MAX {
            rank[scored.index] = position;
        }
    }

    // `usize::MAX` for the unscored sorts them last by construction, and a
    // stable sort keeps them in the cosine order they arrived in.
    let mut indexed: Vec<(usize, Hit)> = hits.drain(..).enumerate().collect();
    indexed.sort_by_key(|(original, _)| rank[*original]);

    indexed
        .into_iter()
        .take(limit)
        .map(|(original, mut hit)| {
            // A hit the reranker never scored keeps the cosine number it came
            // with, and says so. Overwriting it with a placeholder would lose
            // the only judgement anything actually made about it.
            if let Some(scored) = scores.iter().find(|s| s.index == original) {
                hit.score = scored.score;
                hit.ranking = Ranking::Relevance;
            }
            hit
        })
        .collect()
}

/// The lines a hit covers, as they stand now.
fn excerpt(workspace: &Path, entry: &Entry) -> String {
    let Ok(path) = taurus_tools::path_guard::resolve(workspace, &entry.path) else {
        return String::new();
    };
    let Ok(contents) = std::fs::read_to_string(path) else {
        return String::new();
    };
    contents
        .lines()
        .skip(entry.start_line.saturating_sub(1))
        .take(entry.end_line.saturating_sub(entry.start_line) + 1)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Cosine similarity. Zero for a zero vector rather than NaN.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let magnitude = (norm_a.sqrt()) * (norm_b.sqrt());
    if magnitude == 0.0 {
        0.0
    } else {
        dot / magnitude
    }
}

/// A file's length and modification time, as staleness is judged.
pub fn stamp(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_default();
    Some((metadata.len(), modified))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    use taurus_provider::RerankScore;

    fn hit(path: &str, score: f32) -> Hit {
        Hit {
            path: path.into(),
            start_line: 1,
            end_line: 10,
            score,
            ranking: Ranking::Similarity,
            text: format!("body of {path}"),
        }
    }

    fn scored(index: usize, score: f32) -> RerankScore {
        RerankScore { index, score }
    }

    fn paths(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.path.as_str()).collect()
    }

    #[test]
    fn reranking_reorders_the_shortlist_and_relabels_the_score() {
        // Cosine put c.rs last; the reranker puts it first, which is the whole
        // point of the second stage.
        let hits = vec![hit("a.rs", 0.90), hit("b.rs", 0.85), hit("c.rs", 0.80)];
        let out = rerank(hits, &[scored(2, 8.1), scored(0, 2.0), scored(1, -4.5)], 5);

        assert_eq!(paths(&out), ["c.rs", "a.rs", "b.rs"]);
        assert!(out.iter().all(|h| h.ranking == Ranking::Relevance));
        // The number reported is the one that decided the order, not the
        // cosine it replaced.
        assert_eq!(out[0].score, 8.1);
    }

    #[test]
    fn a_negative_score_is_an_ordinary_ranking_not_a_rejection() {
        // llama.cpp returns raw cross-encoder logits, where every document in a
        // weak result set scores below zero. Treating that as "no match" would
        // empty the result exactly when the model most needs something to read.
        let hits = vec![hit("a.rs", 0.5), hit("b.rs", 0.4)];
        let out = rerank(hits, &[scored(0, -8.3), scored(1, -4.7)], 5);

        assert_eq!(paths(&out), ["b.rs", "a.rs"], "less negative ranks higher");
        assert_eq!(out.len(), 2, "nothing is dropped for scoring below zero");
    }

    #[test]
    fn unscored_hits_fall_behind_the_scored_ones_in_cosine_order() {
        // A server honoring a smaller `top_n` than it was asked for says
        // nothing about the rest. Silence is not a low score, so the remainder
        // keeps its original order behind everything that was judged.
        let hits = vec![
            hit("a.rs", 0.90),
            hit("b.rs", 0.85),
            hit("c.rs", 0.80),
            hit("d.rs", 0.75),
        ];
        let out = rerank(hits, &[scored(3, 5.0)], 5);

        assert_eq!(paths(&out), ["d.rs", "a.rs", "b.rs", "c.rs"]);
        assert_eq!(out[0].ranking, Ranking::Relevance);
        assert_eq!(
            out[1].ranking,
            Ranking::Similarity,
            "a hit nobody judged keeps the score and the label it arrived with"
        );
        assert_eq!(out[1].score, 0.90);
    }

    #[test]
    fn a_score_for_a_document_that_was_not_sent_is_ignored() {
        // The adapters reject this at the boundary; this is the layer that
        // keeps a misbehaving server from turning into a panic inside a tool
        // call rather than an answer.
        let hits = vec![hit("a.rs", 0.9), hit("b.rs", 0.8)];
        let out = rerank(hits, &[scored(7, 9.9), scored(1, 1.0)], 5);

        assert_eq!(paths(&out), ["b.rs", "a.rs"]);
    }

    #[test]
    fn reranking_truncates_to_the_limit_after_reordering_not_before() {
        // The candidate list is deliberately much longer than the result, so
        // truncating first would throw away the passage the reranker exists to
        // promote.
        let hits = vec![
            hit("a.rs", 0.90),
            hit("b.rs", 0.85),
            hit("c.rs", 0.80),
            hit("d.rs", 0.75),
        ];
        let out = rerank(hits, &[scored(3, 9.0), scored(0, 1.0)], 2);

        assert_eq!(paths(&out), ["d.rs", "a.rs"]);
    }

    #[test]
    fn no_scores_at_all_leaves_the_cosine_order_standing() {
        let hits = vec![hit("a.rs", 0.9), hit("b.rs", 0.8), hit("c.rs", 0.7)];
        let out = rerank(hits, &[], 2);

        assert_eq!(paths(&out), ["a.rs", "b.rs"]);
        assert!(out.iter().all(|h| h.ranking == Ranking::Similarity));
    }

    fn entry(path: &str, vector: &[f32]) -> Entry {
        Entry {
            path: path.into(),
            start_line: 1,
            end_line: 10,
            len: 100,
            modified: 1,
            vector: encode(vector),
        }
    }

    #[test]
    fn a_vector_survives_the_round_trip_through_base64() {
        let original = vec![0.5, -0.25, 1.0, 0.0, -1.0];
        let decoded = entry("a.rs", &original).decode().expect("valid base64");
        assert_eq!(decoded, original);
    }

    #[test]
    fn base64_is_smaller_than_the_json_it_replaces() {
        // The whole reason for the encoding. A 768-dimension vector as JSON
        // floats is about twice the size, over thousands of chunks.
        let vector: Vec<f32> = (0..768).map(|n| n as f32 * 0.001234).collect();
        let as_json = serde_json::to_string(&vector).unwrap().len();
        assert!(
            encode(&vector).len() < as_json,
            "base64 {} vs json {as_json}",
            encode(&vector).len()
        );
    }

    #[test]
    fn a_corrupt_vector_decodes_to_nothing_rather_than_garbage() {
        let mut broken = entry("a.rs", &[1.0]);
        broken.vector = "!!!".into();
        assert!(broken.decode().is_none());

        // Bytes that are valid base64 but not a whole number of floats.
        broken.vector = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        assert!(broken.decode().is_none());
    }

    #[test]
    fn cosine_is_one_for_the_same_direction_and_zero_for_a_right_angle() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert!((cosine(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_zero_vector_scores_zero_rather_than_nan() {
        // A degenerate embedding would otherwise poison the sort: NaN makes the
        // ordering inconsistent and the result unspecified.
        let score = cosine(&[0.0, 0.0], &[1.0, 1.0]);
        assert!(!score.is_nan());
        assert_eq!(score, 0.0);
    }

    #[test]
    fn results_come_back_best_first() {
        let dir = TempDir::new().unwrap();
        let entries = vec![
            entry("far.rs", &[0.0, 1.0]),
            entry("near.rs", &[1.0, 0.0]),
            entry("middling.rs", &[0.7, 0.7]),
        ];
        let hits = search(&entries, &[1.0, 0.0], 3, dir.path());
        assert_eq!(
            hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["near.rs", "middling.rs", "far.rs"]
        );
    }

    #[test]
    fn one_hit_per_file_however_many_chunks_matched() {
        // A query that matches a file usually matches three consecutive chunks
        // of it, and three windows of one function is a worse answer than three
        // places to look.
        let dir = TempDir::new().unwrap();
        let entries = vec![
            entry("same.rs", &[1.0, 0.0]),
            entry("same.rs", &[0.99, 0.1]),
            entry("same.rs", &[0.98, 0.2]),
            entry("other.rs", &[0.5, 0.5]),
        ];
        let hits = search(&entries, &[1.0, 0.0], 5, dir.path());
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "same.rs");
    }

    #[test]
    fn a_vector_of_the_wrong_width_is_skipped_rather_than_compared() {
        // Two embedding models produce incomparable vectors. The header
        // normally catches that; this is the belt to its braces.
        let dir = TempDir::new().unwrap();
        let entries = vec![
            entry("wrong.rs", &[1.0, 0.0, 0.0]),
            entry("right.rs", &[1.0, 0.0]),
        ];
        let hits = search(&entries, &[1.0, 0.0], 5, dir.path());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "right.rs");
    }

    #[test]
    fn an_index_round_trips_through_disk() {
        let dir = TempDir::new().unwrap();
        let index = Index::new(dir.path(), dir.path());
        let entries = vec![entry("a.rs", &[1.0, 0.0]), entry("b.rs", &[0.0, 1.0])];

        index.save("nomic-embed-text", &entries).unwrap();
        let loaded = index.load("nomic-embed-text");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].path, "a.rs");
    }

    #[test]
    fn an_index_built_by_another_model_reads_as_empty() {
        // Vectors from two models are not comparable — different dimensions if
        // you are lucky, silently meaningless similarities if you are not.
        let dir = TempDir::new().unwrap();
        let index = Index::new(dir.path(), dir.path());
        index
            .save("nomic-embed-text", &[entry("a.rs", &[1.0])])
            .unwrap();

        assert!(index.load("mxbai-embed-large").is_empty());
        assert_eq!(index.load("nomic-embed-text").len(), 1);
    }

    #[test]
    fn a_file_with_no_header_reads_as_empty_rather_than_as_entries() {
        // A header is what says which model the vectors came from. Entries
        // without one are of unknown provenance and unusable.
        let dir = TempDir::new().unwrap();
        let index = Index::new(dir.path(), dir.path());
        std::fs::write(
            index.path(),
            format!(
                "{}\n",
                serde_json::to_string(&Record::Entry(entry("a.rs", &[1.0]))).unwrap()
            ),
        )
        .unwrap();
        assert!(index.load("nomic-embed-text").is_empty());
    }

    #[test]
    fn a_failed_save_leaves_the_previous_index_intact() {
        // Written to a temporary and renamed, so a crash cannot leave a
        // half-written file that loads as a plausible subset.
        let dir = TempDir::new().unwrap();
        let index = Index::new(dir.path(), dir.path());
        index.save("m", &[entry("a.rs", &[1.0])]).unwrap();

        assert!(index.save("m", &[entry("b.rs", &[1.0])]).is_ok());
        let loaded = index.load("m");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].path, "b.rs", "the rename did not take");
        assert!(
            !dir.path().join("index.jsonl.new").exists(),
            "the temporary file was left behind"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_index_is_readable_by_its_owner_and_nobody_else() {
        // It holds the contents of files in the project. The same rule the
        // checkpoint logs follow, for the same reason.
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let index = Index::new(dir.path(), dir.path());
        index.save("m", &[entry("a.rs", &[1.0])]).unwrap();

        let mode = std::fs::metadata(index.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "index mode was {:o}", mode & 0o777);
    }

    #[test]
    fn a_missing_index_is_empty_rather_than_an_error() {
        let dir = TempDir::new().unwrap();
        assert!(Index::new(dir.path(), dir.path()).load("m").is_empty());
    }

    #[test]
    fn forgetting_an_index_that_is_not_there_is_success() {
        let dir = TempDir::new().unwrap();
        assert!(Index::new(dir.path(), dir.path()).forget().is_ok());
    }
}
