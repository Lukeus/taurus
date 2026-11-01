import { describe, expect, it } from "vitest";

import {
  closeTab,
  MAX_TABS,
  nameTab,
  openTab,
  samePath,
  showTab,
  tabLabels,
  type CanvasTabs,
} from "./canvasTabs";

const opened = (...paths: string[]) =>
  paths.reduce<CanvasTabs | null>((state, path) => openTab(state, path, null), null)!;
const names = (state: CanvasTabs | null) => state?.tabs.map((tab) => tab.path);

describe("samePath", () => {
  it("gives every spelling of a workspace file one name", () => {
    for (const spelling of ["a/b.md", "./a/b.md", "a//b.md", "a/c/../b.md", "/w/a/b.md"]) {
      expect(samePath(spelling, "/w")).toBe("a/b.md");
    }
  });

  it("reads Windows separators", () => {
    expect(samePath("C:\\w\\a\\b.md", "C:\\w")).toBe("a/b.md");
  });

  it("leaves a path outside the workspace absolute", () => {
    expect(samePath("/home/me/skills/x.md", "/w")).toBe("/home/me/skills/x.md");
  });

  it("keeps a climb out of the workspace for the guard to refuse", () => {
    expect(samePath("../secret", "/w")).toBe("../secret");
  });
});

describe("openTab", () => {
  it("adds a file and shows it", () => {
    const state = opened("a", "b");
    expect(names(state)).toEqual(["a", "b"]);
    expect(state.active).toBe("b");
  });

  it("goes to a file that's already open rather than opening it twice", () => {
    const state = openTab(opened("a", "b"), "a", null);
    expect(names(state)).toEqual(["a", "b"]);
    expect(state.active).toBe("a");
  });

  it("carries a new place to go to a file that's already open", () => {
    const place = { from: 3, to: 4 };
    expect(openTab(opened("a"), "a", place).tabs[0].reveal).toBe(place);
  });

  it("finds an open file by the name the host gave it", () => {
    const named = nameTab(opened("link.md"), "link.md", "real.md")!;
    expect(names(openTab(named, "real.md", null))).toEqual(["link.md"]);
  });

  it("leaves what's showing alone when asked to open quietly", () => {
    const state = openTab(opened("a"), "b", null, { quietly: true });
    expect(names(state)).toEqual(["a", "b"]);
    expect(state.active).toBe("a");
  });

  it("shows the first file even when asked to open it quietly", () => {
    expect(openTab(null, "a", null, { quietly: true }).active).toBe("a");
  });

  const full = () => opened(...Array.from({ length: MAX_TABS }, (_, i) => `f${i}`));

  it("closes the least recently used file past the limit", () => {
    const state = openTab(showTab(full(), "f0"), "new", null);
    expect(state.tabs).toHaveLength(MAX_TABS);
    expect(names(state)).toContain("f0");
    expect(names(state)).not.toContain("f1");
  });

  it("never closes a file holding what the disk doesn't", () => {
    const state = openTab(full(), "new", null, { keep: (path) => path === "f0" });
    expect(names(state)).toContain("f0");
    expect(names(state)).not.toContain("f1");
  });

  it("runs past the limit rather than lose anybody's typing", () => {
    expect(openTab(full(), "new", null, { keep: () => true }).tabs).toHaveLength(MAX_TABS + 1);
  });
});

describe("closeTab", () => {
  it("shows the next file along", () => {
    expect(closeTab(showTab(opened("a", "b", "c"), "b"), "b")?.active).toBe("c");
  });

  it("shows the one before when the last in the row closes", () => {
    expect(closeTab(opened("a", "b"), "b")?.active).toBe("a");
  });

  it("leaves what's showing when closing another", () => {
    expect(closeTab(opened("a", "b"), "a")?.active).toBe("b");
  });

  it("answers nothing when the last file closes", () => {
    expect(closeTab(opened("a"), "a")).toBeNull();
  });
});

describe("nameTab", () => {
  it("records the host's name without changing what identifies the tab", () => {
    const [tab] = nameTab(opened("link.md"), "link.md", "real.md")!.tabs;
    expect(tab).toMatchObject({ path: "link.md", shown: "real.md" });
  });

  it("folds two routes to one file into the tab already open", () => {
    const state = nameTab(opened("real.md", "link.md"), "link.md", "real.md");
    expect(names(state)).toEqual(["real.md"]);
    expect(state?.active).toBe("real.md");
  });
});

describe("tabLabels", () => {
  it("is the filename when that's enough", () => {
    expect(tabLabels(["src/a.ts", "src/b.ts"]).get("src/a.ts")).toBe("a.ts");
  });

  it("adds as much folder as it takes to tell two apart", () => {
    const labels = tabLabels(["x/one/lib.rs", "x/two/lib.rs", "main.rs"]);
    expect(labels.get("x/one/lib.rs")).toBe("one/lib.rs");
    expect(labels.get("x/two/lib.rs")).toBe("two/lib.rs");
    expect(labels.get("main.rs")).toBe("main.rs");
  });
});
