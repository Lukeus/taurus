import { useEffect, useRef, useState } from "react";

import type { DataTable } from "../lib/api";
import { ink, suggest, type Suggestion } from "../lib/sql";

/**
 * The box you write a query in.
 *
 * A textarea with the query painted behind it, rather than an editor component.
 * That is the load-bearing decision here and it is worth the paragraph: this
 * box holds ten lines of SQL, and the libraries that do this properly —
 * CodeMirror, Monaco — are between a quarter and several megabytes of parser,
 * virtualized rendering and extension machinery for a surface where none of it
 * pays. The dock already carries the largest thing the frontend imports; a
 * second one for a ten-line box would be the tail wagging the app.
 *
 * What is given up is real and small: selection ranges do not tint, and the
 * highlighting is a scanner rather than a grammar. What is kept is that this
 * remains a plain `<textarea>` — so it is accessible, spell-check and IME work,
 * the browser's own undo stack works, and a paste is a paste.
 *
 * # How the two layers stay together
 *
 * The `<pre>` and the `<textarea>` sit exactly on top of each other with the
 * same font, size, line-height, padding and `white-space: pre`, and the scroll
 * offsets are copied across on every scroll. If a character ever appeared in
 * one and not the other the whole rest of the query would slide out of line —
 * which is why `ink` is tested on the property that it gives back exactly what
 * it was handed.
 *
 * Lines do not wrap, deliberately. A wrapped `SELECT` is harder to read than a
 * scrolled one, and it would also make the caret's position unknowable without
 * measuring the DOM — see `caretAt`, which is arithmetic precisely because the
 * font is monospace and every line is one line.
 */
export function SqlEditor({
  value,
  onChange,
  onRun,
  tables,
  placeholder,
  label = "SQL",
}: {
  value: string;
  onChange: (sql: string) => void;
  /** ⌘↵ and ⌃↵. Suppressed while the completion list is open, where Enter
   *  means "take this one". */
  onRun: () => void;
  /** Every table a query here can name, with its columns. Completion is only
   *  as good as this, which is why it is a schema read rather than a guess at
   *  what the datasets are called. */
  tables: DataTable[];
  /** Shown in an empty box. Worth naming a real table in — the shape of a
   *  query is easy and the name of the file is the part nobody has memorised. */
  placeholder?: string;
  label?: string;
}) {
  const box = useRef<HTMLTextAreaElement>(null);
  const ghost = useRef<HTMLPreElement>(null);
  const probe = useRef<HTMLSpanElement>(null);
  const [menu, setMenu] = useState<{
    items: Suggestion[];
    from: number;
    at: { left: number; top: number };
  } | null>(null);
  const [active, setActive] = useState(0);

  /*
   * The box takes the shape of what is in it.
   *
   * Four lines was the right default when everything here was typed, and the
   * wrong one the moment a card in the transcript started putting the model's
   * SQL in — a seven-line aggregate arrived clipped through the middle of a
   * `WHERE`, which reads as broken rather than as scrolled. The floor and the
   * ceiling are in the stylesheet; this measures what is between them.
   */
  useEffect(() => {
    const area = box.current;
    if (!area) return;
    // `auto` first, and that is the whole trick: `scrollHeight` on an element
    // already tall enough reports the height it has, so measuring without
    // collapsing it makes the box grow and never shrink.
    area.style.height = "auto";
    area.style.height = `${area.scrollHeight}px`;
  }, [value]);

  /**
   * Where the caret is on screen, in pixels from the box's top left.
   *
   * Arithmetic rather than a measured mirror element, and it can be because of
   * two properties this box holds on purpose: the font is monospace, so every
   * character is one cell wide, and lines do not wrap, so the row is the count
   * of newlines before the caret. One measurement — the probe — turns that into
   * pixels.
   *
   * `px` defaults anything unmeasurable to zero rather than propagating a
   * `NaN` into the layout. That is not only defensive: jsdom reports no
   * computed line-height at all, so every test that types into this box would
   * otherwise position the list at `NaNpx`.
   */
  const caretAt = (area: HTMLTextAreaElement) => {
    const cell = probe.current?.getBoundingClientRect();
    const style = getComputedStyle(area);
    const before = area.value.slice(0, area.selectionStart);
    const rows = before.split("\n");
    const column = rows[rows.length - 1].length;
    return {
      left:
        px(style.paddingLeft) +
        column * ((cell?.width ?? 0) / PROBE.length) -
        area.scrollLeft,
      // Below the line it is on, not on it, so the list hangs off the caret
      // rather than covering the word being typed.
      top: px(style.paddingTop) + rows.length * px(style.lineHeight) - area.scrollTop,
    };
  };

  const offer = (area: HTMLTextAreaElement) => {
    const { at, items } = suggest(area.value, area.selectionStart, tables);
    // Nothing to add is not the same as nothing to show. A single suggestion
    // that is already exactly what is typed is the finished case, and a list
    // hovering over a completed word is noise.
    const done = items.length === 1 && items[0].insert === at.prefix;
    setMenu(
      items.length === 0 || done
        ? null
        : { items, from: at.from, at: caretAt(area) },
    );
    setActive(0);
  };

  const take = (choice: Suggestion) => {
    const area = box.current;
    if (!area || !menu) return;
    const next =
      value.slice(0, menu.from) + choice.insert + value.slice(area.selectionStart);
    const caret = menu.from + choice.insert.length;
    onChange(next);
    setMenu(null);
    // After React has written the new value, or the caret is set on the old
    // one and lands wherever the length difference puts it.
    requestAnimationFrame(() => {
      area.focus();
      area.setSelectionRange(caret, caret);
    });
  };

  const painted = ink(value);

  return (
    <div className="sql-editor">
      {/* One character's worth of the exact font this box is set in, measured
          rather than assumed. Ten of them, because one character's width
          rounds badly at 12.5px and a tenth of a pixel of drift is a whole
          character by column eighty. */}
      <span className="sql-probe" aria-hidden ref={probe}>
        {PROBE}
      </span>

      <pre className="sql-ink" aria-hidden ref={ghost}>
        {painted.map((run, i) => (
          <span key={i} className={`sql-${run.kind}`}>
            {run.text}
          </span>
        ))}
        {/* A `<pre>` swallows one trailing newline, so a query ending in one
            would paint a line short and the caret would sit below its own
            text. */}
        {"\n"}
      </pre>

      <textarea
        ref={box}
        className="sql-input"
        value={value}
        spellCheck={false}
        rows={4}
        placeholder={placeholder}
        aria-label={label}
        onChange={(e) => {
          onChange(e.target.value);
          offer(e.target);
        }}
        onScroll={(e) => {
          const area = e.currentTarget;
          if (ghost.current) {
            ghost.current.scrollTop = area.scrollTop;
            ghost.current.scrollLeft = area.scrollLeft;
          }
          // The list is placed against the caret, and the caret has just
          // moved relative to the box. Cheaper to close it than to chase it,
          // and scrolling mid-completion is not a thing anybody does on
          // purpose.
          setMenu(null);
        }}
        // A click or an arrow key is a decision to be somewhere else. The list
        // is about the word being typed, and one left behind over a different
        // word is worse than no list.
        onBlur={() => setMenu(null)}
        onKeyDown={(e) => {
          // Before the menu, always. ⌘↵ means run, and it has to mean that
          // whether or not a list happens to be open.
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            setMenu(null);
            onRun();
            return;
          }

          // Asks for the list without typing another letter, which is what
          // every editor binds it to. Useful at the start of a line and after
          // a `.` that has already been typed.
          if (e.key === " " && e.ctrlKey) {
            e.preventDefault();
            offer(e.currentTarget);
            return;
          }

          if (menu) {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              const step = e.key === "ArrowDown" ? 1 : -1;
              setActive((n) => (n + step + menu.items.length) % menu.items.length);
              return;
            }
            if (e.key === "Enter" || e.key === "Tab") {
              e.preventDefault();
              take(menu.items[active]);
              return;
            }
            if (e.key === "Escape") {
              e.preventDefault();
              setMenu(null);
              return;
            }
            if (e.key.startsWith("Arrow") || e.key === "Home" || e.key === "End") {
              setMenu(null);
              return;
            }
          }
          // Enter with no menu is a newline. This is SQL, and a query worth
          // writing is more than one line.
        }}
      />

      {menu && (
        <ul
          className="sql-menu"
          style={{ left: menu.at.left, top: menu.at.top }}
          // The textarea's `blur` fires before a click lands, so the list
          // would close out from under the pointer. Taking the press rather
          // than the click means focus never leaves in the first place.
          onMouseDown={(e) => e.preventDefault()}
        >
          {menu.items.map((item, i) => (
            <li key={`${item.kind}-${item.note}-${item.insert}`}>
              <button
                className={`sql-choice${i === active ? " on" : ""}`}
                onMouseEnter={() => setActive(i)}
                onClick={() => take(item)}
              >
                <span className={`sql-tag ${item.kind}`}>{MARK[item.kind]}</span>
                <span className="sql-label">{item.label}</span>
                {/* The join hint. Two files that share a column are two files
                    that can be joined on it, and this is the cheapest place
                    anybody will ever notice that — while writing the join,
                    rather than by opening two profiles side by side. */}
                {item.shared && (
                  <span className="sql-shared" data-tip="More than one table has this column">
                    joins
                  </span>
                )}
                <span className="sql-note">{item.note}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** A computed length in pixels, or zero where there is nothing to read. */
function px(value: string): number {
  const n = Number.parseFloat(value);
  return Number.isFinite(n) ? n : 0;
}

/** Ten characters of the box's own font, for measuring one. */
const PROBE = "0000000000";

/** What each kind of suggestion wears, so the list can be scanned by shape. */
const MARK: Record<Suggestion["kind"], string> = {
  table: "▦",
  column: "│",
  fn: "ƒ",
  keyword: "·",
};
