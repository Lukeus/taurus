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

  it("keeps reading a name through a capital or an underscore", () => {
    // The bug this closes: a name is whatever its author wrote, and neither of
    // these used to be part of a name as far as this was concerned. Typing one
    // returned null, which the composer reads as "not in a command" — so the
    // menu did not narrow, it disappeared, and the only way back was to delete
    // the character.
    expect(commandQuery("/Release")).toBe("Release");
    expect(commandQuery("/my_skill")).toBe("my_skill");
    expect(commandQuery("/A")).toBe("A");
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

  it("finds a name whatever case it was written in", () => {
    // Nothing lowercases a name on the way in — it is the author's frontmatter
    // verbatim, and a skill borrowed from another client is under no
    // obligation to be kebab-case. Lowering only the query meant every one of
    // these was reachable from the bare slash and unreachable the moment a
    // letter was typed.
    const mixed = [skill("Release-Notes"), skill("deploy"), agent("Reviewer")];
    expect(matches(mixed, "release").map((c) => c.name)).toEqual([
      "Release-Notes",
    ]);
    expect(matches(mixed, "rev").map((c) => c.name)).toEqual(["Reviewer"]);
    expect(matches(mixed, "notes").map((c) => c.name)).toEqual([
      "Release-Notes",
    ]);
  });

  it("ranks a prefix above an interior match whatever the case", () => {
    const mixed = [skill("speckit-Plan"), skill("Plan-release")];
    expect(matches(mixed, "plan").map((c) => c.name)).toEqual([
      "Plan-release",
      "speckit-Plan",
    ]);
  });

  it("matches a typed capital against a lowercase name", () => {
    // The other direction: the query is lowered too, so shift-typing a name
    // finds it rather than emptying the list.
    expect(matches([skill("deploy")], "Dep").map((c) => c.name)).toEqual([
      "deploy",
    ]);
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
