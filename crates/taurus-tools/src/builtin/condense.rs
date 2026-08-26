//! Making a long stream shorter without making it a summary.
//!
//! A command's output is cut at a byte count, and a byte count is indifferent
//! to what it is cutting. That is the wrong instrument for the output an agent
//! actually runs into: a test run is thousands of passing lines around a
//! handful of failing ones, a dev server is the same warning forty times, and
//! keeping the first two thirds of either keeps mostly the part nobody needed.
//!
//! What lives here runs before the cut, so that the cut usually does not have
//! to happen at all. The rule every filter follows is that nothing is *summarized*
//! — a line either survives as itself or is replaced by a sentence saying
//! exactly what stood there. A model reading the result can tell the two apart,
//! which is the difference between a shorter stream and a less trustworthy one.
//!
//! Three filters, run in order, each over what the last one left. A `cargo
//! test` run that compiles with warnings wants all three.
//!
//! Which filter applies is decided by reading the output, not by parsing the
//! command line. The command line is what a proxy outside the harness would
//! have to go on, and it is the weaker signal: `cargo test` behind a pipeline,
//! a test binary in `target/debug/deps` invoked directly, and a log being
//! `cat`ed are all the same output and none of them says "cargo test" in a
//! place worth matching. libtest announces itself — `running 404 tests` — and
//! that announcement is both easier to recognize and harder to be wrong about.

/// Below this a stream is handed over exactly as it arrived.
///
/// Almost every command an agent runs is under it, and for those the model
/// should see what a terminal would have shown rather than something this file
/// had an opinion about. The threshold exists so that the opinion is only
/// applied where the alternative is losing output to the cut.
const CONDENSE_BYTES: usize = 16 * 1024;

/// How many lines a collapse must replace before it is worth making.
///
/// Two is not a run, it is a coincidence — and collapsing a pair costs a line
/// to save a line. Three is the smallest number where the marker is shorter
/// than what it stands in for.
const RUN_THRESHOLD: usize = 3;

/// A shorter stream, or `None` if there was nothing worth doing.
///
/// `None` rather than a copy so the caller can hand the original along
/// untouched, and so "nothing was collapsed" and "something was" stay
/// distinguishable at the call site.
pub(super) fn condense(text: &str) -> Option<String> {
    if text.len() <= CONDENSE_BYTES {
        return None;
    }
    // In order, each over what the last one left. They do not overlap: a
    // passing test is not an identical line and neither is a diagnostic, so
    // none of them can do another's work, and a `cargo test` run that compiles
    // with warnings wants all three.
    let mut shortened: Option<String> = None;
    for filter in [passing_tests, repeated_diagnostics, dedupe] {
        if let Some(next) = filter(shortened.as_deref().unwrap_or(text)) {
            shortened = Some(next);
        }
    }
    shortened
}

/// Drops the body of a compiler warning that has already been shown in full.
///
/// A Rust diagnostic is a headline, a location, the source it is about, and a
/// suggestion — seven or eight lines for one warning. A rename that touches two
/// hundred call sites prints that block two hundred times, and the second one
/// taught the reader everything the two hundredth will.
///
/// So a repeat keeps its headline and its location and loses the rest. Every
/// warning still appears, and still says where it is; what goes is the part
/// that was the same as the part above it.
///
/// **Errors are never touched.** They are what the command was run to find out,
/// there are rarely many, and an abbreviated one is worth less than no
/// abbreviation at all.
///
/// Grouping by lint name would be better and is not available: clippy names its
/// lint in a `help:` line on every occurrence, but rustc names its own in a
/// `#[warn(...)]` note it prints only the first time, so half the warnings in a
/// build carry no lint to group by. The headline is what every block has.
fn repeated_diagnostics(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut collapsed = false;
    let mut seen: Vec<&str> = Vec::new();
    let mut block: Vec<&str> = Vec::new();

    for line in text.split_inclusive('\n') {
        if starts_diagnostic(line) && !block.is_empty() {
            collapsed |= flush_block(&mut out, &block, &mut seen);
            block.clear();
        }
        block.push(line);
    }
    collapsed |= flush_block(&mut out, &block, &mut seen);

    collapsed.then_some(out)
}

/// A `warning:`, `error:` or `error[E0308]:` at the start of a line.
///
/// Column zero matters: the source a diagnostic quotes is indented behind a
/// line number and a bar, so a program whose own text contains one of these
/// words cannot open a block by being quoted.
fn starts_diagnostic(line: &str) -> bool {
    let Some(rest) = line
        .strip_prefix("warning")
        .or_else(|| line.strip_prefix("error"))
    else {
        return false;
    };
    // `error[E0308]: ` as well as a bare `error: `. Anything else after the
    // word is some other word that happens to begin the same way.
    let rest = match rest.split_once(']') {
        Some((code, after))
            if code.starts_with('[')
                && code[1..].bytes().all(|b| b.is_ascii_alphanumeric())
                && code.len() > 1 =>
        {
            after
        }
        _ => rest,
    };
    rest.starts_with(": ")
}

/// Writes one diagnostic, whole or reduced to where it is.
fn flush_block<'a>(out: &mut String, block: &[&'a str], seen: &mut Vec<&'a str>) -> bool {
    let Some(headline) = block.first() else {
        return false;
    };
    // Anything that is not a diagnostic — cargo's progress lines, a linker's
    // complaint, whatever a build script printed — goes through untouched.
    let is_repeat = starts_diagnostic(headline)
        && !headline.starts_with("error")
        && block.iter().skip(1).any(|line| is_location(line))
        && seen.contains(headline);

    if !is_repeat {
        if starts_diagnostic(headline) && !seen.contains(headline) {
            seen.push(headline);
        }
        for line in block {
            out.push_str(line);
        }
        return false;
    }

    out.push_str(headline);
    for line in block.iter().skip(1).filter(|line| is_location(line)) {
        out.push_str(line);
    }
    // "not repeated" rather than "identical": two sites can share a headline
    // and suggest slightly different things, and the location above is there
    // so the difference can be gone and looked at.
    out.push_str("[… body not repeated; the first is above …]\n");
    true
}

/// The ` --> src/lib.rs:18:82` under a diagnostic's headline.
fn is_location(line: &str) -> bool {
    line.trim_start().starts_with("--> ")
}

/// Collapses the lines of a libtest run that say a test passed.
///
/// A test suite is the output an agent runs into most, and it is the worst
/// possible shape for a byte count: thousands of lines saying nothing happened,
/// around the handful saying something did. Cutting the middle out of one takes
/// the failures and keeps the passes.
///
/// Only the `... ok` lines go, and only between the header libtest prints and
/// the result line that closes the block. Everything else survives as it
/// arrived — the compiler's warnings, the `failures:` block with its panics and
/// backtraces, the counts at the end. The lines removed are exactly the ones
/// whose whole content is that there is nothing to say.
fn passing_tests(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut collapsed = false;
    // Set by libtest's own header, so this cannot fire on a grep whose hits
    // happen to look like test results. Nothing outside a run is touched.
    let mut in_run = false;
    let mut passing: Vec<&str> = Vec::new();

    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if in_run && is_passing(trimmed) {
            passing.push(line);
            continue;
        }
        collapsed |= flush_passing(&mut out, &mut passing);
        if is_run_header(trimmed) {
            in_run = true;
        } else if trimmed.starts_with("test result:") {
            in_run = false;
        }
        out.push_str(line);
    }
    collapsed |= flush_passing(&mut out, &mut passing);

    collapsed.then_some(out)
}

/// `running 404 tests`, or `running 1 test`, and nothing else.
fn is_run_header(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("running ") else {
        return false;
    };
    let Some(count) = rest
        .strip_suffix(" tests")
        .or_else(|| rest.strip_suffix(" test"))
    else {
        return false;
    };
    !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit())
}

/// `test some::name ... ok`.
///
/// Deliberately not `ignored`: there are few of them and which tests did not
/// run is worth reading. `FAILED` lines stay for the same reason they always
/// would.
fn is_passing(line: &str) -> bool {
    line.starts_with("test ") && line.ends_with(" ... ok")
}

/// Writes the passing lines held so far, as a count if there are enough of
/// them to be worth one.
fn flush_passing(out: &mut String, passing: &mut Vec<&str>) -> bool {
    let collapsed = passing.len() >= RUN_THRESHOLD;
    if collapsed {
        out.push_str(&format!(
            "[… {} passing tests not shown …]\n",
            passing.len()
        ));
    } else {
        for line in passing.iter() {
            out.push_str(line);
        }
    }
    passing.clear();
    collapsed
}

/// Collapses runs of identical lines into one and a count.
fn dedupe(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut collapsed = false;
    // The line as it arrived, including its newline, and how many times it has
    // arrived in a row.
    let mut run: Option<(&str, usize)> = None;

    // `split_inclusive` rather than `lines` so each line keeps its own
    // terminator: a stream that came back through this should differ from the
    // one that went in only where something was collapsed, and `lines` would
    // quietly rewrite every CRLF on the way past.
    for line in text.split_inclusive('\n') {
        run = match run {
            Some((previous, count)) if repeats(previous, line) => Some((previous, count + 1)),
            Some((previous, count)) => {
                collapsed |= flush(&mut out, previous, count);
                Some((line, 1))
            }
            None => Some((line, 1)),
        };
    }
    if let Some((previous, count)) = run {
        collapsed |= flush(&mut out, previous, count);
    }

    collapsed.then_some(out)
}

/// Whether two lines are the same line twice.
///
/// Compared with trailing whitespace ignored, which is what makes a CRLF
/// stream and an LF stream behave alike, and what stops a trailing space from
/// splitting a run of forty identical warnings into two runs of twenty.
///
/// Nothing here strips terminal escapes, and it does not need to: the pty path
/// has already stripped them by the time output reaches this, and a piped
/// command is talking to something it has been told is not a terminal. A
/// program that colours anyway colours each repetition identically, so the runs
/// still match.
fn repeats(previous: &str, line: &str) -> bool {
    previous.trim_end() == line.trim_end()
}

/// Writes one line, or one line and the count of the ones just like it.
///
/// Returns whether anything was actually collapsed, which is how the caller
/// learns there was a point to any of this.
fn flush(out: &mut String, line: &str, count: usize) -> bool {
    out.push_str(line);
    if count < RUN_THRESHOLD {
        // Under the threshold the repeats are written out as they were, so a
        // pair of identical lines still reads as a pair.
        for _ in 1..count {
            out.push_str(line);
        }
        return false;
    }
    if !line.ends_with('\n') {
        out.push('\n');
    }
    // Bracketed the way every other thing this harness says inside a command's
    // output is, so a model reading a log can tell the harness's sentence from
    // the program's.
    out.push_str(&format!(
        "[… the line above repeated {} more times …]\n",
        count - 1
    ));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Padding that puts a fixture over the threshold without itself repeating.
    fn over_threshold() -> String {
        let padding: String = (0..CONDENSE_BYTES / 8)
            .map(|i| format!("distinct line {i} of padding\n"))
            .collect();
        assert!(padding.len() > CONDENSE_BYTES);
        padding
    }

    /// A libtest run of `passing` passes around one failure, shaped the way
    /// `cargo test` really prints it.
    fn test_run(passing: usize) -> String {
        let mut text = String::from(
            "   Compiling taurus-tools v0.2.0 (/w/crates/taurus-tools)\n\
             warning: unused variable: `x`\n\
             \x20   Finished `test` profile [unoptimized + debuginfo] target(s) in 40.92s\n\
             \x20    Running unittests src/lib.rs (target/debug/deps/taurus_tools-918be)\n\
             \n\
             running 404 tests\n",
        );
        for i in 0..passing {
            text.push_str(&format!("test some::module::case_{i} ... ok\n"));
        }
        text.push_str(
            "test some::module::the_broken_one ... FAILED\n\
             \n\
             failures:\n\
             \n\
             ---- some::module::the_broken_one stdout ----\n\
             thread 'the_broken_one' panicked at crates/taurus-tools/src/lib.rs:12:5:\n\
             assertion `left == right` failed\n\
             \x20 left: 1\n\
             \x20right: 2\n\
             \n\
             \n\
             failures:\n\
             \x20   some::module::the_broken_one\n\
             \n\
             test result: FAILED. 404 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; \
             finished in 3.07s\n",
        );
        text
    }

    #[test]
    fn a_test_run_keeps_its_failure_and_counts_its_passes() {
        let text = test_run(2_000);
        let out = condense(&text).expect("a run this size is worth collapsing");

        assert!(out.contains("[… 2000 passing tests not shown …]"), "{out}");
        assert!(!out.contains("case_1500"), "a pass should not survive");
        assert!(
            out.len() * 20 < text.len(),
            "{} of {}",
            out.len(),
            text.len()
        );
    }

    /// The point of the filter. Every one of these lines is the reason the
    /// command was run, and the byte cut it replaces would have taken the lot.
    #[test]
    fn everything_that_is_not_a_pass_survives_byte_for_byte() {
        let text = test_run(2_000);
        let out = condense(&text).unwrap();

        for kept in [
            "warning: unused variable: `x`",
            "running 404 tests",
            "test some::module::the_broken_one ... FAILED",
            "---- some::module::the_broken_one stdout ----",
            "thread 'the_broken_one' panicked at crates/taurus-tools/src/lib.rs:12:5:",
            "assertion `left == right` failed",
            "  left: 1",
            " right: 2",
            "test result: FAILED. 404 passed; 1 failed;",
        ] {
            assert!(out.contains(kept), "lost {kept:?} from:\n{out}");
        }
    }

    /// Without libtest's own header these are just lines. A grep for passing
    /// tests is a thing somebody asked for, not a thing to collapse.
    #[test]
    fn test_shaped_lines_outside_a_run_are_left_alone() {
        let mut text = over_threshold();
        for i in 0..500 {
            text.push_str(&format!("test some::module::case_{i} ... ok\n"));
        }
        assert!(condense(&text).is_none(), "nothing announced a test run");
    }

    /// Two passes are not worth a sentence saying there were two passes.
    #[test]
    fn a_run_of_one_or_two_passes_is_written_out_as_it_was() {
        let text = format!(
            "{}running 2 tests\ntest a ... ok\ntest b ... ok\n",
            over_threshold()
        );
        assert!(condense(&text).is_none(), "{text}");
    }

    /// The result line closes the block, so what follows it is ordinary output
    /// again — a second suite's compile warnings, a doc-test section.
    #[test]
    fn passes_after_the_result_line_are_no_longer_inside_a_run() {
        let text = format!(
            "{}running 3 tests\ntest a ... ok\ntest b ... ok\ntest c ... ok\n\
             test result: ok. 3 passed;\ntest d ... ok\ntest e ... ok\ntest f ... ok\n",
            over_threshold()
        );
        let out = condense(&text).unwrap();
        assert!(out.contains("[… 3 passing tests not shown …]"), "{out}");
        // The three after the result line announced no run of their own.
        assert!(
            out.contains("test d ... ok\ntest e ... ok\ntest f ... ok\n"),
            "{out}"
        );
    }

    /// An ignored test is a fact about the suite, and there are never many.
    #[test]
    fn ignored_tests_are_not_collapsed_away() {
        let mut text = format!("{}running 900 tests\n", over_threshold());
        for i in 0..900 {
            text.push_str(&format!("test case_{i} ... ok\n"));
        }
        text.push_str("test the_skipped_one ... ignored\n");
        let out = condense(&text).unwrap();
        assert!(out.contains("test the_skipped_one ... ignored"), "{out}");
    }

    /// One clippy warning, shaped the way it really prints, at `line`.
    fn warning_at(line: usize) -> String {
        format!(
            "warning: the loop variable `j` is only used to index `v`\n\
             \x20 --> src/lib.rs:{line}:82\n\
             \x20  |\n\
             {line} | pub fn g() {{ for j in 0..v.len() {{ s.push(v[j]); }} }}\n\
             \x20  |                    ^^^^^^^^^^\n\
             \x20  |\n\
             \x20  = help: for further information visit \
             https://rust-lang.github.io/rust-clippy/index.html#needless_range_loop\n\
             \n"
        )
    }

    #[test]
    fn a_warning_seen_before_keeps_its_headline_and_its_place() {
        // The padding has no repeats of any kind, so it passes through
        // untouched and what it costs can be taken back off both sides.
        let padding = over_threshold();
        let warnings: String = (1..=40).map(warning_at).collect();
        let text = format!("{padding}{warnings}");
        let out = condense(&text).expect("forty of the same warning is worth shortening");

        // The first one is intact, suggestion and all.
        assert!(out.contains("#needless_range_loop"), "{out}");
        // And every later site is still named, with its body gone.
        for line in 2..=40 {
            assert!(
                out.contains(&format!("--> src/lib.rs:{line}:82")),
                "lost site {line}"
            );
        }
        assert_eq!(out.matches("body not repeated").count(), 39);
        assert_eq!(
            out.matches("^^^^^^^^^^").count(),
            1,
            "only one body survives"
        );
        let shortened = out.len() - padding.len();
        assert!(
            shortened * 2 < warnings.len(),
            "{shortened} of {}",
            warnings.len()
        );
    }

    /// An error is what the command was run to find out. There are rarely many
    /// and an abbreviated one is worth less than no abbreviation.
    #[test]
    fn errors_are_never_shortened_however_often_they_repeat() {
        let one = "error[E0308]: mismatched types\n \
                   --> src/lib.rs:1:1\n  |\n  = note: expected `u8`, found `i32`\n\n";
        let text = format!("{}{}", over_threshold(), one.repeat(30));
        let out = condense(&text).unwrap_or_else(|| text.clone());
        assert!(!out.contains("body not repeated"), "{out}");
        assert_eq!(out.matches("expected `u8`, found `i32`").count(), 30);
    }

    /// Cargo's own summary begins with the same word and is not a diagnostic:
    /// no location under it, so nothing to send a reader to.
    #[test]
    fn a_warning_with_no_location_is_left_whole() {
        let summary = "warning: `warny` (lib) generated 27 warnings\n";
        let text = format!("{}{}", over_threshold(), summary.repeat(20));
        let out = condense(&text).unwrap();
        assert!(!out.contains("body not repeated"), "{out}");
        // It is an identical line, so the general filter has it instead.
        assert!(out.contains("repeated 19 more times"), "{out}");
    }

    #[test]
    fn what_opens_a_diagnostic_and_what_only_looks_like_one() {
        assert!(starts_diagnostic("warning: unused variable: `x`"));
        assert!(starts_diagnostic("error: could not compile `warny`"));
        assert!(starts_diagnostic("error[E0308]: mismatched types"));
        // A quoted source line carries its own number and bar in front.
        assert!(!starts_diagnostic("18 | warning: not a diagnostic"));
        assert!(!starts_diagnostic("  --> src/lib.rs:1:1"));
        assert!(!starts_diagnostic("warnings: 3"));
        assert!(!starts_diagnostic("errors happened"));
        // A bracket that is not an error code leaves the line as it was, and
        // it still opens a block on its colon.
        assert!(starts_diagnostic("error: expected `]`, found `,`"));
    }

    #[test]
    fn a_short_stream_is_left_alone() {
        let text = "same\n".repeat(50);
        assert!(condense(&text).is_none());
    }

    #[test]
    fn a_run_becomes_one_line_and_a_count() {
        let text = format!("{}before\n{}after\n", over_threshold(), "same\n".repeat(40));
        let out = condense(&text).expect("a run this long is worth collapsing");

        assert!(out.contains("before\nsame\n[… the line above repeated 39 more times …]\nafter\n"));
        // The line itself survives as itself. Nothing here paraphrases.
        assert_eq!(out.matches("same").count(), 1);
        assert!(out.len() < text.len());
    }

    #[test]
    fn a_stream_that_never_repeats_is_handed_back_untouched() {
        assert!(condense(&over_threshold()).is_none());
    }

    /// Collapsing a pair costs a line to save a line.
    #[test]
    fn a_pair_is_not_a_run() {
        let text = format!("{}twice\ntwice\nend\n", over_threshold());
        assert!(condense(&text).is_none(), "a pair should be left alone");
    }

    #[test]
    fn separate_runs_are_counted_separately() {
        let text = format!(
            "{}{}other\n{}",
            over_threshold(),
            "a\n".repeat(5),
            "a\n".repeat(7)
        );
        let out = condense(&text).unwrap();
        assert!(out.contains("repeated 4 more times"), "{out}");
        assert!(out.contains("repeated 6 more times"), "{out}");
    }

    /// A run at the very end has no following line to trigger the flush, and
    /// forgetting it is the obvious way to write this wrong.
    #[test]
    fn a_run_that_ends_the_stream_is_still_collapsed() {
        let text = format!("{}{}", over_threshold(), "tail\n".repeat(9));
        let out = condense(&text).unwrap();
        assert!(
            out.ends_with("tail\n[… the line above repeated 8 more times …]\n"),
            "{out}"
        );
    }

    /// The last line of a stream need not be terminated, and the marker has to
    /// start on its own line regardless.
    #[test]
    fn a_run_with_no_final_newline_still_gets_its_own_line() {
        let text = format!("{}{}unterminated", over_threshold(), "x\n".repeat(4));
        let out = condense(&text).unwrap();
        assert!(out.ends_with("unterminated"), "{out}");
        assert!(
            out.contains("x\n[… the line above repeated 3 more times …]\n"),
            "{out}"
        );
    }

    /// Line endings are the program's business, not this file's.
    #[test]
    fn crlf_survives_where_nothing_is_collapsed() {
        let text = format!(
            "{}kept\r\nalso kept\r\n{}",
            over_threshold(),
            "r\n".repeat(4)
        );
        let out = condense(&text).unwrap();
        assert!(out.contains("kept\r\nalso kept\r\n"), "{out}");
    }
}
