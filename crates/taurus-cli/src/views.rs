//! Tables and charts, drawn for a terminal.
//!
//! The desktop app gets a sortable table and a chart with tabs. A terminal gets
//! neither, and pretending otherwise — a full-screen pager, a redrawn chart on
//! keypress — would break the one property that makes CLI output useful, which
//! is that it can be scrolled back to, piped, and pasted into an issue.
//!
//! So this draws once, statically, and picks the arrangement that survives
//! that: rows in the order they were sent, and a horizontal bar chart, because
//! vertical bars need a height a scrollback does not have and a width every
//! label has to fit inside.

use taurus_tools::view::{ColumnKind, Series, StepState, TranscriptView};

/// Longest a single table cell may print before it is cut.
///
/// A path or a message can be arbitrarily long, and one of them is enough to
/// wrap every row in an 80-column terminal into three, which costs more than
/// the tail of the cell was worth.
const MAX_CELL: usize = 48;

/// Characters the longest bar takes. Leaves room for a label and a value on an
/// 80-column terminal without wrapping.
const BAR_WIDTH: usize = 32;

pub fn render(view: &TranscriptView, color: bool) -> String {
    match view {
        TranscriptView::Table {
            title,
            caption,
            columns,
            rows,
        } => {
            let widths = widths(columns.len(), rows, columns);
            let mut out = heading(title, caption.as_deref(), color);

            let header: Vec<String> = columns
                .iter()
                .enumerate()
                .map(|(i, c)| pad(&c.label, widths[i], c.kind != ColumnKind::Text))
                .collect();
            out.push_str(&dim(&format!("  {}\n", header.join("  ")), color));
            out.push_str(&dim(
                &format!(
                    "  {}\n",
                    widths
                        .iter()
                        .map(|w| "─".repeat(*w))
                        .collect::<Vec<_>>()
                        .join("  ")
                ),
                color,
            ));

            for row in rows {
                let cells: Vec<String> = row
                    .iter()
                    .enumerate()
                    .map(|(i, cell)| {
                        let text = clip(cell);
                        let right = columns[i].kind != ColumnKind::Text;
                        let padded = pad(&text, widths[i], right);
                        match columns[i].kind {
                            ColumnKind::Delta => tint(&padded, delta_sign(cell), color),
                            _ => padded,
                        }
                    })
                    .collect();
                out.push_str(&format!("  {}\n", cells.join("  ")));
            }
            out
        }

        TranscriptView::Chart {
            title,
            caption,
            labels,
            series,
        } => {
            let mut out = heading(title, caption.as_deref(), color);
            // Every series is drawn, one block after another. The app offers
            // tabs; a scrollback cannot, and dropping all but the first would
            // lose data the model chose to send.
            for (n, s) in series.iter().enumerate() {
                if n > 0 {
                    out.push('\n');
                }
                if series.len() > 1 {
                    out.push_str(&dim(&format!("  {}\n", s.name), color));
                }
                out.push_str(&bars(labels, s, color));
            }
            out
        }

        // A checklist needs neither columns nor scale, so this is the one view
        // the terminal draws as well as the app does. The markers are the
        // model's own — `[x]`, `[>]`, `[ ]` — so a plan read here and a plan
        // read in the transcript say the same thing in the same characters.
        TranscriptView::Plan { steps } => {
            let mut out = String::new();
            for (index, step) in steps.iter().enumerate() {
                let line = format!("  {} {} {}\n", index + 1, step.state.marker(), step.text);
                out.push_str(&match step.state {
                    // The step being worked on is the one fact worth finding at
                    // a glance in a scrollback; the finished ones are context.
                    StepState::Active => bold(&line, color),
                    StepState::Done => dim(&line, color),
                    StepState::Todo => line,
                });
            }
            out
        }

        // Drawn by the prompt that is about to ask them, not here.
        TranscriptView::Questions { .. } => String::new(),
    }
}

fn bold(text: &str, color: bool) -> String {
    if color {
        format!("\x1b[1m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn heading(title: &str, caption: Option<&str>, color: bool) -> String {
    let mut out = if color {
        format!("  \x1b[1m{title}\x1b[0m\n")
    } else {
        format!("  {title}\n")
    };
    if let Some(caption) = caption.filter(|c| !c.trim().is_empty()) {
        out.push_str(&dim(&format!("  {caption}\n"), color));
    }
    out.push('\n');
    out
}

fn bars(labels: &[String], series: &Series, color: bool) -> String {
    // Bars are drawn against the largest value, not against zero-to-max, so a
    // series that never dips still shows its shape.
    let max = series.values.iter().cloned().fold(0.0_f64, f64::max);
    let label_width = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);

    let mut out = String::new();
    for (i, label) in labels.iter().enumerate() {
        let value = series.values.get(i).copied().unwrap_or(0.0);
        let filled = if max > 0.0 {
            ((value / max) * BAR_WIDTH as f64).round() as usize
        } else {
            0
        };
        // A non-zero value always gets at least one block: a bar that rounds to
        // nothing reads as no data rather than as a small number.
        let filled = if filled == 0 && value > 0.0 {
            1
        } else {
            filled
        };
        out.push_str(&format!(
            "  {:>label_width$}  {}{}  {}\n",
            label,
            tint(&"█".repeat(filled), 1, color),
            " ".repeat(BAR_WIDTH - filled),
            dim(&format!("{}{}", number(value), series.unit), color),
        ));
    }
    out
}

/// `42.1` rather than `42.1000000001`, and `318` rather than `318.0`.
///
/// Values arrive as f64 because JSON has one number type, so a count and a
/// duration are indistinguishable by the time they get here. Printing to one
/// decimal and dropping a trailing `.0` covers both without asking the model to
/// declare which it sent.
fn number(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// Column widths, bounded by [`MAX_CELL`] and never narrower than the header.
fn widths(
    count: usize,
    rows: &[Vec<String>],
    columns: &[taurus_tools::view::Column],
) -> Vec<usize> {
    let mut widths: Vec<usize> = columns.iter().map(|c| c.label.chars().count()).collect();
    widths.resize(count, 0);
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(count) {
            widths[i] = widths[i].max(clip(cell).chars().count());
        }
    }
    widths
}

fn clip(cell: &str) -> String {
    if cell.chars().count() <= MAX_CELL {
        return cell.to_string();
    }
    let head: String = cell.chars().take(MAX_CELL - 1).collect();
    format!("{head}…")
}

fn pad(text: &str, width: usize, right: bool) -> String {
    let gap = width.saturating_sub(text.chars().count());
    if right {
        format!("{}{text}", " ".repeat(gap))
    } else {
        format!("{text}{}", " ".repeat(gap))
    }
}

/// `-1`, `0`, or `1` for a delta cell, read off the text the model formatted.
///
/// The cell is already written for a human — `+22%`, `-8`, `—` — so the sign is
/// found rather than computed. Anything that does not start with a sign is
/// treated as no change, which is what an em dash means.
fn delta_sign(cell: &str) -> i8 {
    match cell.trim().chars().next() {
        Some('+') => 1,
        Some('-' | '−') => -1,
        _ => 0,
    }
}

fn dim(text: &str, color: bool) -> String {
    if color {
        format!("\x1b[2m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Green for down, red for up, plain for neither.
///
/// The direction is deliberately not "up is good": every delta this draws is a
/// cost — build time, error count, bundle size — and the one place the app uses
/// the same colours reads them the same way.
fn tint(text: &str, sign: i8, color: bool) -> String {
    if !color || sign == 0 {
        return text.to_string();
    }
    let code = if sign > 0 { 31 } else { 32 };
    format!("\x1b[{code}m{text}\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;
    use taurus_tools::view::Column;

    fn table() -> TranscriptView {
        TranscriptView::Table {
            title: "Crates by build time".into(),
            caption: Some("cargo build --timings".into()),
            columns: vec![
                Column {
                    label: "Crate".into(),
                    kind: ColumnKind::Text,
                },
                Column {
                    label: "Time".into(),
                    kind: ColumnKind::Number,
                },
                Column {
                    label: "Δ".into(),
                    kind: ColumnKind::Delta,
                },
            ],
            rows: vec![
                vec!["taurus-core".into(), "42.1s".into(), "-8%".into()],
                vec!["taurus-mcp".into(), "18.4s".into(), "—".into()],
            ],
        }
    }

    #[test]
    fn a_table_lines_its_columns_up() {
        let out = render(&table(), false);
        let lines: Vec<&str> = out.lines().collect();

        assert_eq!(lines[0], "  Crates by build time");
        assert_eq!(lines[1], "  cargo build --timings");
        // Header, rule, then one line per row, every one the same width.
        let widths: Vec<usize> = lines[3..].iter().map(|l| l.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "ragged columns: {lines:#?}"
        );
    }

    #[test]
    fn numbers_sit_right_and_words_sit_left() {
        let out = render(&table(), false);
        let row = out.lines().find(|l| l.contains("taurus-mcp")).unwrap();

        // `taurus-core` is the wider name, so the shorter one is padded after
        // it; `18.4s` is the same width as `42.1s`, so alignment shows in the
        // header instead — "Time" is narrower than either value.
        assert!(row.starts_with("  taurus-mcp   "), "{row:?}");
        let header = out.lines().find(|l| l.contains("Time")).unwrap();
        assert!(header.contains(" Time "), "{header:?}");
    }

    #[test]
    fn an_overlong_cell_is_cut_rather_than_wrapped() {
        let long = "x".repeat(MAX_CELL + 20);
        let view = TranscriptView::Table {
            title: "t".into(),
            caption: None,
            columns: vec![Column {
                label: "Path".into(),
                kind: ColumnKind::Text,
            }],
            rows: vec![vec![long]],
        };

        let out = render(&view, false);

        assert!(out.contains('…'));
        assert!(
            out.lines().all(|l| l.chars().count() < MAX_CELL + 10),
            "{out}"
        );
    }

    #[test]
    fn every_bar_is_drawn_against_the_largest_value() {
        let view = TranscriptView::Chart {
            title: "Tool calls per turn".into(),
            caption: None,
            labels: vec!["t1".into(), "t2".into()],
            series: vec![Series {
                name: "tool calls".into(),
                unit: String::new(),
                values: vec![4.0, 8.0],
            }],
        };

        let out = render(&view, false);
        let bars: Vec<usize> = out
            .lines()
            .filter(|l| l.contains('█'))
            .map(|l| l.chars().filter(|c| *c == '█').count())
            .collect();

        assert_eq!(bars, vec![BAR_WIDTH / 2, BAR_WIDTH]);
    }

    #[test]
    fn a_small_but_real_value_still_gets_a_bar() {
        // Rounding a genuine measurement down to an empty line would report it
        // as missing data, which is a different claim entirely.
        let view = TranscriptView::Chart {
            title: "t".into(),
            caption: None,
            labels: vec!["a".into(), "b".into()],
            series: vec![Series {
                name: "s".into(),
                unit: String::new(),
                values: vec![1.0, 10_000.0],
            }],
        };

        let out = render(&view, false);
        let first = out
            .lines()
            .find(|l| l.trim_start().starts_with('a'))
            .unwrap();

        assert!(first.contains('█'), "{first:?}");
    }

    #[test]
    fn whole_numbers_do_not_grow_a_decimal_point() {
        assert_eq!(number(318.0), "318");
        assert_eq!(number(42.1), "42.1");
    }

    #[test]
    fn a_plan_prints_its_steps_with_the_markers_the_model_uses() {
        // The terminal and the transcript have to agree character for
        // character: someone reading a piped run and someone reading the app
        // are looking at the same checklist.
        use taurus_tools::view::Step;
        let view = TranscriptView::Plan {
            steps: vec![
                Step {
                    text: "Read the parser".into(),
                    state: StepState::Done,
                },
                Step {
                    text: "Add the token type".into(),
                    state: StepState::Active,
                },
                Step {
                    text: "Update the tests".into(),
                    state: StepState::Todo,
                },
            ],
        };

        let out = render(&view, false);
        assert!(out.contains("1 [x] Read the parser"), "{out}");
        assert!(out.contains("2 [>] Add the token type"), "{out}");
        assert!(out.contains("3 [ ] Update the tests"), "{out}");
        // Uncoloured output must carry no escapes at all — it is what a piped
        // run writes, and what ends up pasted into an issue.
        assert!(!out.contains('\x1b'), "{out:?}");
    }
}
