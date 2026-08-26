import { describe, expect, it } from "vitest";

import { pairs, refine } from "./intraline";

/** What the marked region actually reads as, on each side. */
const shown = (before: string, after: string) => {
  const found = refine(before, after);
  if (!found) return null;
  return [before.slice(found.oldFrom, found.oldTo), after.slice(found.newFrom, found.newTo)];
};

describe("narrowing a replaced line", () => {
  it("marks only the part that differs", () => {
    // The whole feature. A rename in a long line arrives as the line struck
    // out and the line added, and finding the two characters that moved is
    // otherwise left to a reader who is deciding whether to approve a write.
    expect(shown("let total = price * quantity;", "let total = price * amount;")).toEqual([
      "quantity",
      "amount",
    ]);
  });

  it("marks nothing for two lines that are the same", () => {
    expect(refine("same", "same")).toBe(null);
    expect(refine("", "")).toBe(null);
  });

  it("declines a line rewritten end to end", () => {
    // Marking nine tenths of a line is the same as marking none of it while
    // looking like a finding, so it says nothing instead and the line-level
    // colours carry the change.
    expect(refine("let a = 1;", "eprintln!(\"totally different thing\");")).toBe(null);
  });

  it("trims by word rather than by character", () => {
    // Character-level trimming marks the `r` and the `z` in `foo_bar` and
    // `foo_baz`, leaving the reader to work out which word they were in.
    expect(shown("call(foo_bar);", "call(foo_baz);")).toEqual(["foo_bar", "foo_baz"]);
  });

  it("handles a pure insertion and a pure deletion", () => {
    // One side of the region is empty, which is exactly right: nothing was
    // there, and the mark has nowhere to sit on that side.
    expect(shown("if (ok) {", "if (ok && ready) {")).toEqual(["", " && ready"]);
    expect(shown("if (ok && ready) {", "if (ok) {")).toEqual([" && ready", ""]);
  });

  it("counts leading whitespace as part of the line", () => {
    // A re-indent is a real change and the diff already says so; what this
    // must not do is claim the change is somewhere else.
    expect(shown("  return x;", "    return x;")).toEqual(["  ", "    "]);
  });
});

describe("pairing the two sides of a hunk", () => {
  const pairing = (kinds: string[]) => [...pairs(kinds)].sort((a, b) => a[0] - b[0]);

  it("pairs runs of equal length index by index", () => {
    expect(pairing(["context", "removed", "removed", "added", "added", "context"])).toEqual([
      [1, 3],
      [2, 4],
      [3, 1],
      [4, 2],
    ]);
  });

  it("pairs nothing when the runs are different lengths", () => {
    // Lines were inserted or deleted as well as changed, so an index-wise
    // pairing would line up lines that have nothing to do with each other and
    // mark the difference between them — confidently, and wrongly.
    expect(pairing(["removed", "added", "added"])).toEqual([]);
    expect(pairing(["removed", "removed", "added"])).toEqual([]);
  });

  it("pairs nothing for a removal or an addition on its own", () => {
    expect(pairing(["context", "removed", "context"])).toEqual([]);
    expect(pairing(["context", "added", "context"])).toEqual([]);
  });

  it("does not pair across a context line", () => {
    // `removed context added` is a deletion and an unrelated insertion, not
    // one line rewritten, and the context line between them says so.
    expect(pairing(["removed", "context", "added"])).toEqual([]);
  });

  it("pairs each run in a hunk that holds several", () => {
    expect(
      pairing(["removed", "added", "context", "context", "removed", "added"]),
    ).toEqual([
      [0, 1],
      [1, 0],
      [4, 5],
      [5, 4],
    ]);
  });
});
