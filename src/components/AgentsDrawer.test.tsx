import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import type { AgentSummary } from "../lib/api";
import { partition } from "./AgentsDrawer";

const agent = (patch: Partial<AgentSummary> = {}): AgentSummary => ({
  name: "reviewer",
  description: "Reviews a diff for correctness bugs",
  tier: "user",
  tools: ["read_file", "grep"],
  max_iterations: 20,
  model: null,
  provider: null,
  shadows: null,
  degraded: null,
  path: "/home/me/.taurus/agents/reviewer.md",
  ...patch,
});

describe("agent filters", () => {
  const roster = [
    agent({ name: "explorer", tier: "builtin", path: null }),
    agent({ name: "a", tier: "user" }),
    agent({ name: "b", tier: "project" }),
    agent({ name: "c", tier: "project", degraded: "cannot use run_command" }),
  ];

  it("counts every agent under all, built-ins included", () => {
    // The built-ins are agents like any other here: they can be shadowed, and
    // hiding them would make the roster disagree with what the model sees.
    expect(partition(roster, "all").shown).toHaveLength(4);
  });

  it("narrows to what this project added", () => {
    const { shown } = partition(roster, "project");
    expect(shown.map((a) => a.name)).toEqual(["b", "c"]);
  });

  it("finds the broken one, which is the reason to open this list", () => {
    const { shown } = partition(roster, "attention");
    expect(shown.map((a) => a.name)).toEqual(["c"]);
  });

  it("counts the same regardless of which filter is showing", () => {
    for (const filter of ["all", "project", "attention"] as const) {
      const counts = partition(roster, filter);
      expect(counts.all).toHaveLength(4);
      expect(counts.project).toHaveLength(2);
      expect(counts.attention).toHaveLength(1);
    }
  });

  it("treats a degraded project agent as both, not either", () => {
    const { project, attention } = partition(roster, "all");
    expect(project.map((a) => a.name)).toContain("c");
    expect(attention.map((a) => a.name)).toContain("c");
  });

  it("survives an empty roster without inventing a count", () => {
    const empty = partition([], "all");
    expect(empty.all).toEqual([]);
    expect(empty.attention).toEqual([]);
    expect(empty.shown).toEqual([]);
  });
});
