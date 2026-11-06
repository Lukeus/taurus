// @vitest-environment jsdom
//
// A round trip through `localStorage`, which needs a real one, and the guard
// that makes a blocked store harmless needs one that can be made to throw.
import { afterEach, describe, expect, it, vi } from "vitest";

import { MAX_TABS, nameTab, openTab, recallTabs, rememberTabs, showTab, type CanvasTabs } from "./canvasTabs";

const KEY = "taurus.canvasTabs";

const opened = (...paths: string[]) =>
  paths.reduce<CanvasTabs | null>((state, path) => openTab(state, path, null), null)!;

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("tabs kept between sessions", () => {
  it("come back for the folder they were open in, with the same one showing", () => {
    rememberTabs("/w", showTab(opened("a.md", "b.md", "c.md"), "b.md"));
    rememberTabs("/other", opened("x.md"));

    const back = recallTabs("/w")!;
    expect(back.tabs.map((tab) => tab.path)).toEqual(["a.md", "b.md", "c.md"]);
    expect(back.active).toBe("b.md");
    expect(back.tabs.every((tab) => tab.restored && tab.shown === null)).toBe(true);
    expect(recallTabs("/other")!.tabs.map((tab) => tab.path)).toEqual(["x.md"]);
    expect(recallTabs("/never")).toBeNull();
  });

  it("keep their order, so the oldest is still the first to close for room", () => {
    rememberTabs("/w", opened(...Array.from({ length: MAX_TABS }, (_, i) => `f${i}.md`)));
    const back = recallTabs("/w")!;
    const next = openTab(back, "new.md", null);
    expect(next.tabs.map((tab) => tab.path)).not.toContain("f0.md");
    expect(next.tabs).toHaveLength(MAX_TABS);
  });

  it("are forgotten once the last one closes", () => {
    rememberTabs("/w", opened("a.md"));
    rememberTabs("/w", null);
    expect(recallTabs("/w")).toBeNull();
    expect(JSON.parse(localStorage.getItem(KEY)!)).toEqual({});
  });

  it("stop being restored once they've been read", () => {
    rememberTabs("/w", opened("a.md"));
    const back = recallTabs("/w")!;
    expect(back.tabs[0].restored).toBe(true);
    const read = nameTab(back, "a.md", "a.md")!;
    expect(read.tabs[0].restored).toBeUndefined();
  });

  it("make do with whatever an older version left, one folder at a time", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({
        "/w": { paths: ["a.md", 7, "", "a.md", "b.md"], active: "gone.md" },
        "/bad": "nonsense",
        "/empty": { paths: [] },
      }),
    );
    const back = recallTabs("/w")!;
    expect(back.tabs.map((tab) => tab.path)).toEqual(["a.md", "b.md"]);
    // A tab that isn't there can't be the one showing; the last one is.
    expect(back.active).toBe("b.md");
    expect(recallTabs("/bad")).toBeNull();
    expect(recallTabs("/empty")).toBeNull();

    localStorage.setItem(KEY, "{not json");
    expect(recallTabs("/w")).toBeNull();
  });

  it("are lost quietly when storage is blocked", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(() => rememberTabs("/w", opened("a.md"))).not.toThrow();
    expect(recallTabs("/w")).toBeNull();
  });
});
