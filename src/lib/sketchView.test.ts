// @vitest-environment jsdom
//
// Everything here is a round trip through `localStorage`, which needs a real
// one — and the guard that makes a blocked store harmless needs one that can be
// made to throw.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { forgetView, moveView, recall, remember, shows, viewKey, type View } from "./sketchView";

const KEY = "taurus.sketchViews";

const view = (over: Partial<View> = {}): View => ({
  zoom: 1.5,
  scrollX: -100,
  scrollY: -40,
  width: 800,
  height: 600,
  ...over,
});

beforeEach(() => localStorage.clear());
afterEach(() => vi.restoreAllMocks());

describe("keeping a sketch's view", () => {
  it("gives back the view it was given", () => {
    const key = viewKey({ scope: "workspace", name: "Auth flow" }, "/code/taurus");
    remember(key, view());
    expect(recall(key)).toEqual(view());
  });

  it("keeps a project sketch's view with its folder, and a global one with neither", () => {
    const here = viewKey({ scope: "workspace", name: "Arch" }, "/code/a");
    const there = viewKey({ scope: "workspace", name: "Arch" }, "/code/b");
    expect(here).not.toBe(there);
    remember(here, view({ zoom: 2 }));
    expect(recall(there)).toBeNull();
    // A global sketch belongs to no folder, so every folder finds the same view.
    expect(viewKey({ scope: "global", name: "Arch" }, "/code/a")).toBe(
      viewKey({ scope: "global", name: "Arch" }, "/code/b"),
    );
  });

  it("carries the view to a new name, and drops it on delete", () => {
    remember("old", view());
    moveView("old", "new");
    expect(recall("old")).toBeNull();
    expect(recall("new")).toEqual(view());
    forgetView("new");
    expect(recall("new")).toBeNull();
  });

  it("refuses a view Excalidraw could not have reported", () => {
    for (const bad of [view({ zoom: 0 }), view({ zoom: 31 }), view({ scrollX: Number.NaN }), view({ width: 0 })]) {
      remember("k", bad);
      expect(recall("k"), JSON.stringify(bad)).toBeNull();
    }
  });

  it("reads past a stored value it did not write, one entry at a time", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({ good: { ...view(), at: 1 }, bad: { zoom: "big" }, worse: null }),
    );
    expect(recall("good")).toEqual(view());
    expect(recall("bad")).toBeNull();
    localStorage.setItem(KEY, "{ not json");
    expect(recall("good")).toBeNull();
  });

  it("lets the least recently kept go once there are too many", () => {
    let now = 1_000;
    vi.spyOn(Date, "now").mockImplementation(() => ++now);
    for (let i = 0; i < 201; i++) remember(`s${i}`, view());
    expect(recall("s0")).toBeNull();
    expect(recall("s1")).toEqual(view());
    expect(recall("s200")).toEqual(view());
  });

  it("does nothing, and throws nothing, when the store is blocked", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(() => remember("k", view())).not.toThrow();
    expect(recall("k")).toBeNull();
  });
});

describe("whether a view still shows the drawing", () => {
  // At zoom 2 on an 800×600 canvas, panned so the scene's (100, 50) is the top
  // left: the view covers x 100–500 and y 50–350 of the scene.
  const here = view({ zoom: 2, scrollX: -100, scrollY: -50, width: 800, height: 600 });
  const box = (x: number, y: number, extra = {}) => ({ x, y, width: 50, height: 50, ...extra });

  it("does when any element overlaps it", () => {
    expect(shows(here, [box(2000, 2000), box(480, 340)])).toBe(true);
  });

  it("does not when the drawing is somewhere else entirely", () => {
    // A teammate moved everything, or a turn redrew it.
    expect(shows(here, [box(2000, 2000), box(-500, 0)])).toBe(false);
  });

  it("ignores what has been deleted", () => {
    expect(shows(here, [box(200, 100, { isDeleted: true }), box(2000, 2000)])).toBe(false);
  });

  it("reads an extent drawn backwards", () => {
    expect(shows(here, [{ x: 600, y: 100, width: -150, height: 20 }])).toBe(true);
  });

  it("does for an empty drawing, where there is nothing to centre on", () => {
    expect(shows(here, [])).toBe(true);
    expect(shows(here, [box(200, 100, { isDeleted: true })])).toBe(true);
  });
});
