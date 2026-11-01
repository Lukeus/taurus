import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";

import { cutAtHits, findAll, hitFrom, lineAsked, MOST_HITS, type FindKey } from "../lib/find";
import { enterEdit, indentOf, outdentEdit, replaceText, tabEdit, type Edit } from "../lib/indent";
import { grammarFor, paint, sliceRuns } from "../lib/ink";
import { isComposing } from "../lib/keys";
import { FindBar } from "./FindBar";
import { InkLayer } from "./InkLayer";

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
 * Folding, multiple cursors, bracket matching, and replace. Each is a
 * feature in its own right rather than a detail of this one, and the honest
 * shape of that decision is a list rather than a half-built version of each.
 * See `docs/known-gaps.md`.
 */
export function DocumentEditor({
  text,
  path,
  reveal,
  flash,
  ask = null,
  onSelect,
  onChange,
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
  /**
   * Lines somebody else just changed, to tint briefly.
   *
   * The difference between the model editing your document and the text
   * blinking. An object rather than a range so two edits to the same lines are
   * two flashes.
   */
  flash: { from: number; to: number } | null;
  /** A find key, for the bar. An object so pressing it twice is two events. */
  ask?: { key: FindKey } | null;
  onSelect: (selection: { from: number; to: number; text: string } | null) => void;
  /** What was typed. Absent while the document is not editable. */
  onChange?: (text: string) => void;
}) {
  const box = useRef<HTMLTextAreaElement>(null);
  const ghost = useRef<HTMLPreElement>(null);
  const rail = useRef<HTMLDivElement>(null);
  /** The visible box, which fades its right edge while lines run past it. */
  const port = useRef<HTMLDivElement>(null);
  const [scroll, setScroll] = useState(0);
  const [height, setHeight] = useState(0);
  const [line, setLine] = useState(0);
  /* Bumped whenever a flash arrives, so the element's key changes and the
     animation runs again even when the same lines changed twice. */
  const [flashes, setFlashes] = useState(0);
  useEffect(() => {
    if (flash) setFlashes((n) => n + 1);
  }, [flash]);

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
    // Both ways, and the gutter too. Only the box has a scrollbar; the other
    // two are `overflow: hidden` and are moved here to the same place. Moving
    // only the paint sideways left both of them at the top of the file while
    // the box scrolled down, so past the first screen the window of painted
    // lines sat below what either layer showed, and the editor read as blank.
    if (ghost.current) {
      ghost.current.scrollTop = area.scrollTop;
      ghost.current.scrollLeft = area.scrollLeft;
    }
    if (rail.current) rail.current.scrollTop = area.scrollTop;
    // A line that runs past the edge has more to it, and says so: the edge
    // fades while there is anything further right. Without it a long line read
    // as cut off, because the scrollbar that says otherwise is hidden until
    // somebody scrolls. An attribute rather than state, so scrolling sideways
    // repaints nothing.
    const more = area.scrollWidth - area.scrollLeft - area.clientWidth > 1;
    if (port.current) port.current.toggleAttribute("data-more", more);
  }, []);

  useLayoutEffect(view, [view, text]);

  // And again when the panel changes size: dragging the split changes how
  // many lines fit and whether any run past the edge, with no scroll to say so.
  useEffect(() => {
    const area = box.current;
    if (!area || typeof ResizeObserver === "undefined") return;
    const watch = new ResizeObserver(view);
    watch.observe(area);
    return () => watch.disconnect();
  }, [view]);

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

  /*
   * Put both layers back where the box is after every repaint of the window,
   * not only on a scroll event. Moving the window rewrites what is above the
   * viewport in both layers, and a scroll position set before that rewrite is
   * not guaranteed to survive it.
   */
  useLayoutEffect(() => {
    const area = box.current;
    if (!area) return;
    if (ghost.current) ghost.current.scrollTop = area.scrollTop;
    if (rail.current) rail.current.scrollTop = area.scrollTop;
  }, [window_.from, window_.to]);

  /*
   * The whole file, scanned once per edit, so a comment or docstring that opens
   * above the window still colours what's inside it. Past `SCAN_WHOLE` a scan is a
   * keystroke's worth of lag, and such a file is painted by the window alone: a
   * block that opens above it reopens at the top. See `docs/known-gaps.md`.
   */
  const whole = useMemo(
    () => (text.length <= SCAN_WHOLE ? paint(text, grammar) : null),
    [text, grammar],
  );
  const painted = useMemo(() => {
    if (!whole) return paint(lines.slice(window_.from, window_.to).join("\n"), grammar);
    const from = offsetOf(lines, window_.from);
    const to = from + lines.slice(window_.from, window_.to).join("\n").length;
    return sliceRuns(whole, from, to);
  }, [whole, lines, window_.from, window_.to, grammar]);

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

  /*
   * Tab and Enter, for code. Through the browser's own edit so ⌘Z takes each one
   * back. Not while an input method is composing, since those keys are its own.
   *
   * Tab stays in the box, so Escape is the way out for a keyboard: it leaves the
   * field, and Tab then moves on. See the Escape handling in `CanvasSlot`.
   */
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!onChange || isComposing(e) || e.altKey || e.ctrlKey || e.metaKey) return;
    const area = e.currentTarget;
    const { value, selectionStart: start, selectionEnd: end } = area;
    let edit: Edit | null = null;
    if (e.key === "Tab") {
      const unit = indentOf(value, grammar);
      edit = e.shiftKey ? outdentEdit(value, start, end, unit) : tabEdit(value, start, end, unit);
    } else if (e.key === "Enter" && !e.shiftKey) {
      edit = enterEdit(value, start, end, indentOf(value, grammar), grammar);
    }
    if (!edit) return;
    e.preventDefault();
    // Nothing to do, as Shift-Tab on a line with no indent is. Inserting the same
    // text would still be an edit for ⌘Z to walk back through.
    if (edit.insert !== value.slice(edit.from, edit.to)) {
      replaceText(area, edit.from, edit.to, edit.insert);
    }
    area.setSelectionRange(edit.start, edit.end);
    report(area);
  };

  /*
   * Find. Nothing is selected while the bar is open, only painted, because a
   * selection travels to the model with the next message and "lines 40–41" would
   * be a claim about whatever the search last passed. Closing the bar selects the
   * match it stopped on, which is where somebody meant to be.
   */
  const [finding, setFinding] = useState(false);
  const [asked, setAsked] = useState(0);
  const [query, setQuery] = useState("");
  const [current, setCurrent] = useState(-1);
  /** Where the caret was, for a search to begin from rather than the top. */
  const startedAt = useRef(0);
  const goingTo = lineAsked(query);
  const hits = useMemo(
    () => (finding && goingTo === null ? findAll(text, query) : []),
    [finding, goingTo, text, query],
  );
  const shown = hits.length === 0 ? -1 : Math.min(Math.max(current, 0), hits.length - 1);

  const lookAt = useCallback(
    (offset: number) => {
      const area = box.current;
      if (!area) return;
      const at = lineAt(text, offset);
      const rows = area.clientHeight / row;
      const top = area.scrollTop / row;
      if (at < top || at >= top + rows - 1) {
        area.scrollTop = Math.max(0, (at - Math.floor(rows / 3)) * row);
      }
      // Sideways too, since nothing wraps and a match can be far along its line.
      const column = offset - offsetOf(lines, at);
      const x = column * charWidth(area);
      if (x < area.scrollLeft || x > area.scrollLeft + area.clientWidth - 64) {
        area.scrollLeft = Math.max(0, x - area.clientWidth / 2);
      }
      view();
    },
    [text, lines, row, view],
  );

  const typeQuery = (next: string) => {
    setQuery(next);
    const found = lineAsked(next) === null ? findAll(text, next) : [];
    const at = hitFrom(found, startedAt.current);
    setCurrent(at);
    if (at >= 0) lookAt(found[at].from);
  };

  const move = (by: 1 | -1) => {
    if (hits.length === 0) return;
    const next = (shown + by + hits.length) % hits.length;
    setCurrent(next);
    lookAt(hits[next].from);
  };

  const closeFind = () => {
    const area = box.current;
    const hit = hits[shown];
    setFinding(false);
    if (!area) return;
    area.focus({ preventScroll: true });
    if (hit) {
      area.setSelectionRange(hit.from, hit.to);
      lookAt(hit.from);
      report(area);
    }
  };

  /** Enter in the box. Answers whether it was a line to go to. */
  const goToLine = () => {
    const area = box.current;
    if (goingTo === null) return false;
    const line = Math.min(goingTo, lines.length) - 1;
    setFinding(false);
    if (!area) return true;
    const start = offsetOf(lines, line);
    area.focus({ preventScroll: true });
    area.setSelectionRange(start, start);
    lookAt(start);
    report(area);
    return true;
  };

  useEffect(() => {
    if (!ask) return;
    const area = box.current;
    if (ask.key === "next" || ask.key === "previous") {
      if (finding && hits.length > 0) {
        move(ask.key === "next" ? 1 : -1);
        return;
      }
    }
    if (!finding && area) startedAt.current = area.selectionStart;
    if (ask.key === "line") setQuery(":");
    else if (area && area.selectionStart !== area.selectionEnd && !area.value.slice(area.selectionStart, area.selectionEnd).includes("\n")) {
      // What's highlighted is usually what's being looked for.
      typeQuery(area.value.slice(area.selectionStart, area.selectionEnd));
    } else if (lineAsked(query) !== null || query === ":") setQuery("");
    setFinding(true);
    setAsked((n) => n + 1);
    // Fires on the key and nothing else. What it reads is this render's.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ask]);

  const found = useMemo(
    () => cutAtHits(painted, offsetOf(lines, window_.from), hits, shown),
    [painted, lines, window_.from, hits, shown],
  );

  const said =
    goingTo !== null
      ? `Go to line ${Math.min(goingTo, lines.length)}`
      : !query
        ? null
        : hits.length === 0
          ? "No matches"
          : hits.length >= MOST_HITS
            ? `${shown + 1} of ${MOST_HITS.toLocaleString()}+`
            : `${shown + 1} of ${hits.length}`;

  const total = lines.length;

  return (
    <div className="doc-editor">
      {finding && (
        <FindBar
          query={query}
          said={said}
          asked={asked}
          onQuery={typeQuery}
          onMove={move}
          onSubmit={goToLine}
          onClose={closeFind}
        />
      )}
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
        {/* What is below the window, so the gutter is as tall as the file and
            can be scrolled as far as the box is. One row more than that, for
            the reason the paint ends on a newline: the box loses a row to its
            sideways scrollbar and can scroll that much further than a layer
            without one. */}
        <div style={{ height: (Math.max(0, total - window_.to) + 1) * row }} />
      </div>

      <div className="doc-scroll" ref={port}>
        {/*
          * What somebody else just changed, tinted for a moment.
          *
          * A band behind the text rather than a class on the painted runs,
          * because the runs are split by token and a changed *line* is not a
          * token boundary. Positioned in the same arithmetic everything else
          * here uses — a row is a line, so a span of lines is a rectangle —
          * and keyed on the flash so a second edit to the same lines restarts
          * the animation instead of being ignored as an unchanged element.
          */}
        {flash && (
          <div
            key={`${flash.from}-${flash.to}-${flashes}`}
            className="doc-flash"
            aria-hidden
            style={{
              top: (flash.from - 1) * row - scroll,
              height: (flash.to - flash.from + 1) * row,
            }}
          />
        )}

        <InkLayer
          runs={found}
          className="doc-ink"
          ghost={ghost}
          // What is above the window and below it, as height and nothing else.
          before={<div style={{ height: window_.from * row }} />}
          after={<div style={{ height: Math.max(0, total - window_.to) * row }} />}
        />

        <textarea
          ref={box}
          className="painted-input doc-input"
          value={text}
          spellCheck={false}
          aria-label={path}
          aria-description="Tab indents. Press Escape, then Tab, to leave the editor."
          onKeyDown={onKeyDown}
          onChange={(e) => onChange?.(e.target.value)}
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

/**
 * Characters past which a file isn't scanned whole on every edit.
 *
 * Measured on this repository's own source: about 30ms a scan at 1 MB, which is
 * as much of a frame budget as a keystroke can give up, and 130ms at 4 MB.
 */
const SCAN_WHOLE = 1024 * 1024;

/** Row height before anything has been measured. Only ever briefly true. */
const DEFAULT_ROW = 20;

/** How wide a character is in the box. The font is fixed-pitch, or columns mean nothing. */
function charWidth(area: HTMLTextAreaElement): number {
  const pen = document.createElement("canvas").getContext?.("2d");
  if (!pen) return 8;
  pen.font = getComputedStyle(area).font;
  return pen.measureText("M").width || 8;
}

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
