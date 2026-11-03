/**
 * Finding text in a file open on the canvas.
 *
 * Pure, for the reason `document.ts` is: these are decisions that can be wrong in
 * ways nobody notices, and none of them render anything.
 */

import type { Ink } from "./ink";
import { APPLE, isChord } from "./keys";

export type Hit = { from: number; to: number };

/** What a key asks of the find bar. */
export type FindKey = "find" | "replace" | "line" | "next" | "previous";

/** The bar's three toggles. */
export type FindOptions = {
  /** Case always matters. Off, it matters once the query has a capital in it. */
  matchCase: boolean;
  /** Only where the query isn't part of a longer word. */
  wholeWord: boolean;
  /** The query is a regular expression, and `$1` in a replacement means its group. */
  regex: boolean;
};

export const PLAIN: FindOptions = { matchCase: false, wholeWord: false, regex: false };

/**
 * Matches past which the count stops.
 *
 * A one-letter search of a 4 MB file is a million of them, and nobody is stepping
 * through those. They'd only cost the list.
 */
export const MOST_HITS = 10_000;

/**
 * Every place `query` appears, in order, without overlaps.
 *
 * Case matters only once the query has a capital in it, or `matchCase` is on:
 * `retry` finds `Retry`, and `Retry` doesn't find `retry`. Lowering can change a
 * string's length, as `İ` does, which would put every offset after it in the
 * wrong place. Such a text is matched exactly instead.
 *
 * A pattern that doesn't compile finds nothing; `findProblem` says why. A match
 * of no characters is skipped, since there is nothing to paint or replace.
 */
export function findAll(
  text: string,
  query: string,
  options: FindOptions = PLAIN,
  most: number = MOST_HITS,
): Hit[] {
  if (!query) return [];
  const hits: Hit[] = [];
  if (!options.regex && !options.wholeWord) {
    let hay = text;
    let needle = query;
    if (!options.matchCase && query === query.toLowerCase()) {
      const lowered = text.toLowerCase();
      if (lowered.length === text.length) hay = lowered;
      needle = query.toLowerCase();
    }
    let at = hay.indexOf(needle);
    while (at !== -1 && hits.length < most) {
      hits.push({ from: at, to: at + needle.length });
      at = hay.indexOf(needle, at + needle.length);
    }
    return hits;
  }
  const pattern = compile(query, options);
  if (typeof pattern === "string") return [];
  let match: RegExpExecArray | null;
  while (hits.length < most && (match = pattern.exec(text))) {
    if (match[0] === "") {
      pattern.lastIndex = match.index + 1;
      continue;
    }
    hits.push({ from: match.index, to: match.index + match[0].length });
  }
  return hits;
}

/** Why a query can't be searched for, in a few words. `null` when it can. */
export function findProblem(query: string, options: FindOptions): string | null {
  if (!query || !options.regex) return null;
  const pattern = compile(query, options);
  return typeof pattern === "string" ? pattern : null;
}

/**
 * The query as a pattern, or why it isn't one.
 *
 * A word is letters, digits and `_` in any script when the query is plain text.
 * A regular expression keeps its own syntax, which the `u` flag would make
 * stricter, so its words are `\b`'s ASCII ones.
 */
function compile(query: string, options: FindOptions): RegExp | string {
  let source = options.regex ? query : query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  let flags = "g";
  if (options.wholeWord && !options.regex) {
    source = `(?<![\\p{L}\\p{N}_])${source}(?![\\p{L}\\p{N}_])`;
    flags += "u";
  } else if (options.wholeWord) {
    source = `\\b(?:${source})\\b`;
  }
  const smart = !options.regex && query !== query.toLowerCase();
  if (!options.matchCase && !smart) flags += "i";
  try {
    return new RegExp(source, flags);
  } catch {
    return "Not a valid pattern";
  }
}

/**
 * What the match at `hit` becomes.
 *
 * Plain text is put in as typed. With a pattern, `$1`, `$<name>`, `$&` and `$$`
 * mean what they do in `String.replace`, read from the match where it sits in the
 * whole text, so a lookbehind still sees what is before it.
 */
export function replacementFor(
  text: string,
  query: string,
  options: FindOptions,
  hit: Hit,
  replacement: string,
): string {
  if (!options.regex) return replacement;
  const pattern = compile(query, options);
  if (typeof pattern === "string") return replacement;
  const here = new RegExp(pattern.source, pattern.flags.replace("g", "") + "y");
  here.lastIndex = hit.from;
  const match = here.exec(text);
  return match ? expand(match, replacement) : replacement;
}

function expand(match: RegExpExecArray, template: string): string {
  return template.replace(/\$(\$|&|\d{1,2}|<([^>]*)>)/g, (all, token: string, name?: string) => {
    if (token === "$") return "$";
    if (token === "&") return match[0];
    if (name !== undefined) return match.groups ? (match.groups[name] ?? "") : all;
    const n = Number(token);
    if (n >= 1 && n < match.length) return match[n] ?? "";
    // `$12` with one group is the group and a 2, as `String.replace` reads it.
    const one = Number(token[0]);
    if (token.length === 2 && one >= 1 && one < match.length) return (match[one] ?? "") + token[1];
    return all;
  });
}

/**
 * Every match replaced, as one edit from the first match to the end of the last,
 * so ⌘Z takes it back in one step and nothing outside it moves. `null` when
 * there is nothing to replace.
 */
export function replaceEvery(
  text: string,
  query: string,
  options: FindOptions,
  replacement: string,
): { from: number; to: number; insert: string; count: number } | null {
  const hits = findAll(text, query, options, Infinity);
  if (hits.length === 0) return null;
  let insert = "";
  let at = hits[0].from;
  for (const hit of hits) {
    insert += text.slice(at, hit.from) + replacementFor(text, query, options, hit, replacement);
    at = hit.to;
  }
  return { from: hits[0].from, to: at, insert, count: hits.length };
}

/** The line a query asks for, as `:120` does, counting from 1. `null` if it isn't one. */
export function lineAsked(query: string): number | null {
  const match = /^:(\d+)$/.exec(query.trim());
  return match ? Math.max(Number(match[1]), 1) : null;
}

/** The first hit at or after `offset`, wrapping to the first. `-1` when there are none. */
export function hitFrom(hits: readonly Hit[], offset: number): number {
  if (hits.length === 0) return -1;
  const at = hits.findIndex((hit) => hit.from >= offset);
  return at === -1 ? 0 : at;
}

/** A painted run, and whether it's part of a match. */
export type FoundInk = Ink & { hit?: "other" | "current" };

/**
 * Painted runs cut where the matches start and stop.
 *
 * `runs` begin at `offset` in the file, since only a window of it is painted, and
 * every character comes back once and in order, because the runs lie under the
 * textarea. The colour of the text doesn't change, so a match in a string still
 * reads as a string.
 */
export function cutAtHits(
  runs: readonly Ink[],
  offset: number,
  hits: readonly Hit[],
  current: number,
): FoundInk[] {
  if (hits.length === 0) return runs as FoundInk[];
  const out: FoundInk[] = [];
  let at = offset;
  let h = hits.findIndex((hit) => hit.to > offset);
  if (h === -1) return runs as FoundInk[];
  for (const run of runs) {
    const end = at + run.text.length;
    let from = at;
    while (from < end) {
      const hit = hits[h];
      if (!hit || hit.from >= end) {
        out.push({ text: run.text.slice(from - at), kind: run.kind });
        from = end;
      } else if (from < hit.from) {
        out.push({ text: run.text.slice(from - at, hit.from - at), kind: run.kind });
        from = hit.from;
      } else {
        const stop = Math.min(end, hit.to);
        out.push({
          text: run.text.slice(from - at, stop - at),
          kind: run.kind,
          hit: h === current ? "current" : "other",
        });
        from = stop;
        if (stop === hit.to) h += 1;
      }
    }
    at = end;
  }
  return out;
}

/**
 * Which find key this is, if any.
 *
 * Going to a line is Control and G everywhere, as other editors have it. On a Mac
 * the Command chord of the same letter is find-next, its usual meaning there. F3
 * is find-next everywhere, because Control and G is already taken elsewhere.
 * Replace is Option-Command-F on a Mac and Control-H elsewhere, as other editors
 * have it.
 */
export function findKey(e: KeyboardEvent): FindKey | null {
  if (e.key === "F3") return e.shiftKey ? "previous" : "next";
  if (isReplaceChord(e)) return "replace";
  if (isChord(e, "f") && !e.shiftKey) return "find";
  if (e.key.toLowerCase() === "g" && e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
    return "line";
  }
  if (APPLE && isChord(e, "g")) return e.shiftKey ? "previous" : "next";
  return null;
}

function isReplaceChord(e: KeyboardEvent): boolean {
  if (e.shiftKey) return false;
  if (APPLE) return e.code === "KeyF" && e.metaKey && e.altKey && !e.ctrlKey;
  return e.key.toLowerCase() === "h" && e.ctrlKey && !e.metaKey && !e.altKey;
}
