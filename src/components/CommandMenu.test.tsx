import { describe, expect, it } from "vitest";

import type { SkillSummary } from "../lib/api";
import { commandQuery, matches } from "./CommandMenu";

const skill = (name: string): SkillSummary => ({
  name,
  description: `does ${name}`,
  when_to_use: `when you need ${name}`,
  version: 1,
  tier: "project",
  origin: "claude",
  compatibility: null,
  allowed_tools: [],
  scripts: [],
  warnings: [],
  degraded: null,
  dir: `/ws/.claude/skills/${name}`,
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

describe("narrowing to a skill", () => {
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

  it("returns nothing for a name no skill has", () => {
    expect(matches(library, "nonsense")).toEqual([]);
  });
});
