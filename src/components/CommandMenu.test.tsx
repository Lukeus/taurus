import { describe, expect, it } from "vitest";

import type { CommandSummary } from "../lib/api";
import { commandQuery, matches } from "./CommandMenu";

const skill = (name: string): CommandSummary => ({
  name,
  kind: "skill",
  when_to_use: `when you need ${name}`,
});

const agent = (name: string): CommandSummary => ({
  name,
  kind: "agent",
  when_to_use: `delegate ${name} work`,
});

describe("reading the command being typed", () => {
  it("reads the name after a slash", () => {
    expect(commandQuery("/speck")).toBe("speck");
    expect(commandQuery("/")).toBe("");
  });

  it("stops once the name is settled", () => {
    // A space means the user has moved on to arguments, and a menu hovering
    // over a sentence being written is in the way.
    expect(commandQuery("/speckit-specify add dark mode")).toBeNull();
    expect(commandQuery("/speckit-specify ")).toBeNull();
  });

  it("ignores text that merely starts with a slash", () => {
    expect(commandQuery("/usr/bin/env")).toBeNull();
    expect(commandQuery("nope")).toBeNull();
    expect(commandQuery("")).toBeNull();
  });
});

describe("narrowing to a command", () => {
  const library = [
    skill("speckit-plan"),
    skill("speckit-specify"),
    skill("plan-release"),
  ];

  it("offers everything for a bare slash", () => {
    expect(matches(library, "").length).toBe(3);
  });

  it("ranks what the user is typing toward above an interior match", () => {
    // Both contain "plan". `plan-release` starts with it, so it is the one
    // being typed toward and Enter must not send the other.
    expect(matches(library, "plan").map((s) => s.name)).toEqual([
      "plan-release",
      "speckit-plan",
    ]);
  });

  it("narrows to one as the name is completed", () => {
    expect(matches(library, "speckit-s").map((s) => s.name)).toEqual([
      "speckit-specify",
    ]);
  });

  it("returns nothing for a name nothing has", () => {
    expect(matches(library, "nonsense")).toEqual([]);
  });

  it("ranks skills and agents against each other on the name alone", () => {
    // Which kind a name belongs to is settled by the harness before the list
    // gets here. Sorting agents to the bottom would make the highlighted row
    // depend on something the user did not type.
    const mixed = [skill("review-diff"), agent("reviewer"), skill("rewind")];
    expect(matches(mixed, "review").map((c) => c.name)).toEqual([
      "review-diff",
      "reviewer",
    ]);
  });
});
