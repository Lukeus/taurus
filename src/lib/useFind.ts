import { useEffect, useMemo, useRef, useState, type RefObject } from "react";

import {
  findAll,
  findProblem,
  hitFrom,
  lineAsked,
  MOST_HITS,
  PLAIN,
  replaceEvery,
  replacementFor,
  type FindKey,
  type FindOptions,
  type Hit,
} from "./find";
import { replaceText } from "./indent";

/** What the find bar is handed. See `FindBar`. */
export type FindBarProps = {
  query: string;
  said: string | null;
  asked: number;
  options: FindOptions;
  /** Absent when the text can't be changed, which hides the replace row. */
  replace?: {
    open: boolean;
    text: string;
    onOpen: (open: boolean) => void;
    onText: (text: string) => void;
    onOne: () => void;
    onAll: () => void;
  };
  onQuery: (query: string) => void;
  onOption: (option: keyof FindOptions) => void;
  onMove: (by: 1 | -1) => void;
  onSubmit: () => boolean;
  onClose: () => void;
};

/**
 * Find, replace and go to a line, over a textarea.
 *
 * One hook for every editor that has a find bar, so the canvas and a note find
 * the same things with the same keys. What differs between them is how a place
 * in the text is brought into view, which is `lookAt`: the canvas does arithmetic
 * on fixed rows, and a note, whose lines wrap, scrolls to the painted match.
 *
 * Nothing is selected while the bar is open, only painted, because a selection
 * on the canvas travels to the model with the next message, and "lines 40–41"
 * would be a claim about whatever the search last passed. Closing the bar selects
 * the match it stopped on, which is where somebody meant to be.
 */
export function useFind({
  text,
  box,
  ask,
  editable,
  lookAt,
  land,
}: {
  text: string;
  box: RefObject<HTMLTextAreaElement | null>;
  /** A find key, for the bar. An object so pressing it twice is two events. */
  ask?: { key: FindKey } | null;
  /** Whether a replace may write into the box. */
  editable: boolean;
  /** Brings `offset` into view. Called after the text it is in has rendered. */
  lookAt: (offset: number) => void;
  /** After the caret is placed in the box, for whatever reports it. */
  land?: (area: HTMLTextAreaElement) => void;
}): { bar: FindBarProps | null; hits: Hit[]; shown: number } {
  const [finding, setFinding] = useState(false);
  const [asked, setAsked] = useState(0);
  const [query, setQuery] = useState("");
  const [options, setOptions] = useState<FindOptions>(PLAIN);
  const [current, setCurrent] = useState(-1);
  const [replacing, setReplacing] = useState(false);
  const [replacement, setReplacement] = useState("");
  /** Said in place of the count until the search changes. */
  const [replaced, setReplaced] = useState<number | null>(null);
  /** Where the caret was, for a search to begin from rather than the top. */
  const startedAt = useRef(0);
  /** A place to look at once the text holding it has rendered. */
  const follow = useRef<number | null>(null);

  const goingTo = lineAsked(query);
  const hits = useMemo(
    () => (finding && goingTo === null ? findAll(text, query, options) : []),
    [finding, goingTo, text, query, options],
  );
  const shown = hits.length === 0 ? -1 : Math.min(Math.max(current, 0), hits.length - 1);

  useEffect(() => {
    if (follow.current === null) return;
    const at = follow.current;
    follow.current = null;
    lookAt(at);
  });

  /** Moves to the first match at or after `from` in `within`, and looks at it. */
  const settle = (within: string, q: string, o: FindOptions, from: number) => {
    const found = lineAsked(q) === null ? findAll(within, q, o) : [];
    const at = hitFrom(found, from);
    setCurrent(at);
    if (at >= 0) follow.current = found[at].from;
  };

  const typeQuery = (next: string) => {
    setQuery(next);
    setReplaced(null);
    settle(text, next, options, startedAt.current);
  };

  const toggle = (option: keyof FindOptions) => {
    const next = { ...options, [option]: !options[option] };
    setOptions(next);
    setReplaced(null);
    settle(text, query, next, hits[shown]?.from ?? startedAt.current);
  };

  const move = (by: 1 | -1) => {
    if (hits.length === 0) return;
    const next = (shown + by + hits.length) % hits.length;
    setCurrent(next);
    lookAt(hits[next].from);
  };

  const close = () => {
    const area = box.current;
    const hit = hits[shown];
    setFinding(false);
    setReplaced(null);
    if (!area) return;
    area.focus({ preventScroll: true });
    if (hit) {
      area.setSelectionRange(hit.from, hit.to);
      lookAt(hit.from);
      land?.(area);
    }
  };

  /** Enter in the box. Answers whether it was a line to go to. */
  const goToLine = () => {
    const area = box.current;
    if (goingTo === null) return false;
    setFinding(false);
    if (!area) return true;
    const start = lineStart(text, goingTo - 1);
    area.focus({ preventScroll: true });
    area.setSelectionRange(start, start);
    lookAt(start);
    land?.(area);
    return true;
  };

  /*
   * Through the browser's own edit, so ⌘Z takes it back, and then on to the next
   * match after what was put in: the replacement may itself match, as `a` to
   * `aa` does, and stepping onto it again would replace it for ever.
   */
  const replaceOne = () => {
    const area = box.current;
    const hit = hits[shown];
    if (!area || !hit || !editable) return;
    const insert = replacementFor(text, query, options, hit, replacement);
    replaceText(area, hit.from, hit.to, insert);
    const after = text.slice(0, hit.from) + insert + text.slice(hit.to);
    setReplaced(null);
    settle(after, query, options, hit.from + insert.length);
  };

  /* One edit from the first match to the end of the last, so it is one ⌘Z. */
  const replaceAll = () => {
    const area = box.current;
    if (!area || !editable) return;
    const edit = replaceEvery(text, query, options, replacement);
    if (!edit) return;
    replaceText(area, edit.from, edit.to, edit.insert);
    setReplaced(edit.count);
    setCurrent(-1);
    follow.current = edit.from;
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
    if (ask.key === "replace" && editable) setReplacing(true);
    if (ask.key === "line") setQuery(":");
    else if (
      area &&
      area.selectionStart !== area.selectionEnd &&
      !area.value.slice(area.selectionStart, area.selectionEnd).includes("\n")
    ) {
      // What's highlighted is usually what's being looked for.
      typeQuery(area.value.slice(area.selectionStart, area.selectionEnd));
    } else if (lineAsked(query) !== null || query === ":") setQuery("");
    setFinding(true);
    setAsked((n) => n + 1);
    // Fires on the key and nothing else. What it reads is this render's.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ask]);

  if (!finding) return { bar: null, hits, shown };

  const problem = findProblem(query, options);
  const lines = goingTo === null ? 0 : text.split("\n").length;
  const said =
    goingTo !== null
      ? `Go to line ${Math.min(goingTo, lines)}`
      : replaced !== null
        ? `Replaced ${replaced.toLocaleString()}`
        : !query
          ? null
          : problem
            ? problem
            : hits.length === 0
              ? "No matches"
              : hits.length >= MOST_HITS
                ? `${shown + 1} of ${MOST_HITS.toLocaleString()}+`
                : `${shown + 1} of ${hits.length}`;

  return {
    hits,
    shown,
    bar: {
      query,
      said,
      asked,
      options,
      replace: editable
        ? {
            open: replacing,
            text: replacement,
            onOpen: setReplacing,
            onText: setReplacement,
            onOne: replaceOne,
            onAll: replaceAll,
          }
        : undefined,
      onQuery: typeQuery,
      onOption: toggle,
      onMove: move,
      onSubmit: goToLine,
      onClose: close,
    },
  };
}

/** The offset line `n` starts at, counting from 0, or the last line's start past the end. */
function lineStart(text: string, n: number): number {
  let at = 0;
  for (let line = 0; line < n; line++) {
    const next = text.indexOf("\n", at);
    if (next === -1) break;
    at = next + 1;
  }
  return at;
}
