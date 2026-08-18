//! Tables, charts and diagrams, drawn for a terminal.
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

use taurus_tools::view::{
    ColumnKind, FlowEdge, FlowStage, MessageKind, SequenceMessage, Series, StepState,
    TranscriptView,
};

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

        // Lanes and arrows, the same shape the app draws. A sequence diagram is
        // the one picture a scrollback can hold honestly: its layout is the
        // payload's own order rather than anything measured, so nothing is lost
        // by drawing it in characters instead of pixels.
        TranscriptView::Sequence {
            title,
            caption,
            participants,
            messages,
        } => {
            let mut out = heading(title, caption.as_deref(), color);
            out.push_str(&sequence(participants, messages, color));
            out
        }

        // The stages, then the arrows.
        //
        // Not the boxes-and-lines the app draws, and deliberately. A sequence
        // diagram's layout survives being drawn in characters because it is a
        // grid; a graph's does not — routing arbitrary edges between arbitrary
        // rows in a character cell needs either crossings a reader cannot
        // follow or a canvas a scrollback has not got. So the terminal gets the
        // two facts the picture is made of, in a form that is complete, greppable
        // and pastes into an issue: what is at each depth, and what points at
        // what.
        TranscriptView::Flow {
            title,
            caption,
            stages,
            edges,
        } => {
            let mut out = heading(title, caption.as_deref(), color);
            out.push_str(&flow(stages, edges, color));
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

/// Characters one lane occupies, gap included.
///
/// Wide enough that the shortest arrow still reads as an arrow rather than as
/// two brackets touching. Grown to fit the longest participant name, so the
/// header never has to be truncated — the names are what every arrow is read
/// against.
const LANE_MIN: usize = 13;

/// Draws the lanes and the arrows between them.
///
/// A character grid rather than string concatenation, because every row has to
/// put a lifeline at each lane's centre *and* an arrow across some span of them,
/// and those two overlap. Built as a row of spaces, stamped with the lifelines,
/// then stamped again with the arrow — so the arrow wins wherever it lands,
/// which is exactly the rule the picture needs.
///
/// The arrowheads are `<` and `>` rather than the geometric pointers they would
/// otherwise want to be. Everything else drawn here is box-drawing (the U+2500
/// block), which every terminal renders one cell wide. The pointers are
/// Geometric Shapes, which are East-Asian-ambiguous: a terminal set for a CJK
/// locale gives them two cells, and every lane to the right of an arrow shifts
/// by one. A picture whose alignment is the whole point cannot be built on a
/// character whose width is a setting — and this output is meant to survive
/// being pasted somewhere else.
fn sequence(participants: &[String], messages: &[SequenceMessage], color: bool) -> String {
    let widest = participants
        .iter()
        .map(|p| p.chars().count())
        .max()
        .unwrap_or(0);
    let lane = LANE_MIN.max(widest + 3);
    let centre = |i: usize| i * lane + lane / 2;
    // A whole lane past the last centre, so the rightmost name has room to sit
    // centred rather than being clipped by the edge of the grid. Trailing
    // spaces come off every row before it is printed.
    let width = participants.len() * lane;

    let index = |name: &String| participants.iter().position(|p| p == name);

    let mut out = String::new();

    // The header, each name centred on the lane it labels.
    let mut header = vec![' '; width];
    for (i, name) in participants.iter().enumerate() {
        let chars: Vec<char> = name.chars().collect();
        let start = centre(i).saturating_sub(chars.len() / 2);
        for (n, ch) in chars.iter().enumerate() {
            if let Some(slot) = header.get_mut(start + n) {
                *slot = *ch;
            }
        }
    }
    out.push_str(&format!("  {}\n", collect(&header)));
    out.push_str(&dim(
        &format!(
            "  {}\n",
            collect(&lifelines(participants.len(), width, lane))
        ),
        color,
    ));

    for message in messages {
        let (Some(from), Some(to)) = (index(&message.from), index(&message.to)) else {
            // Unreachable through the tool, which refuses an arrow naming a
            // participant it never declared. A transcript hand-edited past that
            // check still must not lose the message: the label carries the
            // meaning, and a row without its arrow is better than no row.
            out.push_str(&format!(
                "  {} → {}: {}\n",
                message.from, message.to, message.text
            ));
            continue;
        };

        let dash = match message.kind {
            MessageKind::Call => '─',
            MessageKind::Return => '┄',
        };

        if from == to {
            // Work a participant does on its own, drawn as a turn back into the
            // same lane. Two rows, because one cannot both leave and arrive.
            let mut top = lifelines(participants.len(), width, lane);
            let mut bottom = lifelines(participants.len(), width, lane);
            stamp(&mut top, centre(from), '├');
            stamp(&mut top, centre(from) + 1, dash);
            stamp(&mut top, centre(from) + 2, '╮');
            stamp(&mut bottom, centre(from), '<');
            stamp(&mut bottom, centre(from) + 1, dash);
            stamp(&mut bottom, centre(from) + 2, '╯');
            out.push_str(&row(&top, &message.text, color));
            out.push_str(&row(&bottom, "", color));
            continue;
        }

        let mut line = lifelines(participants.len(), width, lane);
        let (left, right) = (centre(from.min(to)), centre(from.max(to)));
        for x in left + 1..right {
            stamp(&mut line, x, dash);
        }
        // The tail is a tee into the lane it leaves; the head is the arrow.
        if from < to {
            stamp(&mut line, left, '├');
            stamp(&mut line, right, '>');
        } else {
            stamp(&mut line, right, '┤');
            stamp(&mut line, left, '<');
        }
        out.push_str(&row(&line, &message.text, color));
    }

    out
}

/// A row with a lifeline at every lane's centre and nothing else.
fn lifelines(count: usize, width: usize, lane: usize) -> Vec<char> {
    let mut row = vec![' '; width];
    for i in 0..count {
        stamp(&mut row, i * lane + lane / 2, '│');
    }
    row
}

/// Writes one character, ignoring anything that would land off the grid.
fn stamp(row: &mut [char], at: usize, ch: char) {
    if let Some(slot) = row.get_mut(at) {
        *slot = ch;
    }
}

/// One drawn row and the label that belongs to it.
///
/// The label sits to the right of the whole grid rather than over the arrow it
/// belongs to: an arrow between two adjacent lanes is eleven characters wide,
/// and a label written into that would be cut to nothing.
fn row(cells: &[char], label: &str, color: bool) -> String {
    let drawn = dim(&format!("  {}", collect(cells)), color);
    if label.is_empty() {
        return format!("{drawn}\n");
    }
    format!("{drawn}  {label}\n")
}

fn collect(cells: &[char]) -> String {
    let text: String = cells.iter().collect();
    text.trim_end().to_string()
}

/// Draws a flow diagram as its stages and then its arrows.
///
/// The arrow list is aligned into a column so the `from` and `to` names line up
/// down the page, which is what lets a reader scan for one node and find every
/// connection it has without reading the rest.
fn flow(stages: &[FlowStage], edges: &[FlowEdge], color: bool) -> String {
    let mut out = String::new();

    for (n, stage) in stages.iter().enumerate() {
        // Numbered when unnamed, so a reader can still say which depth a box is
        // at — that ordering is the thing the model was asked to decide.
        let name = stage
            .name
            .clone()
            .unwrap_or_else(|| format!("stage {}", n + 1));
        out.push_str(&dim(&format!("  {name}\n"), color));
        for node in &stage.nodes {
            let note = node
                .note
                .as_deref()
                .filter(|n| !n.trim().is_empty())
                .map(|n| format!("  ({n})"))
                .unwrap_or_default();
            out.push_str(&format!("    {}{}\n", node.label, dim(&note, color)));
        }
    }

    if edges.is_empty() {
        return out;
    }
    out.push('\n');

    // Where each node sits, so an arrow that goes back to an earlier stage can
    // be marked as one. A loop is the thing a reader most needs pointed out
    // here: on the page it is visibly a loop, and in a list it is just a line.
    let depth = |label: &String| {
        stages
            .iter()
            .position(|stage| stage.nodes.iter().any(|node| &node.label == label))
    };

    let width = edges
        .iter()
        .map(|e| e.from.chars().count() + e.to.chars().count())
        .max()
        .unwrap_or(0)
        + 5;

    for edge in edges {
        let pair = format!("{} ──> {}", edge.from, edge.to);
        let pad = width.saturating_sub(edge.from.chars().count() + edge.to.chars().count());
        let label = edge.label.as_deref().filter(|l| !l.trim().is_empty());
        let back = match (depth(&edge.from), depth(&edge.to)) {
            (Some(from), Some(to)) if to <= from => "  (loops back)",
            _ => "",
        };
        match label {
            Some(label) => out.push_str(&format!(
                "  {pair}{:pad$}{}{}\n",
                "",
                label,
                dim(back, color),
                pad = pad
            )),
            None => out.push_str(&format!("  {pair}{}\n", dim(back, color))),
        }
    }

    out
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
                    active_form: None,
                },
                Step {
                    text: "Add the token type".into(),
                    state: StepState::Active,
                    active_form: None,
                },
                Step {
                    text: "Update the tests".into(),
                    state: StepState::Todo,
                    active_form: None,
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

    fn message(from: &str, to: &str, text: &str, kind: MessageKind) -> SequenceMessage {
        SequenceMessage {
            from: from.into(),
            to: to.into(),
            text: text.into(),
            kind,
        }
    }

    fn exchange() -> TranscriptView {
        TranscriptView::Sequence {
            title: "Placing an order".into(),
            caption: None,
            participants: vec!["Client".into(), "API".into(), "Store".into()],
            messages: vec![
                message("Client", "API", "POST /orders", MessageKind::Call),
                message("API", "API", "validate the body", MessageKind::Call),
                message("API", "Store", "insert row", MessageKind::Call),
                message("Store", "API", "ok", MessageKind::Return),
            ],
        }
    }

    /// Where a row puts every lane, by character offset.
    fn lanes(line: &str) -> Vec<usize> {
        line.chars()
            .enumerate()
            .filter(|(_, c)| "\u{2502}\u{251c}\u{2524}<>".contains(*c))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn a_sequence_keeps_every_lane_in_the_same_column() {
        // The whole readability of the picture. A lane that wandered by one
        // column would make an arrow look like it landed somewhere it did not,
        // and every arrow is read against the header names above it.
        let out = render(&exchange(), false);
        let drawn: Vec<&str> = out
            .lines()
            .filter(|l| l.contains('\u{2502}') || l.contains('\u{251c}'))
            .collect();

        assert!(drawn.len() >= 5, "{out}");
        let first = lanes(drawn[0]);
        assert_eq!(first.len(), 3, "three participants, three lanes: {out}");
        for line in &drawn {
            for column in lanes(line) {
                assert!(
                    first.contains(&column) || column == first[0] + 1 || column == first[0] + 2,
                    "a mark at column {column} is in no lane:\n{out}"
                );
            }
        }
    }

    #[test]
    fn an_answer_coming_back_is_drawn_differently_from_the_call() {
        // The turn-around is the one thing a sequence diagram exists to show.
        // Drawn identically, the picture says a request went both ways.
        let out = render(&exchange(), false);
        let call = out.lines().find(|l| l.contains("insert row")).unwrap();
        let reply = out.lines().find(|l| l.ends_with("ok")).unwrap();

        assert!(
            call.contains('\u{2500}') && !call.contains('\u{2504}'),
            "{call:?}"
        );
        assert!(
            reply.contains('\u{2504}') && !reply.contains('\u{2500}'),
            "{reply:?}"
        );
        // And they point opposite ways.
        assert!(call.contains('>'), "{call:?}");
        assert!(reply.contains('<'), "{reply:?}");
    }

    #[test]
    fn a_participant_talking_to_itself_turns_back_into_its_own_lane() {
        let out = render(&exchange(), false);
        let at = out
            .lines()
            .position(|l| l.contains("validate the body"))
            .unwrap();
        let lines: Vec<&str> = out.lines().collect();

        // Two rows: one leaving, one arriving. One row cannot do both.
        assert!(lines[at].contains('\u{256e}'), "{:?}", lines[at]);
        assert!(lines[at + 1].contains('\u{256f}'), "{:?}", lines[at + 1]);
        assert!(lines[at + 1].contains('<'), "{:?}", lines[at + 1]);
    }

    #[test]
    fn a_sequence_is_drawn_without_escapes_when_uncoloured() {
        // What a piped run writes, and what gets pasted into an issue.
        let out = render(&exchange(), false);
        assert!(!out.contains('\u{1b}'), "{out:?}");
        assert!(out.contains("Client") && out.contains("Store"), "{out}");
    }

    #[test]
    fn the_arrowheads_are_the_ascii_ones() {
        // Geometric-shape pointers are East-Asian-ambiguous, so a CJK locale
        // renders them two cells wide and shifts every lane right of the arrow.
        let out = render(&exchange(), false);
        assert!(
            !out.contains('\u{25ba}') && !out.contains('\u{25c4}'),
            "{out:?}"
        );
    }

    fn request_path() -> TranscriptView {
        use taurus_tools::view::FlowNode;
        let node = |label: &str, note: Option<&str>| FlowNode {
            label: label.into(),
            note: note.map(str::to_string),
        };
        let edge = |from: &str, to: &str, label: Option<&str>| FlowEdge {
            from: from.into(),
            to: to.into(),
            label: label.map(str::to_string),
        };
        TranscriptView::Flow {
            title: "How a request reaches the database".into(),
            caption: None,
            stages: vec![
                FlowStage {
                    name: Some("Edge".into()),
                    nodes: vec![node("Client", None)],
                },
                FlowStage {
                    name: Some("Service".into()),
                    nodes: vec![node("API", Some("axum")), node("Worker", None)],
                },
                FlowStage {
                    name: None,
                    nodes: vec![node("Postgres", None)],
                },
            ],
            edges: vec![
                edge("Client", "API", Some("POST /orders")),
                edge("API", "Postgres", Some("insert row")),
                edge("Worker", "API", Some("retry")),
            ],
        }
    }

    #[test]
    fn a_flow_lists_its_stages_with_what_is_at_each_depth() {
        let out = render(&request_path(), false);
        assert!(out.contains("Edge"), "{out}");
        assert!(out.contains("Service"), "{out}");
        // A stage the model did not name still says which depth it is, because
        // the ordering is the thing it was asked to decide.
        assert!(out.contains("stage 3"), "{out}");
        assert!(out.contains("API"), "{out}");
        assert!(out.contains("(axum)"), "{out}");
    }

    #[test]
    fn a_flow_lists_every_edge_with_its_label() {
        let out = render(&request_path(), false);
        assert!(out.contains("Client \u{2500}\u{2500}> API"), "{out}");
        assert!(out.contains("POST /orders"), "{out}");
        assert!(out.contains("insert row"), "{out}");
    }

    #[test]
    fn an_edge_back_to_an_earlier_stage_is_marked_as_a_loop() {
        // On the page it is visibly a loop. In a list it is just another line,
        // so it has to be said.
        let out = render(&request_path(), false);
        let retry = out.lines().find(|l| l.contains("retry")).unwrap();
        assert!(retry.contains("loops back"), "{retry:?}");
        let forward = out.lines().find(|l| l.contains("insert row")).unwrap();
        assert!(!forward.contains("loops back"), "{forward:?}");
    }

    #[test]
    fn a_flow_is_drawn_without_escapes_when_uncoloured() {
        let out = render(&request_path(), false);
        assert!(!out.contains('\u{1b}'), "{out:?}");
    }
}
