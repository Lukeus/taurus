import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import type { AgentSummary } from "../lib/api";
import { chips, partition } from "./AgentsDrawer";

const agent = (patch: Partial<AgentSummary> = {}): AgentSummary => ({
  name: "reviewer",
  description: "Reviews a diff for correctness bugs",
  tier: "user",
  tools: ["read_file", "grep"],
  max_iterations: 20,
  model: null,
  provider: null,
  forks_on_edit: false,
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

  it("narrows to the built-ins, which are the ones with no file to open", () => {
    const { shown } = partition(roster, "builtin");
    expect(shown.map((a) => a.name)).toEqual(["explorer"]);
  });

  it("finds the broken one, which is the reason to open this list", () => {
    const { shown } = partition(roster, "attention");
    expect(shown.map((a) => a.name)).toEqual(["c"]);
  });

  it("counts the same regardless of which filter is showing", () => {
    // The pills read their counts off this, so a count that moved with the
    // filter would have the list disagree with the label above it.
    for (const filter of ["all", "builtin", "attention"] as const) {
      const counts = partition(roster, filter);
      expect(counts.all).toHaveLength(4);
      expect(counts.builtin).toHaveLength(1);
      expect(counts.attention).toHaveLength(1);
    }
  });

  it("survives an empty roster without inventing a count", () => {
    const empty = partition([], "all");
    expect(empty.all).toEqual([]);
    expect(empty.attention).toEqual([]);
    expect(empty.shown).toEqual([]);
  });
});

describe("tool chips on a card", () => {
  it("shows a short scope in full", () => {
    expect(chips(["read_file", "grep"])).toEqual({
      shown: ["read_file", "grep"],
      hidden: 0,
    });
  });

  it("counts what it had to leave out rather than truncating silently", () => {
    // A card that showed three of eleven tools and said nothing would read as
    // a read-only agent that can in fact write.
    const { shown, hidden } = chips(["a", "b", "c", "d", "e"]);
    expect(shown).toEqual(["a", "b", "c"]);
    expect(hidden).toBe(2);
  });

  it("never reports a negative overflow", () => {
    expect(chips([]).hidden).toBe(0);
  });
});
