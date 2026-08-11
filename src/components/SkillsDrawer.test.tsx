import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import type { SkillSummary } from "../lib/api";
import { partition } from "./SkillsDrawer";

const skill = (patch: Partial<SkillSummary> = {}): SkillSummary => ({
  name: "release-notes",
  description: "Assemble release notes",
  when_to_use: "Preparing release notes",
  version: 1,
  tier: "user",
  allowed_tools: [],
  scripts: [],
  degraded: null,
  dir: "/home/me/.taurus/skills/release-notes",
  ...patch,
});

describe("skill filters", () => {
  const library = [
    skill({ name: "a", tier: "user" }),
    skill({ name: "b", tier: "project" }),
    skill({ name: "c", tier: "project", degraded: "python3 not found" }),
  ];

  it("counts every skill under all", () => {
    expect(partition(library, "all").shown).toHaveLength(3);
  });

  it("narrows to what this project added", () => {
    const { shown } = partition(library, "project");
    expect(shown.map((s) => s.name)).toEqual(["b", "c"]);
  });

  it("finds the broken one, which is the reason to open this list", () => {
    const { shown } = partition(library, "attention");
    expect(shown.map((s) => s.name)).toEqual(["c"]);
  });

  it("counts the same regardless of which filter is showing", () => {
    // The pills render counts from this result while the list renders `shown`.
    // If they were computed separately they could disagree, and a filter that
    // says 1 over a list of two is the sort of thing nobody reports.
    for (const filter of ["all", "project", "attention"] as const) {
      const counts = partition(library, filter);
      expect(counts.all).toHaveLength(3);
      expect(counts.project).toHaveLength(2);
      expect(counts.attention).toHaveLength(1);
    }
  });

  it("treats a degraded project skill as both, not either", () => {
    // `c` is degraded *and* from the project. A partition that moved it out of
    // the project list would make the counts add up and the answer wrong.
    const { project, attention } = partition(library, "all");
    expect(project.map((s) => s.name)).toContain("c");
    expect(attention.map((s) => s.name)).toContain("c");
  });

  it("survives an empty library without inventing a count", () => {
    const empty = partition([], "all");
    expect(empty.all).toEqual([]);
    expect(empty.attention).toEqual([]);
    expect(empty.shown).toEqual([]);
  });
});
