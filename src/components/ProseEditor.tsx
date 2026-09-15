import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { paint } from "../lib/ink";
import { carry, suggest, type Choice, type Offer } from "../lib/prose";
import { growToContent, InkLayer } from "./InkLayer";

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
 *
 * # Help with the syntax
 *
 * Two kinds, both decided in `lib/prose.ts` and both leaving the textarea a
 * textarea. A menu opens where there is syntax worth not remembering — `/` at
 * the start of a line, a fence's language, a sketch's embed line — and Enter on
 * a list item starts the next one. Everything either writes goes in through the
 * browser's own `insertText`, so **⌘Z takes it back in one step**, the same as
 * something typed. Setting the value instead would have been simpler and would
 * have emptied the undo history every time a list carried on.
 *
 * The menu hangs off the caret, and finding the caret is where wrapping costs
 * something. `SqlEditor` works it out with arithmetic, because its lines never
 * wrap and its font is one cell wide. Here a line breaks wherever the browser
 * decided, so the text up to the caret is laid out once more in a hidden copy
 * of the painted layer — the same width, font and wrapping, which is the whole
 * premise of the painted layer — and the end of it is where the caret is.
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

const NONE: readonly string[] = [];

export function ProseEditor({
  text,
  onChange,
  onBlur,
  placeholder,
  sketches = NONE,
  notes = NONE,
}: {
  text: string;
  onChange: (text: string) => void;
  /** Saves now rather than waiting out the debounce — see `NotesPane`. */
  onBlur?: () => void;
  placeholder?: string;
  /** The sketches in this note's notebook, which the menu offers to embed. */
  sketches?: readonly string[];
  /** The other notes in its notebook, which the menu offers to link to. */
  notes?: readonly string[];
}) {
  const box = useRef<HTMLTextAreaElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const [menu, setMenu] = useState<(Offer & { at: Caret }) | null>(null);
  const [active, setActive] = useState(0);
  /**
   * Set while this editor writes into its own textarea. The write arrives back
   * as an ordinary `input`, and the menu must not be worked out against the
   * caret halfway through it — the caret is placed once the write is in.
   */
  const placing = useRef(false);

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
    if (el) growToContent(el);
  }, [text]);

  /*
   * And again whenever its width changes, which moves where every line wraps:
   * the canvas split opening, the rail dragged, the window resized. Measured on
   * a change of text alone, a narrowed note kept the height it had at the old
   * width — its last lines cut off below the box, and the painted layer drifting
   * off the text above it until the next keystroke.
   *
   * Width only. The height `growToContent` sets is itself a resize, and
   * answering that too would be a loop that only happens to settle.
   */
  useLayoutEffect(() => {
    const el = box.current;
    // Absent in jsdom, which has no layout for it to report on anyway.
    if (!el || typeof ResizeObserver === "undefined") return;
    let width = el.clientWidth;
    const watch = new ResizeObserver(() => {
      if (el.clientWidth === width) return;
      width = el.clientWidth;
      growToContent(el);
    });
    watch.observe(el);
    return () => watch.disconnect();
  }, []);

  /*
   * Where the menu actually goes, once it has a size.
   *
   * Under the caret by default. Above it when there is not room below inside
   * the panel that scrolls — the last lines of a note are exactly where a new
   * block gets started, and a menu hanging off the bottom of the panel there
   * is a menu with its best rows cut off. And pulled left when it would run off
   * the right edge. Written to the element directly and before paint, so it
   * never shows in the wrong place first; React leaves it alone after, because
   * the position it was given has not changed.
   */
  useLayoutEffect(() => {
    const el = list.current;
    const area = box.current;
    if (!menu || !el || !area) return;
    const rect = el.getBoundingClientRect();
    const room = scrollerOf(area)?.getBoundingClientRect();
    const host = area.parentElement?.clientWidth ?? 0;
    if (room && rect.bottom > room.bottom) {
      // The caret's line, on screen: the list hangs `GAP` below its bottom.
      const lineTop = rect.top - GAP - menu.at.line;
      if (lineTop - GAP - rect.height >= room.top) {
        el.style.top = `${menu.at.top - menu.at.line - GAP - rect.height}px`;
      }
    }
    if (host > 0 && menu.at.left + rect.width > host - GAP) {
      el.style.left = `${Math.max(GAP, host - rect.width - GAP)}px`;
    }
  }, [menu]);

  // The row Enter will take stays in sight as the arrows walk past the edge of
  // a list taller than its box. Absent in jsdom, hence the guard.
  useEffect(() => {
    const row = list.current?.querySelectorAll<HTMLElement>(".prose-choice")[active];
    row?.scrollIntoView?.({ block: "nearest" });
  }, [active, menu]);

  const offer = (area: HTMLTextAreaElement, asked = false) => {
    // A selection is somebody about to act on the selection, not a word being
    // finished.
    const found =
      area.selectionStart === area.selectionEnd
        ? suggest(area.value, area.selectionStart, { sketches, notes }, asked)
        : null;
    setMenu(found && { ...found, at: caretAt(area) });
    setActive(0);
  };

  /** Writes `insert` over `from`–`to` and puts the caret or selection where it belongs. */
  const write = (area: HTMLTextAreaElement, from: number, to: number, insert: string, select: [number, number]) => {
    placing.current = true;
    try {
      replace(area, from, to, insert);
    } finally {
      placing.current = false;
    }
    area.setSelectionRange(select[0], select[1]);
  };

  const take = (choice: Choice) => {
    const area = box.current;
    if (!area || !menu) return;
    const { from, to } = menu;
    setMenu(null);
    const select: [number, number] = choice.select
      ? [from + choice.select[0], from + choice.select[1]]
      : [from + (choice.caret ?? choice.insert.length), from + (choice.caret ?? choice.insert.length)];
    write(area, from, to, choice.insert, select);
    // What was just written can be somewhere the menu opens on its own — a
    // code block's fence, waiting for its language.
    if (select[0] === select[1]) offer(area);
  };

  return (
    <div className="prose-edit">
      {/* Behind the text, and hidden from a screen reader: the textarea over it
          holds the same characters, and a reader that saw both would read the
          note twice. */}
      {runs && <InkLayer runs={runs} className="prose-ink" />}
      <textarea
        ref={box}
        className={`painted-input prose-input${runs ? "" : " plain"}`}
        value={text}
        onChange={(e) => {
          onChange(e.target.value);
          if (!placing.current) offer(e.target);
        }}
        onBlur={() => {
          // The list is about the word being typed. See `SqlEditor`.
          setMenu(null);
          onBlur?.();
        }}
        // A click puts the caret somewhere else, and a list left over the old
        // spot is worse than none.
        onPointerDown={() => setMenu(null)}
        onKeyDown={(e) => {
          // Mid-composition, every key belongs to the input method.
          if (e.nativeEvent.isComposing) return;
          const area = e.currentTarget;

          // The list without typing the `/`, the key every editor binds it to.
          if (e.key === " " && e.ctrlKey) {
            e.preventDefault();
            offer(area, true);
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

          // The next list item, or the end of the list. ⇧↵ is always a plain
          // newline — the way out for a line that only looks like an item.
          if (e.key === "Enter" && !e.shiftKey && !e.metaKey && !e.ctrlKey && !e.altKey) {
            if (area.selectionStart !== area.selectionEnd) return;
            const edit = carry(area.value, area.selectionStart);
            if (!edit) return;
            e.preventDefault();
            write(area, edit.from, edit.to, edit.insert, [edit.caret, edit.caret]);
          }
        }}
        placeholder={placeholder}
        spellCheck
        // Prose, unlike code: the browser's own writing aids are wanted here and
        // are turned off in the other two editors for the opposite reason.
        autoCapitalize="sentences"
        autoCorrect="on"
      />

      {menu && (
        <ul
          ref={list}
          className="prose-menu"
          role="listbox"
          aria-label="Markdown"
          style={{ left: menu.at.left, top: menu.at.top + GAP }}
          // The textarea's `blur` fires before a click lands, so the list would
          // close out from under the pointer. Taking the press rather than the
          // click means focus never leaves in the first place.
          onMouseDown={(e) => e.preventDefault()}
        >
          {menu.items.map((item, i) => (
            // The kind too: a sketch may be called "Table".
            <li key={`${item.kind}:${item.label}`} role="presentation">
              <button
                type="button"
                role="option"
                aria-selected={i === active}
                className={`prose-choice${i === active ? " on" : ""}`}
                onMouseEnter={() => setActive(i)}
                onClick={() => take(item)}
              >
                <span className={`prose-tag ${item.kind}`}>{MARK[item.kind]}</span>
                <span className="prose-label">{item.label}</span>
                <span className="prose-note">{item.note}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Where the caret is, in pixels from the editor's top left: the bottom of its
 *  line, and that line's height. */
type Caret = { left: number; top: number; line: number };

/** The space between the caret's line and the menu, and between the menu and
 *  the panel's edge. */
const GAP = 4;

/** What each kind of row wears, so the list is scannable by shape. */
const MARK: Record<Choice["kind"], string> = {
  block: "¶",
  diagram: "◇",
  language: "`",
  sketch: "✎",
  note: "→",
};

/**
 * Where the caret is, measured in a hidden copy of the painted layer.
 *
 * The copy has the painted layer's own classes, so it wraps exactly where the
 * layer does — which is exactly where the textarea does, or nothing on screen
 * would line up. The text up to the caret goes in, then an empty marker, and
 * the marker's position is the caret's. Made and removed in one go, so there is
 * never a second copy of a note in the document between keystrokes.
 *
 * jsdom lays nothing out and reports zero for all of it, which lands the list
 * at the top left in the mount tests — the `notes-complete` screenshot is the
 * check of where it really goes.
 */
function caretAt(area: HTMLTextAreaElement): Caret {
  const [mark] = measure(area, [area.selectionStart]);
  return mark
    ? { left: mark.left, top: mark.top + mark.height, line: mark.height }
    : { left: 0, top: 0, line: 0 };
}

/**
 * How far down the editor each of `offsets` lands, in order.
 *
 * The same layout pass the menu is placed by, handed to the pane so it can
 * find the note's headings in the editor and line the two views up by them —
 * see `NoteBody`. Zero for every one in jsdom.
 */
export function offsetTops(area: HTMLTextAreaElement, offsets: readonly number[]): number[] {
  return measure(area, offsets).map((mark) => mark.top);
}

/**
 * Lays the text out once more with a marker at each offset, and reads where
 * each marker went. `offsets` in order, as positions in the textarea's value.
 */
function measure(
  area: HTMLTextAreaElement,
  offsets: readonly number[],
): { left: number; top: number; height: number }[] {
  const host = area.parentElement;
  if (!host) return offsets.map(() => ({ left: 0, top: 0, height: 0 }));
  const copy = document.createElement("pre");
  copy.className = "painted-ink prose-ink prose-measure";
  copy.setAttribute("aria-hidden", "true");
  const marks: HTMLSpanElement[] = [];
  let at = 0;
  for (const offset of offsets) {
    copy.append(area.value.slice(at, offset));
    const mark = document.createElement("span");
    // A zero-width space, so the marker has a line box to report without
    // adding a character that could move a break.
    mark.textContent = "​";
    copy.append(mark);
    marks.push(mark);
    at = offset;
  }
  host.appendChild(copy);
  const found = marks.map((mark) => ({
    left: mark.offsetLeft,
    top: mark.offsetTop,
    height: mark.offsetHeight,
  }));
  copy.remove();
  return found;
}

/** The nearest box around `el` that scrolls — the one the menu must fit in. */
function scrollerOf(el: HTMLElement): HTMLElement | null {
  for (let up = el.parentElement; up; up = up.parentElement) {
    const { overflowY } = getComputedStyle(up);
    if (overflowY === "auto" || overflowY === "scroll") return up;
  }
  return null;
}

/**
 * Replaces a stretch of the textarea the way typing would.
 *
 * `insertText` is the one route into the browser's own undo history, so it is
 * tried first; a ⌘Z after a list carried on takes the new item back rather
 * than the whole history with it. Where it is missing — jsdom, and in principle
 * any engine that finally drops a deprecated command — the text goes in
 * directly and an `input` is sent, which is what the component listens to
 * either way.
 */
function replace(area: HTMLTextAreaElement, from: number, to: number, insert: string) {
  area.focus();
  area.setSelectionRange(from, to);
  const exec = typeof document.execCommand === "function" ? document.execCommand.bind(document) : null;
  // An empty insert is a deletion, which `insertText` does not reliably do.
  const done = exec
    ? insert === ""
      ? from === to || exec("delete", false)
      : exec("insertText", false, insert)
    : false;
  if (done) return;
  area.setRangeText(insert, from, to, "end");
  area.dispatchEvent(new Event("input", { bubbles: true }));
}
