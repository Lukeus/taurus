//! Cutting a file into pieces small enough to embed and large enough to mean
//! something.
//!
//! An embedding is one vector for one passage, so the passage is the unit of
//! retrieval: too small and every chunk is a fragment that matches everything,
//! too large and the vector is an average of six unrelated ideas and matches
//! nothing. Somewhere around thirty to sixty lines of source is where a chunk
//! is still about one thing.
//!
//! # Structure, not syntax
//!
//! This used to split on line counts and nothing else, and said why: a parser
//! per language would cut at real boundaries and produce better chunks, but it
//! would need a grammar for every language in the workspace, would fall back to
//! line windows for the ones it did not have, and would go wrong silently on a
//! file it half understood.
//!
//! All three objections stand, and none of them applies here, because there is
//! no grammar. What this reads is layout — indentation, blank lines, and lines
//! made of nothing but closing punctuation — which every language a person
//! writes by hand has, and which means the same thing in all of them: a
//! non-blank line at zero indent, after a blank line or after the close of what
//! came before, is where a new top-level thing starts.
//!
//! So a cut is *snapped* to one of those when there is one within
//! [`SNAP_LINES`] of where the line count would have cut anyway. Where there is
//! not — a minified bundle, a table of data, a file with no blank lines in it —
//! the cut lands exactly where it used to. That is the whole fallback, and it
//! is the same code path rather than a second one.
//!
//! Nothing here claims to understand the file. The worst case is a cut a few
//! lines from the best place, bounded by the snap window, which is what the
//! line count was doing every time.
//!
//! # What a chunk is *of*
//!
//! A window of code says what it does and not what it is. Forty lines from the
//! middle of a long `impl` are a body with no signature above them, so the
//! vector describes the statements and not the function they belong to — and
//! "the function that resolves a workspace path" has nothing to match on but
//! the words in the body.
//!
//! Every chunk therefore carries a [heading](Chunk::heading): the path of the
//! file, and the lines that enclose it, read off the same indentation the cuts
//! are. That heading is embedded with the body and is not part of what a hit
//! shows — the excerpt is re-read from the file by line number, so nothing the
//! model reads has a line in it that the file does not have.
//!
//! Bump `store::FORMAT_VERSION` when anything in here changes what goes into a
//! vector. An index is judged current by each file's length and modification
//! time, so a re-chunked workspace whose files have not been touched would
//! otherwise keep vectors that were built from different passages.

/// Lines in one chunk, before snapping.
///
/// Sized so a chunk is about one thing: a short function and its signature, or
/// a section of a config file. Longer and the vector averages unrelated ideas
/// together, which is how an index answers every query with the same file.
const CHUNK_LINES: usize = 40;

/// Lines each chunk repeats from the one before it, after a cut that had to be
/// made mid-structure.
///
/// The seam insurance, and it is now only paid where a seam is actually a risk.
/// A cut snapped to a real boundary has nothing to insure against: what is on
/// the other side of it is a different top-level thing, and repeating ten lines
/// of it into this chunk would put the head of the next function in the vector
/// for this one.
const OVERLAP_LINES: usize = 10;

/// How far from the line count a cut may move to reach a real boundary.
///
/// Small on purpose. This is a nudge onto a nearby seam, not a search for the
/// best split in the file — a wide window would let one enormous function pull
/// a cut far from where the size budget wanted it, which is the failure the
/// size budget exists to prevent.
///
/// It is also the only bound the snap needs. A separate floor and ceiling on
/// chunk length would read as belt and braces and be neither: every cut starts
/// from `start + CHUNK_LINES`, so this window already holds every chunk to
/// between 28 and 52 lines, and a second pair of constants nothing could ever
/// reach is a knob with no setting behind it.
const SNAP_LINES: usize = 12;

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

/// How many enclosing lines a heading carries.
///
/// Three is a module, a type, and a function, which is as much as names a
/// passage. Past that the heading is longer than some of the bodies it
/// describes and starts competing with them for the vector.
const HEADING_DEPTH: usize = 3;

/// One passage of one file, and where it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// 1-based, inclusive. What a result cites, so it has to be the file's own
    /// numbering rather than an offset into the chunking.
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    /// The lines this chunk sits inside, outermost first.
    ///
    /// Empty for a chunk that starts at the top level, which has nothing
    /// enclosing it. Never shown — see the module note.
    pub heading: Vec<String>,
}

impl Chunk {
    /// What actually gets embedded: where this is, then what it says.
    ///
    /// The path is in here because a name in it is often the best description
    /// of the passage there is — `crates/taurus-index/src/store.rs` says more
    /// about forty lines of serialization than the forty lines do — and because
    /// a query that names a module could otherwise match only chunks that
    /// happened to mention it.
    pub fn passage(&self, path: &str) -> String {
        let mut out = String::with_capacity(self.text.len() + 128);
        out.push_str(path);
        out.push('\n');
        for line in &self.heading {
            out.push_str(line);
            out.push('\n');
        }
        // A blank line between what this is and what it says, so the two do not
        // read as one run-on statement to a model that was trained on prose.
        out.push('\n');
        out.push_str(&self.text);
        out
    }
}

/// Splits a file's text into windows, snapped to structure where there is any.
///
/// Returns nothing for a file with no substance in it, which is a real state:
/// an empty `mod.rs`, a placeholder, a file of blank lines.
pub fn split(contents: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = contents.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    let opens = cut_points(&lines);
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < lines.len() {
        let ideal = start + CHUNK_LINES;
        // The end of the file is a boundary, and the best one there is.
        let (end, snapped) = if ideal >= lines.len() {
            (lines.len(), true)
        } else {
            match nearest_cut(&opens, ideal, start) {
                Some(at) => (at, true),
                None => (ideal, false),
            }
        };

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
                heading: heading(&lines, start),
            });
        }

        if end == lines.len() {
            break;
        }
        // Overlap only where the cut was forced. See `OVERLAP_LINES`.
        start = if snapped {
            end
        } else {
            end.saturating_sub(OVERLAP_LINES)
        };
    }

    chunks
}

/// Where a new top-level thing starts.
///
/// Three conditions, and each rules out a case the other two let through. The
/// line has something on it, so a run of blanks is not a run of boundaries. It
/// begins at column zero, which is what "top level" means in every language
/// written by a person. And something ended before it — a blank line, or a line
/// of nothing but closing punctuation — so the second line of a two-line `use`
/// block is not a boundary and neither is every line of a flat config file.
fn cut_points(lines: &[&str]) -> Vec<bool> {
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if line.trim().is_empty() || line.starts_with([' ', '\t']) {
                return false;
            }
            i == 0 || lines[i - 1].trim().is_empty() || closes(lines[i - 1])
        })
        .collect()
}

/// Whether a line is nothing but the end of something.
fn closes(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| matches!(c, '}' | ')' | ']' | ';' | ','))
}

/// The boundary nearest `ideal`, or `None` if there is none worth moving to.
///
/// Nearest rather than next, and it walks outwards alternately, so a cut moves
/// as little as it can — a boundary two lines early beats one nine lines late.
fn nearest_cut(opens: &[bool], ideal: usize, start: usize) -> Option<usize> {
    for offset in 0..=SNAP_LINES {
        for candidate in [ideal.checked_sub(offset), Some(ideal + offset)]
            .into_iter()
            .flatten()
        {
            // Past `start` as well as inside the window: a boundary at or
            // before where this chunk began would cut nothing and loop.
            if candidate > start && candidate < opens.len() && opens[candidate] {
                return Some(candidate);
            }
        }
    }
    None
}

/// The lines that enclose the chunk starting at `start`, outermost first.
///
/// Read off indentation, walking back for each line that sits further left than
/// the last one taken. A line of nothing but closing punctuation is skipped —
/// it is the end of something that ended before this chunk began, not something
/// this chunk is inside.
fn heading(lines: &[&str], start: usize) -> Vec<String> {
    let Some(base) = lines[start..].iter().find_map(|line| indent(line)) else {
        return Vec::new();
    };
    if base == 0 {
        return Vec::new();
    }

    let mut found = Vec::new();
    let mut want = base;
    for line in lines[..start].iter().rev() {
        if closes(line) {
            continue;
        }
        let Some(depth) = indent(line) else { continue };
        if depth >= want {
            continue;
        }
        found.push(clip(line.trim_end()));
        want = depth;
        if depth == 0 || found.len() == HEADING_DEPTH {
            break;
        }
    }
    found.reverse();
    found
}

/// How far a line is indented, or `None` if it has nothing on it.
///
/// A tab counts as one, like a space. The absolute number never leaves this
/// file — only comparisons between lines of the same file do — so the two do
/// not have to agree about width, and a file that mixes them is compared
/// against itself either way.
fn indent(line: &str) -> Option<usize> {
    if line.trim().is_empty() {
        return None;
    }
    Some(line.len() - line.trim_start().len())
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

    /// A file with no structure in it at all: no blank lines, nothing at zero
    /// indent after one. Every cut here is a forced cut, which is what makes
    /// this the fixture for the old behaviour.
    fn flat(lines: usize) -> String {
        (0..lines)
            .map(|n| format!("    let value_{n} = compute_something({n});"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Something shaped like source: top-level items, separated by blank lines.
    fn structured(items: usize, body: usize) -> String {
        let mut out = String::new();
        for n in 0..items {
            out.push_str(&format!("/// What item {n} is for.\n"));
            out.push_str(&format!("pub fn item_{n}(input: &str) -> String {{\n"));
            for line in 0..body {
                out.push_str(&format!("    let step_{line} = input.len() + {line};\n"));
            }
            out.push_str("}\n\n");
        }
        out
    }

    fn covered(chunks: &[Chunk], total: usize) -> bool {
        let mut seen = vec![false; total + 1];
        for chunk in chunks {
            seen[chunk.start_line..=chunk.end_line].fill(true);
        }
        seen[1..].iter().all(|c| *c)
    }

    #[test]
    fn a_short_file_is_one_chunk_covering_all_of_it() {
        let chunks = split(&flat(10));
        assert_eq!(chunks.len(), 1);
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (1, 10));
    }

    #[test]
    fn every_line_of_the_file_lands_in_some_chunk() {
        // The property that matters for retrieval: nothing is unreachable.
        // Held for both shapes, because the two take different paths through
        // the walk — one snaps every cut and the other snaps none.
        for total in [37, 100, 137, 400] {
            assert!(covered(&split(&flat(total)), total), "flat {total}");
        }
        let source = structured(9, 14);
        let lines = source.lines().count();
        assert!(covered(&split(&source), lines), "structured");
    }

    #[test]
    fn the_last_chunk_ends_at_the_last_line() {
        assert_eq!(split(&flat(95)).last().unwrap().end_line, 95);
        let source = structured(7, 20);
        assert_eq!(
            split(&source).last().unwrap().end_line,
            source.lines().count()
        );
    }

    #[test]
    fn line_numbers_are_the_files_own_so_a_result_can_be_opened() {
        assert_eq!(
            split(&flat(100))[0].start_line,
            1,
            "1-based, like an editor"
        );
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
    fn a_chunk_never_repeats_the_one_before_it_entirely() {
        // Striding past the end of the file would emit a window made only of
        // lines already covered — a request paid for nothing.
        for total in [41, 45, 60, 71, 90, 200] {
            for source in [flat(total), structured(total / 12 + 1, 9)] {
                let chunks = split(&source);
                for pair in chunks.windows(2) {
                    let [first, second] = pair else {
                        unreachable!()
                    };
                    assert!(
                        second.end_line > first.end_line,
                        "chunk {}-{} adds nothing to {}-{}",
                        second.start_line,
                        second.end_line,
                        first.start_line,
                        first.end_line
                    );
                }
            }
        }
    }

    #[test]
    fn a_forced_cut_overlaps_and_a_snapped_one_does_not() {
        // The whole trade. A cut made mid-structure leaves a passage half in
        // each chunk, so the chunks share lines; a cut at a real boundary has
        // a different top-level thing on the other side of it, and repeating
        // ten lines of that into this chunk would put the head of the next
        // function into the vector for this one.
        let forced = split(&flat(200));
        assert!(forced.len() > 2);
        for pair in forced.windows(2) {
            let [first, second] = pair else {
                unreachable!()
            };
            assert!(
                second.start_line <= first.end_line,
                "a forced cut lost its overlap"
            );
        }

        let snapped = split(&structured(12, 14));
        assert!(snapped.len() > 2);
        for pair in snapped.windows(2) {
            let [first, second] = pair else {
                unreachable!()
            };
            assert_eq!(
                second.start_line,
                first.end_line + 1,
                "a snapped cut overlapped"
            );
        }
    }

    #[test]
    fn a_cut_lands_on_a_definition_rather_than_inside_one() {
        // Items of 16 lines against a 40-line target: the line count would cut
        // in the middle of the third one every time.
        let source = structured(10, 14);
        let lines: Vec<&str> = source.lines().collect();
        for chunk in split(&source).iter().skip(1) {
            let first = lines[chunk.start_line - 1];
            assert!(
                first.starts_with("///"),
                "chunk starts at {:?}, mid-item",
                first
            );
        }
    }

    #[test]
    fn a_cut_does_not_move_further_than_the_snap_window() {
        // A nudge onto a nearby seam, not a search for the best split in the
        // file — one enormous function must not drag a cut far from where the
        // size budget wanted it. Items of 42 lines against a 40-line target
        // put every boundary out of reach, so every cut here is forced.
        for chunk in split(&structured(6, 40)) {
            let length = chunk.end_line - chunk.start_line + 1;
            assert!(
                length <= CHUNK_LINES + SNAP_LINES,
                "a chunk of {length} lines escaped the snap window"
            );
        }
    }

    #[test]
    fn a_file_with_no_boundaries_cuts_exactly_where_it_used_to() {
        // The fallback, and the point that answers the old objection about
        // one: it is not a second code path, it is this one finding nothing to
        // snap to. Windows of CHUNK_LINES, striding by CHUNK_LINES - OVERLAP.
        let chunks = split(&flat(200));
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (1, CHUNK_LINES));
        assert_eq!(
            chunks[1].start_line,
            1 + CHUNK_LINES - OVERLAP_LINES,
            "the stride moved"
        );
    }

    #[test]
    fn a_top_level_chunk_has_nothing_enclosing_it() {
        let source = "pub fn first(input: &str) -> usize {\n    input.len()\n}\n\n                      pub fn second(input: &str) -> bool {\n    input.is_empty()\n}\n";
        let chunks = split(source);
        assert_eq!(chunks.len(), 1, "the fixture should clear MIN_CHUNK_CHARS");
        assert_eq!(chunks[0].heading, Vec::<String>::new());
    }

    #[test]
    fn a_chunk_inside_a_definition_carries_it() {
        // The case the whole heading exists for: forty lines from the middle
        // of a long `impl` are a body with no signature above them.
        let mut source = String::from("impl Host {\n    pub fn build_agent(&self) -> Agent {\n");
        for n in 0..80 {
            source.push_str(&format!("        let step_{n} = {n};\n"));
        }
        source.push_str("    }\n}\n");

        let chunks = split(&source);
        assert!(chunks.len() > 1, "the fixture should span chunks");
        let inside = &chunks[1];
        assert_eq!(
            inside.heading,
            vec![
                "impl Host {".to_string(),
                "    pub fn build_agent(&self) -> Agent {".to_string(),
            ],
            "outermost first"
        );
    }

    #[test]
    fn a_heading_reads_indentation_rather_than_any_language() {
        // Python has no braces to count. It has the same layout.
        let mut source = String::from("class Loader:\n    def read(self, path):\n");
        for n in 0..80 {
            source.push_str(&format!("        step_{n} = {n}\n"));
        }

        let chunks = split(&source);
        assert_eq!(
            chunks[1].heading,
            vec![
                "class Loader:".to_string(),
                "    def read(self, path):".to_string(),
            ]
        );
    }

    #[test]
    fn a_heading_stops_at_three_levels() {
        // Past three the heading is longer than some of the bodies it
        // describes, and starts competing with them for the vector.
        let mut source = String::from("mod outer {\n");
        for depth in 1..7 {
            source.push_str(&format!("{}level_{depth} {{\n", "    ".repeat(depth)));
        }
        let indent = "    ".repeat(7);
        for n in 0..80 {
            source.push_str(&format!("{indent}let step_{n} = {n};\n"));
        }

        let chunks = split(&source);
        assert!(chunks[1].heading.len() <= HEADING_DEPTH);
    }

    #[test]
    fn a_closing_brace_is_not_something_a_chunk_is_inside() {
        // It is the end of something that ended before this chunk began.
        let mut source =
            String::from("fn before() {\n    done();\n}\n\nimpl Thing {\n    fn method(&self) {\n");
        for n in 0..80 {
            source.push_str(&format!("        let step_{n} = {n};\n"));
        }

        let chunks = split(&source);
        let inside = chunks.iter().find(|c| !c.heading.is_empty()).unwrap();
        assert!(
            !inside.heading.iter().any(|line| line.trim() == "}"),
            "{:?}",
            inside.heading
        );
    }

    #[test]
    fn what_is_embedded_names_the_file_and_is_not_what_is_shown() {
        // The excerpt a hit shows is re-read from the file by line number, so
        // nothing the model reads has a line in it that the file does not. The
        // heading only ever reaches the vector.
        let mut source = String::from("impl Store {\n    fn save(&self) {\n");
        for n in 0..80 {
            source.push_str(&format!("        let step_{n} = {n};\n"));
        }
        let chunks = split(&source);
        let passage = chunks[1].passage("crates/taurus-index/src/store.rs");

        assert!(passage.starts_with("crates/taurus-index/src/store.rs\n"));
        assert!(passage.contains("impl Store {"));
        assert!(passage.ends_with(&chunks[1].text));
        assert!(!chunks[1].text.contains("crates/taurus-index"));
    }
}
