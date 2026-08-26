//! Cutting a file into pieces small enough to embed and large enough to mean
//! something.
//!
//! An embedding is one vector for one passage, so the passage is the unit of
//! retrieval: too small and every chunk is a fragment that matches everything,
//! too large and the vector is an average of six unrelated ideas and matches
//! nothing. Somewhere around thirty to sixty lines of source is where a chunk
//! is still about one thing.
//!
//! # Lines, not syntax
//!
//! This splits on line counts and nothing else. A parser per language would cut
//! at function boundaries and produce better chunks — and would need a grammar
//! for every language in the workspace, would fall back to *this* for the ones
//! it did not have, and would go wrong silently on a file it half understood.
//! Line windows are worse per chunk and identical in every language, which for
//! a tool that has to work on whatever the user opened is the better trade.
//!
//! The overlap is what stops the seam being a blind spot. A function split
//! across two chunks would otherwise be half in each and whole in neither, so
//! consecutive chunks share [`OVERLAP_LINES`] of context and a passage never
//! falls entirely between two of them.

/// Lines in one chunk, before overlap.
///
/// Sized so a chunk is about one thing: a short function and its signature, or
/// a section of a config file. Longer and the vector averages unrelated ideas
/// together, which is how an index answers every query with the same file.
const CHUNK_LINES: usize = 40;

/// Lines each chunk repeats from the one before it.
///
/// The seam insurance. Without it a function split across a boundary is half in
/// each chunk and whole in neither, so the query that describes it matches
/// neither well.
const OVERLAP_LINES: usize = 10;

/// Longest single line kept whole.
///
/// A minified bundle or a base64 blob is one line of tens of thousands of
/// characters, and a chunk containing it is almost entirely that line. Cut
/// rather than skipped: the surrounding lines are still worth indexing.
const MAX_LINE_CHARS: usize = 500;

/// Smallest chunk worth embedding, in non-whitespace characters.
///
/// The tail of a file is often a closing brace and two blank lines. Embedding
/// that costs a request and produces a vector that sits near every other
/// almost-empty chunk in the workspace.
const MIN_CHUNK_CHARS: usize = 40;

/// One passage of one file, and where it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// 1-based, inclusive. What a result cites, so it has to be the file's own
    /// numbering rather than an offset into the chunking.
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

/// Splits a file's text into overlapping windows.
///
/// Returns nothing for a file with no substance in it, which is a real state:
/// an empty `mod.rs`, a placeholder, a file of blank lines.
pub fn split(contents: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = contents.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    let stride = CHUNK_LINES.saturating_sub(OVERLAP_LINES).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < lines.len() {
        let end = (start + CHUNK_LINES).min(lines.len());
        let text = lines[start..end]
            .iter()
            .map(|line| clip(line))
            .collect::<Vec<_>>()
            .join("\n");

        if text.chars().filter(|c| !c.is_whitespace()).count() >= MIN_CHUNK_CHARS {
            chunks.push(Chunk {
                start_line: start + 1,
                end_line: end,
                text,
            });
        }

        // The last window always reaches the end of the file, so stopping here
        // rather than striding past it avoids emitting a final chunk that is
        // entirely overlap with the one before it.
        if end == lines.len() {
            break;
        }
        start += stride;
    }

    chunks
}

fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    line.chars().take(MAX_LINE_CHARS).collect::<String>() + " …"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(lines: usize) -> String {
        (0..lines)
            .map(|n| format!("let value_{n} = compute_something({n});"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_short_file_is_one_chunk_covering_all_of_it() {
        let chunks = split(&source(10));
        assert_eq!(chunks.len(), 1);
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (1, 10));
    }

    #[test]
    fn consecutive_chunks_overlap_so_a_seam_is_not_a_blind_spot() {
        // A function split across a boundary would otherwise be half in each
        // chunk and whole in neither.
        let chunks = split(&source(100));
        assert!(chunks.len() > 1);
        for pair in chunks.windows(2) {
            let [first, second] = pair else {
                unreachable!()
            };
            assert!(
                second.start_line <= first.end_line,
                "gap between {}-{} and {}-{}",
                first.start_line,
                first.end_line,
                second.start_line,
                second.end_line
            );
        }
    }

    #[test]
    fn every_line_of_the_file_lands_in_some_chunk() {
        // The property that matters for retrieval: nothing is unreachable.
        let total = 137;
        let chunks = split(&source(total));
        let mut covered = vec![false; total + 1];
        for chunk in &chunks {
            covered[chunk.start_line..=chunk.end_line].fill(true);
        }
        assert!(
            covered[1..].iter().all(|c| *c),
            "some lines were not indexed"
        );
    }

    #[test]
    fn the_last_chunk_ends_at_the_last_line() {
        let chunks = split(&source(95));
        assert_eq!(chunks.last().unwrap().end_line, 95);
    }

    #[test]
    fn line_numbers_are_the_files_own_so_a_result_can_be_opened() {
        let chunks = split(&source(100));
        assert_eq!(chunks[0].start_line, 1, "1-based, like every editor");
    }

    #[test]
    fn a_file_of_nothing_produces_nothing_to_embed() {
        // Real states: an empty `mod.rs`, a placeholder, a file of blank lines.
        // Each would cost a request and produce a vector near every other
        // almost-empty chunk in the workspace.
        assert!(split("").is_empty());
        assert!(split("\n\n\n\n").is_empty());
        assert!(split("}\n").is_empty());
    }

    #[test]
    fn a_minified_line_is_cut_rather_than_taking_the_chunk_with_it() {
        // One line of a bundle is tens of thousands of characters, and a chunk
        // containing it is almost entirely that line. The lines around it are
        // still worth indexing, so the line is cut and the chunk is kept.
        let long = "x".repeat(50_000);
        let contents = format!("fn real_code() {{}}\n{long}\nfn more_real_code() {{}}");
        let chunks = split(&contents);

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("fn real_code"));
        assert!(chunks[0].text.contains("fn more_real_code"));
        assert!(
            chunks[0].text.chars().count() < MAX_LINE_CHARS + 200,
            "the long line was kept whole"
        );
    }

    #[test]
    fn no_chunk_is_entirely_overlap_with_the_one_before_it() {
        // Striding past the end of the file would emit a final window made
        // only of lines already covered — a request paid for nothing.
        for total in [41, 45, 60, 71, 90] {
            let chunks = split(&source(total));
            for pair in chunks.windows(2) {
                let [first, second] = pair else {
                    unreachable!()
                };
                assert!(
                    second.end_line > first.end_line,
                    "{total} lines: chunk {}-{} adds nothing to {}-{}",
                    second.start_line,
                    second.end_line,
                    first.start_line,
                    first.end_line
                );
            }
        }
    }
}
