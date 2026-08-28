import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { grammarFor, paint } from "../lib/ink";

/**
 * A file, painted and scrollable.
 *
 * The same two-layer trick `SqlEditor` uses — a `<pre>` under a
 * `color: transparent` `<textarea>` — carried up from a ten-line box to a whole
 * file. That decision is argued at length there and it survives the change of
 * scale for the same reasons: no editor dependency, and a real `<textarea>`
 * underneath, so selection, IME, spell-check, `⌘A`, and the browser's own
 * find-on-page all keep working without any of them being re-implemented.
 *
 * Two things had to be added to make it hold at this size.
 *
 * # Only what is on screen is painted
 *
 * `SqlEditor` inks its whole value on every keystroke, which is right for ten
 * lines and ruinous for four thousand: the scanner is linear, but the DOM it
 * produces is not free, and a file with a `<span>` per token has tens of
 * thousands of them. So the `<pre>` holds only the lines within [`OVERSCAN`] of
 * the viewport, pushed down by a spacer as tall as everything above it.
 *
 * The `<textarea>` still holds the whole text and is still the thing that
 * scrolls — which is the property that makes this safe. Nothing about the
 * caret, the selection, or the scrollbar is being simulated; the browser is
 * doing all of it over the real text, and the paint is a decoration that
 * follows along.
 *
 * # The gutter is a third layer, and it scrolls with them
 *
 * Line numbers cannot go in the `<pre>` — they would be inside the text the
 * `<textarea>` has to line up with, and every line would sit six characters to
 * the right of its own caret. So they are their own column, virtualized the
 * same way and offset by the same scroll.
 *
 * # Nothing wraps, and the gutter is why
 *
 * `SqlEditor` declines to wrap because a wrapped `SELECT` reads worse than a
 * scrolled one and because wrapping would make the caret's row unknowable
 * without measuring. Both hold here. What settled it was a third reason, found
 * by photographing a Markdown file with wrapping on: **a wrapped line occupies
 * more than one row, and a gutter is one number per row.** The numbers came
 * apart from their lines at the first long paragraph and stayed apart — 4
 * pointing at the second half of line 3, and worse further down. A line number
 * that is wrong is worse than no line number, because everything else here
 * speaks in them: the model points with them, the selection reports in them,
 * the chip on the composer repeats them.
 *
 * Fixing it inside a wrapped layout means measuring every line's height on
 * every keystroke, which gives up both the virtualization and the arithmetic.
 * So Source shows the file's real lines and scrolls sideways for the long ones,
 * and Markdown that somebody wants to *read* has a preview one click away —
 * which is the mode built for reading, and where wrapping belongs.
 *
 * # What is deliberately not here
 *
 * Folding, multiple cursors, bracket matching, and find-and-replace. Each is a
 * feature in its own right rather than a detail of this one, and the honest
 * shape of that decision is a list rather than a half-built version of each.
 * See `docs/known-gaps.md`.
 */
export function DocumentEditor({
  text,
  path,
  reveal,
  onSelect,
  readOnly = true,
}: {
  /** The file, whole. */
  text: string;
  /** What it is called, which is the only thing that picks the colours. */
  path: string;
  /**
   * Lines to scroll to and select, 1-based and inclusive.
   *
   * An object rather than a bare range so that being sent to the same lines
   * twice is two events — the model saying "look here" again should move the
   * view again, even when it is already there.
   */
  reveal: { from: number; to: number } | null;
  /**
   * What is highlighted, whenever that changes.
   *
   * Reported up rather than held here because it is not this component's
   * business: it travels to the model on the next message, which is a decision
   * the app makes and the editor only observes. `null` when the selection is
   * empty, which is most of the time.
   */
  onSelect: (selection: { from: number; to: number; text: string } | null) => void;
  readOnly?: boolean;
}) {
  const box = useRef<HTMLTextAreaElement>(null);
  const ghost = useRef<HTMLPreElement>(null);
  const rail = useRef<HTMLDivElement>(null);
  const [scroll, setScroll] = useState(0);
  const [height, setHeight] = useState(0);
  const [line, setLine] = useState(0);

  const grammar = useMemo(() => grammarFor(path), [path]);
  /*
   * Split once per document rather than once per paint. Every layer here needs
   * the same array — the gutter counts it, the window slices it, the reveal
   * indexes into it — and splitting a four-thousand-line file three times on
   * every scroll frame is the one cost in this component that would actually
   * show.
   */
  const lines = useMemo(() => text.split("\n"), [text]);

  /* The row height, measured from the box rather than assumed. Everything that
     turns a line number into a pixel goes through this. */
  const [row, setRow] = useState(DEFAULT_ROW);
  useLayoutEffect(() => {
    const area = box.current;
    if (!area) return;
    const measured = Number.parseFloat(getComputedStyle(area).lineHeight);
    // jsdom reports no computed line-height at all, so every test that mounts
    // this would otherwise divide by `NaN` and window nothing.
    if (Number.isFinite(measured) && measured > 0) setRow(measured);
  }, [text]);

  const view = useCallback(() => {
    const area = box.current;
    if (!area) return;
    setScroll(area.scrollTop);
    setHeight(area.clientHeight);
    if (ghost.current) ghost.current.scrollLeft = area.scrollLeft;
  }, []);

  useLayoutEffect(view, [view, text]);

  /*
   * The window of lines worth painting.
   *
   * Arithmetic, and it can be because one line is one row — see the note on
   * wrapping at the top of this file. Falls back to painting everything when
   * there is nothing to measure from, which is a browser that has not laid out
   * yet and is also every test: jsdom reports zero for every dimension, so
   * without this the window would be empty and nothing would paint at all.
   */
  const window_ = useMemo(() => {
    if (row <= 0 || height <= 0) return { from: 0, to: lines.length };
    const first = Math.max(0, Math.floor(scroll / row) - OVERSCAN);
    const count = Math.ceil(height / row) + OVERSCAN * 2;
    return { from: first, to: Math.min(lines.length, first + count) };
  }, [row, height, scroll, lines.length]);

  const painted = useMemo(
    // Joined back with the newlines that `split` removed, so what the scanner
    // sees is a run of real lines. A fenced block or a block comment that opens
    // above the window is a known and accepted cost of painting a slice: it
    // reopens at the top of the window rather than carrying its colour down
    // from wherever it started.
    () => paint(lines.slice(window_.from, window_.to).join("\n"), grammar),
    [lines, window_.from, window_.to, grammar],
  );

  /* Scrolls to what the model pointed at, and highlights it. */
  useEffect(() => {
    const area = box.current;
    if (!area || !reveal) return;
    const from = Math.max(0, Math.min(reveal.from - 1, lines.length - 1));
    const to = Math.max(from, Math.min(reveal.to - 1, lines.length - 1));
    const start = offsetOf(lines, from);
    const end = offsetOf(lines, to) + (lines[to]?.length ?? 0);

    area.focus({ preventScroll: true });
    area.setSelectionRange(start, end);
    // A third of the way down rather than at the very top: a passage flush
    // against the top edge reads as the start of the file, and what is above it
    // is usually what makes it make sense.
    area.scrollTop = Math.max(0, (from - Math.floor(area.clientHeight / row / 3)) * row);
    view();
    report(area);
    // `lines` deliberately absent: re-revealing on every edit would fight the
    // cursor. This fires when the model points somewhere, and then not again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reveal]);

  const report = useCallback(
    (area: HTMLTextAreaElement) => {
      const { selectionStart: start, selectionEnd: end } = area;
      setLine(lineAt(area.value, start));
      if (start === end) {
        onSelect(null);
        return;
      }
      onSelect({
        from: lineAt(area.value, start) + 1,
        to: lineAt(area.value, end) + 1,
        text: area.value.slice(start, end),
      });
    },
    [onSelect],
  );

  const total = lines.length;

  return (
    <div className="doc-editor">
      <div className="doc-gutter" aria-hidden ref={rail}>
        <div style={{ height: window_.from * row }} />
        {lines.slice(window_.from, window_.to).map((_, i) => {
          const n = window_.from + i;
          return (
            <div key={n} className={`doc-line-no${n === line ? " on" : ""}`}>
              {n + 1}
            </div>
          );
        })}
      </div>

      <div className="doc-scroll">
        <pre className="doc-ink" aria-hidden ref={ghost}>
          {/* Everything above the window, as height and nothing else. */}
          <div style={{ height: window_.from * row }} />
          {painted.map((run, i) => (
            <span key={i} className={`ink-${run.kind}`}>
              {run.text}
            </span>
          ))}
          {/* A `<pre>` swallows one trailing newline, so a file ending in one
              would paint a line short and every caret below it would sit off
              its own text. */}
          {"\n"}
          <div style={{ height: Math.max(0, total - window_.to) * row }} />
        </pre>

        <textarea
          ref={box}
          className="doc-input"
          value={text}
          readOnly={readOnly}
          spellCheck={false}
          aria-label={path}
          // Read-only for now, so nothing here changes the file. The handler
          // exists because React warns about a `value` with no `onChange`, and
          // saying so is better than reaching for `defaultValue` and quietly
          // giving up control of what is shown.
          onChange={() => {}}
          onScroll={view}
          onSelect={(e) => report(e.currentTarget)}
          onKeyUp={(e) => report(e.currentTarget)}
          onMouseUp={(e) => report(e.currentTarget)}
        />
      </div>
    </div>
  );
}

/**
 * Lines kept painted above and below the viewport.
 *
 * Enough that a flick of the wheel lands inside what is already drawn. Too few
 * and a fast scroll shows unpainted text for a frame; too many and the saving
 * this exists for goes away. Forty is roughly one screen either side.
 */
const OVERSCAN = 40;

/** Row height before anything has been measured. Only ever briefly true. */
const DEFAULT_ROW = 20;

/** The character offset where line `n` starts, counting from 0. */
function offsetOf(lines: string[], n: number): number {
  let at = 0;
  for (let i = 0; i < n && i < lines.length; i++) at += lines[i].length + 1;
  return at;
}

/** Which line an offset falls on, counting from 0. */
function lineAt(text: string, offset: number): number {
  let n = 0;
  for (let i = 0; i < offset && i < text.length; i++) {
    if (text[i] === "\n") n++;
  }
  return n;
}
