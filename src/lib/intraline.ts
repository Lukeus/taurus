/**
 * Which part of a changed line actually changed.
 *
 * A line diff says a line was replaced. That is the truth and it is rarely the
 * answer: a rename touching one identifier in a sixty-character line arrives
 * as a whole line struck out and a whole line added, and finding the two
 * characters that moved is left to the reader — at the exact moment the reader
 * is deciding whether to approve a write.
 *
 * This narrows it, and it narrows it by trimming rather than by matching. The
 * common run of words at the start and the common run at the end come off, and
 * whatever is left in the middle is the change. That is one region per line by
 * construction, which is both the cheap thing to compute and the honest thing
 * to draw: two regions marked on one line ask a reader to work out whether the
 * gap between them is unchanged or merely unmarked.
 *
 * It also declines. A line rewritten from end to end has no common trim worth
 * the name, and marking almost all of it is the same as marking none of it
 * while looking like a finding — see `FLOOR`.
 */

/** The changed span of each side, as character offsets into that side's text. */
export type Refined = {
  oldFrom: number;
  oldTo: number;
  newFrom: number;
  newTo: number;
};

/**
 * How much of the longer line has to survive the trim for the result to be
 * worth showing.
 *
 * Below this the two lines have little in common and the honest rendering is
 * the one already there: a whole line removed and a whole line added. The
 * number is a judgement rather than a measurement — a quarter is about where a
 * marked region stops reading as "look here" and starts reading as "this line,
 * again".
 */
const FLOOR = 0.25;

/**
 * Splits a line into the units a person would say changed.
 *
 * Words, runs of whitespace, and single characters for everything else. Not
 * characters throughout: a character-level trim on `foo_bar` and `foo_baz`
 * marks the `r`/`z` and leaves the reader to notice which word it was in,
 * which is a smaller region and a worse answer.
 */
function units(line: string): string[] {
  return line.match(/\s+|[A-Za-z0-9_$]+|[^\s]/g) ?? [];
}

/**
 * The changed middle of a pair of lines, or `null` when there is not a useful
 * one — identical lines, and lines too different to have a middle.
 */
export function refine(before: string, after: string): Refined | null {
  if (before === after) return null;

  const a = units(before);
  const b = units(after);

  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head += 1;

  let tail = 0;
  while (
    tail < a.length - head &&
    tail < b.length - head &&
    a[a.length - 1 - tail] === b[b.length - 1 - tail]
  ) {
    tail += 1;
  }

  const width = (parts: string[]) => parts.reduce((n, part) => n + part.length, 0);
  const oldFrom = width(a.slice(0, head));
  const newFrom = width(b.slice(0, head));
  const oldTo = before.length - width(a.slice(a.length - tail));
  const newTo = after.length - width(b.slice(b.length - tail));

  const kept = oldFrom + (before.length - oldTo);
  const longest = Math.max(before.length, after.length);
  if (longest === 0 || kept / longest < FLOOR) return null;

  return { oldFrom, oldTo, newFrom, newTo };
}

/**
 * Pairs the removed lines of a hunk with the added lines that replaced them.
 *
 * A hunk is a flat list — `context`, `removed`, `added` — and nothing in it
 * says which addition answers which removal. What is reliable is the shape a
 * diff produces: an edit shows up as a run of removals followed immediately by
 * a run of additions, and when those two runs are the same length, the nth of
 * each is the same line before and after.
 *
 * When the lengths differ, they are not paired at all. Lines were inserted or
 * deleted as well as changed, so an index-wise pairing would be lining up
 * lines that have nothing to do with each other and marking the difference
 * between them — confidently, and wrongly. Nothing is worse here than a
 * highlight that points at the wrong characters, so the rule is to pair only
 * where the pairing is forced.
 *
 * Returns, for each index in `kinds`, the index of its counterpart.
 */
export function pairs(kinds: readonly string[]): Map<number, number> {
  const found = new Map<number, number>();
  let i = 0;
  while (i < kinds.length) {
    if (kinds[i] !== "removed") {
      i += 1;
      continue;
    }
    let removedEnd = i;
    while (removedEnd < kinds.length && kinds[removedEnd] === "removed") removedEnd += 1;
    let addedEnd = removedEnd;
    while (addedEnd < kinds.length && kinds[addedEnd] === "added") addedEnd += 1;

    const removed = removedEnd - i;
    const added = addedEnd - removedEnd;
    if (added > 0 && added === removed) {
      for (let n = 0; n < removed; n += 1) {
        found.set(i + n, removedEnd + n);
        found.set(removedEnd + n, i + n);
      }
    }
    i = addedEnd > removedEnd ? addedEnd : removedEnd;
  }
  return found;
}
