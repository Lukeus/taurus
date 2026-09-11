import { useLayoutEffect, useMemo, useRef } from "react";

import { paint } from "../lib/ink";

/**
 * Markdown, painted and wrapping.
 *
 * The third editor in the app on the same two-layer trick — a `<pre>` under a
 * `color: transparent` `<textarea>`, so selection, IME, spell-check, undo, `⌘A`
 * and find-on-page are all the browser's own over the real text. `SqlEditor`
 * argues that at length and `DocumentEditor` carried it up to a whole file.
 *
 * # Why this is a sibling of `DocumentEditor` rather than a flag on it
 *
 * Because of the one thing it does that the other cannot: it **wraps**.
 *
 * `DocumentEditor` never wraps, and the reason is load-bearing rather than
 * preference. A wrapped line occupies more than one row, and a gutter is one
 * number per row — so the numbers came apart from their lines at the first long
 * paragraph and stayed apart, 4 pointing at the second half of 3. That was found
 * by photographing a Markdown file, and the conclusion drawn there was that
 * prose which wants wrapping has the Preview.
 *
 * A note is prose *all the way down*. There is nothing in it a line number
 * refers to — the model does not point at line 40 of somebody's design note, the
 * selection does not report in lines, no chip on the composer repeats one. So
 * the gutter goes, and once it is gone wrapping costs nothing: two layers that
 * wrap identically stay aligned, because the browser is doing the wrapping in
 * both of them over the same text at the same width in the same font.
 *
 * Adding a `wrap` flag to `DocumentEditor` instead would mean one component
 * holding both the virtualized-with-a-gutter arithmetic and this, with the
 * invariant that made the gutter correct true in only one of its two modes.
 *
 * # Why nothing is virtualized, and what happens when a note is long
 *
 * `DocumentEditor` paints only the lines on screen, pushed down by a spacer as
 * tall as everything above — which is what makes a 4000-line file scroll like a
 * short one. That trick needs to know how tall a line is, and a wrapped line's
 * height is whatever the browser decided. The two are not combinable without
 * measuring every line on every keystroke.
 *
 * So this paints the whole note, and the honest consequence is a ceiling. Past
 * [`PAINT_LIMIT`] the colour is dropped and the text stays — a note that long is
 * still perfectly editable, just not tinted, which is a better failure than an
 * editor that stutters. `notebook::MAX_NOTE_BYTES` is the harder limit above
 * that, where the answer is that the thing is a file rather than a note.
 *
 * # Why the textarea grows instead of scrolling
 *
 * A scrollbar inside the textarea would narrow its content box, which changes
 * where its text wraps — and the `<pre>` underneath, with no scrollbar, would
 * wrap somewhere else. Every line after the first difference would sit off its
 * own text. So the textarea is always exactly as tall as its content and never
 * scrolls; the panel around it does. That also removes the scroll-offset
 * copying the other two editors need, because there is no offset.
 */

/**
 * Bytes past which the paint is dropped.
 *
 * A `<span>` per token over the whole note, since none of it can be windowed.
 * Around this size the DOM is large enough that a keystroke is visibly behind
 * the finger, and a note this long is one somebody is reading rather than
 * writing.
 */
export const PAINT_LIMIT = 64 * 1024;

export function ProseEditor({
  text,
  onChange,
  onBlur,
  placeholder,
}: {
  text: string;
  onChange: (text: string) => void;
  /** Saves now rather than waiting out the debounce — see `NotesPane`. */
  onBlur?: () => void;
  placeholder?: string;
}) {
  const box = useRef<HTMLTextAreaElement>(null);
  const runs = useMemo(
    () => (text.length > PAINT_LIMIT ? null : paint(text, "markdown")),
    [text],
  );

  // As tall as its content, measured after every change. `scrollHeight` is zero
  // in jsdom, so nothing about this is provable in a mount test — the notes
  // screenshot is its only check, which is the same standing the Data pane's
  // auto-sizing query box has.
  useLayoutEffect(() => {
    const el = box.current;
    if (el) fit(el);
  }, [text]);

  /*
   * And again whenever its width changes, which moves where every line wraps:
   * the canvas split opening, the rail dragged, the window resized. Measured on
   * a change of text alone, a narrowed note kept the height it had at the old
   * width — its last lines cut off below the box, and the painted layer drifting
   * off the text above it until the next keystroke.
   *
   * Width only. The height `fit` sets is itself a resize, and answering that
   * too would be a loop that only happens to settle.
   */
  useLayoutEffect(() => {
    const el = box.current;
    // Absent in jsdom, which has no layout for it to report on anyway.
    if (!el || typeof ResizeObserver === "undefined") return;
    let width = el.clientWidth;
    const watch = new ResizeObserver(() => {
      if (el.clientWidth === width) return;
      width = el.clientWidth;
      fit(el);
    });
    watch.observe(el);
    return () => watch.disconnect();
  }, []);

  return (
    <div className="prose-edit">
      {/* Behind the text, and hidden from a screen reader: the textarea over it
          holds the same characters, and a reader that saw both would read the
          note twice. */}
      {runs && (
        <pre className="prose-ink" aria-hidden="true">
          {runs.map((run, i) => (
            <span key={i} className={`ink-${run.kind}`}>
              {run.text}
            </span>
          ))}
          {/* A trailing newline has no glyph, so the painted layer ends one line
              short of the textarea and the last line of a note that ends in a
              blank one is painted nowhere. */}
          {"\n"}
        </pre>
      )}
      <textarea
        ref={box}
        className={`prose-input${runs ? "" : " plain"}`}
        value={text}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
        placeholder={placeholder}
        spellCheck
        // Prose, unlike code: the browser's own writing aids are wanted here and
        // are turned off in the other two editors for the opposite reason.
        autoCapitalize="sentences"
        autoCorrect="on"
      />
    </div>
  );
}

/** Makes the box exactly as tall as what is in it, at the width it has now. */
function fit(el: HTMLTextAreaElement) {
  el.style.height = "auto";
  el.style.height = `${el.scrollHeight}px`;
}
