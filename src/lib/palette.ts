/**
 * What the palette matches, and how well.
 *
 * Pure and here rather than in the component, for the reason `sql.ts` is: the
 * ranking is the whole feel of the thing, it is worth testing on its own, and
 * none of it needs a DOM.
 *
 * The design goal is that the *second* character you type puts the right row
 * first. Everything below is in service of that — a subsequence match is what
 * lets `nc` find "New conversation", and the bonuses are what stop it also
 * finding "Open **n**ext **c**hanges" and putting that above.
 */

/** One thing the palette can do. */
export type Action = {
  /** Stable across rebuilds of the list, so the highlight survives a redraw. */
  id: string;
  label: string;
  /** The heading this sits under. Rows are shown grouped, in list order. */
  group: string;
  /**
   * Words it should match on that are not on the row.
   *
   * The synonyms someone would reach for — "undo" for the Changes drawer,
   * "tokens" for the context account. Matched at a penalty and never
   * highlighted, because marking characters in text that is not on screen is
   * marking nothing.
   */
  keywords?: string;
  /** The key that also does this. Shown on the row, which is how anybody
   *  finds out it exists. */
  shortcut?: string;
  /**
   * Why this cannot be run right now.
   *
   * Shown, and the row is left in place rather than filtered out. A command
   * that disappears when it is unavailable teaches that it does not exist;
   * one that says "no conversation open" teaches what it needs.
   */
  unavailable?: string;
  run: () => void;
};

/** A scored candidate, with the characters that earned it. */
export type Scored<T> = {
  item: T;
  score: number;
  /** Indices into the label that matched, for the highlight. */
  spans: number[];
};

/*
 * The bonuses, named rather than spelled inline so the ranking can be read as
 * a sentence: a match at the start of a word beats one in the middle of it, a
 * run of adjacent characters beats the same characters scattered, and an
 * earlier match beats a later one.
 */
const AT_START = 12;
const AT_WORD = 8;
const ADJACENT = 6;
const PER_CHAR = 1;
/** Charged per character skipped before the first match, so `git` ranks
 *  "Git status" above "Toggle git", which merely contains it. */
const LATE = 0.4;
/** What a keyword-only match gives up. Enough that anything matching the
 *  visible label wins, not so much that a synonym is useless. */
const HIDDEN = 30;

const BOUNDARY = /[\s\-_/.:]/;

/**
 * How well `query` matches `text`, or `null` for not at all.
 *
 * Higher is better. Subsequence rather than substring: every character of the
 * query must appear, in order, and what varies is how much they are rewarded
 * for where they appear.
 */
export function score(text: string, query: string): { score: number; spans: number[] } | null {
  if (!query) return { score: 0, spans: [] };
  const haystack = text.toLowerCase();
  const needle = query.toLowerCase();

  const spans: number[] = [];
  let total = 0;
  let at = 0;

  for (let q = 0; q < needle.length; q += 1) {
    const wanted = needle[q];
    // Whitespace in the query is a separator, not something to find: "new
    // conv" is two words to look for, and the space between them is neither.
    if (/\s/.test(wanted)) continue;

    const found = haystack.indexOf(wanted, at);
    if (found === -1) return null;

    if (found === 0) total += AT_START;
    else if (BOUNDARY.test(haystack[found - 1])) total += AT_WORD;
    if (spans.length && found === spans[spans.length - 1] + 1) total += ADJACENT;
    total += PER_CHAR;
    // Only the gap this character skipped, so a long label is not punished
    // for the part after the match.
    total -= (found - at) * LATE;

    spans.push(found);
    at = found + 1;
  }

  // A short label that matched is a better answer than a long one that
  // matched the same way — there is simply less of it that is not the query.
  return { score: total - text.length * 0.05, spans };
}

/**
 * The best matches for `query`, best first.
 *
 * An empty query keeps everything in the order it was given, which is what the
 * list should already be in: the palette opened on nothing is a menu, and a
 * menu that reorders itself is a menu nobody learns.
 */
export function rank<T>(
  items: readonly T[],
  query: string,
  read: (item: T) => { label: string; hidden?: string },
  limit?: number,
): Scored<T>[] {
  if (!query.trim()) {
    const all = items.map((item) => ({ item, score: 0, spans: [] as number[] }));
    return limit === undefined ? all : all.slice(0, limit);
  }

  const scored: Scored<T>[] = [];
  for (const item of items) {
    const { label, hidden } = read(item);
    const direct = score(label, query);
    if (direct) {
      scored.push({ item, score: direct.score, spans: direct.spans });
      continue;
    }
    if (!hidden) continue;
    // Matched on words that are not on the row, so there is nothing to
    // highlight — and saying so with an empty span list is more honest than
    // marking characters of the label that had nothing to do with it.
    const indirect = score(`${label} ${hidden}`, query);
    if (indirect) scored.push({ item, score: indirect.score - HIDDEN, spans: [] });
  }

  // Stable within a score: two rows that match equally well keep the order the
  // caller put them in, which is the order they were meant to be read in.
  const order = new Map(items.map((item, i) => [item, i]));
  scored.sort((a, b) => b.score - a.score || order.get(a.item)! - order.get(b.item)!);
  return limit === undefined ? scored : scored.slice(0, limit);
}

/**
 * Splits a label into the runs that matched and the runs that did not.
 *
 * Runs rather than per-character spans: three adjacent matched characters are
 * one mark, and a component that drew one element each would put a seam
 * through the middle of a word.
 */
export function marked(label: string, spans: readonly number[]): { text: string; on: boolean }[] {
  if (!spans.length) return label ? [{ text: label, on: false }] : [];
  const hit = new Set(spans);
  const out: { text: string; on: boolean }[] = [];
  for (let i = 0; i < label.length; i += 1) {
    const on = hit.has(i);
    const last = out[out.length - 1];
    if (last && last.on === on) last.text += label[i];
    else out.push({ text: label[i], on });
  }
  return out;
}
