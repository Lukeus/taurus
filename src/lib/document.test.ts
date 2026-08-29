import { describe, expect, it } from "vitest";

import { changedLines, dirty, reconcile } from "./document";

const buffer = (base: string, draft = base) => ({ base, draft });

describe("what happens when a file changes under an open document", () => {
  /** The rule this whole module exists for. */
  it("never loses typing: a dirty buffer asks rather than reloading", () => {
    const mine = buffer("one\n", "one\ntyping\n");
    expect(reconcile(mine, "theirs\n")).toEqual({ kind: "conflict" });
  });

  it("takes the new version when there is nothing to lose", () => {
    expect(reconcile(buffer("one\n"), "one\ntwo\n")).toEqual({ kind: "reload" });
  });

  /**
   * Not an optimisation. `files_changed` names every path a turn touched, and a
   * turn that rewrote a file to the same bytes — a formatter, an idempotent
   * edit — would otherwise flash the whole document, or raise a conflict
   * between two identical texts.
   */
  it("does nothing when the new version is what is already on screen", () => {
    expect(reconcile(buffer("one\n"), "one\n")).toEqual({ kind: "same" });
    // Including when it matches what was *typed* rather than what was read:
    // the model made the edit the person was halfway through making.
    expect(reconcile(buffer("one\n", "one\ntwo\n"), "one\ntwo\n")).toEqual({
      kind: "same",
    });
  });

  it("knows whether anything is unsaved", () => {
    expect(dirty(buffer("a"))).toBe(false);
    expect(dirty(buffer("a", "ab"))).toBe(true);
  });
});

describe("the lines to point at after a change", () => {
  it("finds a single changed line", () => {
    expect(changedLines("a\nb\nc\n", "a\nB\nc\n")).toEqual({ from: 2, to: 2 });
  });

  it("finds an inserted run", () => {
    expect(changedLines("a\nd\n", "a\nb\nc\nd\n")).toEqual({ from: 2, to: 3 });
  });

  it("points at where a deletion happened", () => {
    const at = changedLines("a\nb\nc\nd\n", "a\nd\n")!;
    expect(at.from).toBe(2);
    // A deletion has no lines of its own to tint, so the region collapses onto
    // the line that closed over it rather than inverting.
    expect(at.to).toBeGreaterThanOrEqual(at.from);
  });

  it("covers a change at the very top and at the very bottom", () => {
    expect(changedLines("a\nb\n", "A\nb\n")).toEqual({ from: 1, to: 1 });
    expect(changedLines("a\nb\n", "a\nB\n")).toEqual({ from: 2, to: 2 });
  });

  it("says nothing when nothing changed", () => {
    expect(changedLines("a\nb\n", "a\nb\n")).toBeNull();
    expect(changedLines("", "")).toBeNull();
  });

  it("handles one version being empty", () => {
    expect(changedLines("", "a\nb\n")).toEqual({ from: 1, to: 2 });
    expect(changedLines("a\nb\n", "")).not.toBeNull();
  });

  /**
   * The trap the tail loop is written around: with repeated lines, counting
   * back from the end can cross the head and invert the region.
   */
  it("does not invert when the file is full of identical lines", () => {
    const at = changedLines("x\nx\nx\nx\n", "x\nx\n")!;
    expect(at.to).toBeGreaterThanOrEqual(at.from);
    expect(at.from).toBeGreaterThanOrEqual(1);
  });

  /** Stated rather than hidden: this is a pointer, not a diff. */
  it("returns one region even for two edits far apart", () => {
    const at = changedLines("A\nb\nc\nd\nE\n", "X\nb\nc\nd\nY\n")!;
    expect(at).toEqual({ from: 1, to: 5 });
  });
});
