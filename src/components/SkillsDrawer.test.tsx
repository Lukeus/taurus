import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import type { Instructions, SkillSummary } from "../lib/api";
import { InstructionsSection, originLabel, partition } from "./SkillsDrawer";

const skill = (patch: Partial<SkillSummary> = {}): SkillSummary => ({
  name: "release-notes",
  description: "Assemble release notes",
  when_to_use: "Preparing release notes",
  version: 1,
  tier: "user",
  origin: "taurus",
  compatibility: null,
  allowed_tools: [],
  scripts: [],
  warnings: [],
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

  it("surfaces a skill that only loaded because Taurus was lenient", () => {
    // It runs, so it is not degraded — but a name that disagrees with its
    // directory is exactly what this filter exists to help someone find.
    const lenient = skill({ name: "d", warnings: ["directory is named 'x'"] });
    const { attention } = partition([...library, lenient], "all");
    expect(attention.map((s) => s.name)).toEqual(["c", "d"]);
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

describe("skill origin", () => {
  it("names the directory a borrowed skill came from", () => {
    expect(originLabel("agents")).toBe(".agents");
    expect(originLabel("claude")).toBe(".claude");
  });

  it("says nothing about a skill in Taurus's own location", () => {
    // A badge on every row is a badge nobody reads. The question the label
    // answers is "where did this come from", and `.taurus` is the answer that
    // needed no asking.
    expect(originLabel("taurus")).toBeNull();
  });
});

describe("the instructions section", () => {
  const brief = (patch: Partial<Instructions> = {}): Instructions => ({
    source: { tier: "project", origin: "agents", path: "/repo/AGENTS.md" },
    body: "Run the tests before saying you are done.",
    truncated: false,
    ...patch,
  });

  it("names each file that is already in the prompt", () => {
    // A brief applies to every turn whether or not anyone remembers writing
    // it. A file read silently is what makes behaviour inexplicable.
    const html = renderToStaticMarkup(
      <InstructionsSection instructions={[brief()]} />,
    );
    expect(html).toContain("/repo/AGENTS.md");
    expect(html).toContain("project");
  });

  it("separates a personal brief from a project one", () => {
    // They carry different weight when they disagree, and one of them came
    // from a repository the user may have just cloned.
    const html = renderToStaticMarkup(
      <InstructionsSection
        instructions={[
          brief({
            source: {
              tier: "user",
              origin: "claude",
              path: "/home/me/.claude/CLAUDE.md",
            },
          }),
        ]}
      />,
    );
    expect(html).toContain("personal");
  });

  it("says when a file did not arrive whole", () => {
    const html = renderToStaticMarkup(
      <InstructionsSection instructions={[brief({ truncated: true })]} />,
    );
    expect(html).toContain("truncated");
  });

  it("says where to put one when there is none", () => {
    // An empty section reads as a feature that is broken rather than unused.
    const html = renderToStaticMarkup(<InstructionsSection instructions={[]} />);
    expect(html).toContain("AGENTS.md");
    expect(html).toContain("Rescan");
  });
});
