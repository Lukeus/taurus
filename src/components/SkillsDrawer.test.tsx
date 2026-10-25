import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import type { Instructions, SkillSummary } from "../lib/api";
import { InstructionsSection, originLabel, partition, skillActions } from "./SkillsDrawer";

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
  managed: false,
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
    expect(originLabel("agents", "user")).toBe(".agents");
    expect(originLabel("claude", "user")).toBe(".claude");
  });

  it("says nothing about a skill in Taurus's own location", () => {
    // A badge on every row is a badge nobody reads. The question the label
    // answers is "where did this come from", and `.taurus` is the answer that
    // needed no asking.
    expect(originLabel("taurus", "user")).toBeNull();
  });

  it("sends a Copilot skill to whichever of its two folders it is actually in", () => {
    // The one origin whose directories are not named the same. A repository's
    // sit in `.github` beside everything else GitHub reads; a person's in a
    // dotdir of Copilot's own. One label for both would send half of everyone
    // to a folder that does not exist.
    expect(originLabel("copilot", "project")).toBe(".github");
    expect(originLabel("copilot", "user")).toBe(".copilot");
  });
});

describe("the instructions section", () => {
  const brief = (patch: Partial<Instructions> = {}): Instructions => ({
    source: { tier: "project", origin: "agents", path: "/repo/AGENTS.md" },
    body: "Run the tests before saying you are done.",
    truncated: false,
    applies_to: null,
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

  it("shows the paths a scoped brief is actually about", () => {
    // Copilot's `*.instructions.md` files apply to a glob, not the workspace.
    // Listed beside the ones that always apply with nothing to tell them
    // apart, every row reads as unconditional.
    const html = renderToStaticMarkup(
      <InstructionsSection
        instructions={[
          brief({
            source: {
              tier: "project",
              origin: "copilot",
              path: "/repo/.github/instructions/rust.instructions.md",
            },
            applies_to: "**/*.rs",
          }),
        ]}
      />,
    );
    expect(html).toContain("**/*.rs");
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

describe("what a skill card offers", () => {
  it("opens and deletes a skill Taurus wrote, and offers no copy", () => {
    expect(skillActions(skill())).toEqual({
      open: "/home/me/.taurus/skills/release-notes/SKILL.md",
      copy: false,
      remove: true,
    });
  });

  it("opens another tool's skill and copies it, but never deletes it", () => {
    const actions = skillActions(skill({ origin: "claude" }));
    expect(actions.open).toBeTruthy();
    expect(actions.copy).toBe(true);
    expect(actions.remove).toBe(false);
  });

  it("only copies a fetched plugin's skill, whose files its next update replaces", () => {
    expect(skillActions(skill({ origin: "plugin", plugin: "ops", managed: true }))).toEqual({
      open: null,
      copy: true,
      remove: false,
    });
  });

  it("treats a skill in a plugin you made as yours", () => {
    const actions = skillActions(skill({ origin: "plugin", plugin: "kit", managed: false }));
    expect(actions.open).toBeTruthy();
    expect(actions.copy).toBe(false);
    expect(actions.remove).toBe(true);
  });

  it("opens a plugin command's own file", () => {
    const actions = skillActions(
      skill({ origin: "plugin_command", plugin: "kit", command_file: "/p/kit/commands/review.md" }),
    );
    expect(actions.open).toBe("/p/kit/commands/review.md");
    expect(actions.remove).toBe(false);
  });
});

describe("starting a brief", () => {
  const entry = (tier: "user" | "project", origin: Instructions["source"]["origin"]) =>
    ({
      source: { tier, origin, path: `/${tier}/${origin}.md` },
      body: "x",
      truncated: false,
      applies_to: null,
    }) as Instructions;

  it("offers a TAURUS.md only for a tier that has none", () => {
    const html = renderToStaticMarkup(
      <InstructionsSection
        instructions={[entry("project", "taurus"), entry("user", "claude")]}
        onStart={() => {}}
      />,
    );
    expect(html).toContain("Start a personal TAURUS.md");
    expect(html).not.toContain("Start a project TAURUS.md");
  });
});
