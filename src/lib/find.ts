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
export type FindKey = "find" | "line" | "next" | "previous";

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
 * Case matters only once the query has a capital in it: `retry` finds `Retry`,
 * and `Retry` doesn't find `retry`. Lowering can change a string's length, as
 * `İ` does, which would put every offset after it in the wrong place. Such a text
 * is matched exactly instead.
 */
export function findAll(text: string, query: string): Hit[] {
  if (!query) return [];
  let hay = text;
  let needle = query;
  if (query === query.toLowerCase()) {
    const lowered = text.toLowerCase();
    if (lowered.length === text.length) hay = lowered;
    needle = query.toLowerCase();
  }
  const hits: Hit[] = [];
  let at = hay.indexOf(needle);
  while (at !== -1 && hits.length < MOST_HITS) {
    hits.push({ from: at, to: at + needle.length });
    at = hay.indexOf(needle, at + needle.length);
  }
  return hits;
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
 */
export function findKey(e: KeyboardEvent): FindKey | null {
  if (e.key === "F3") return e.shiftKey ? "previous" : "next";
  if (isChord(e, "f") && !e.shiftKey) return "find";
  if (e.key.toLowerCase() === "g" && e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
    return "line";
  }
  if (APPLE && isChord(e, "g")) return e.shiftKey ? "previous" : "next";
  return null;
}
