import { describe, expect, it } from "vitest";
import type { SessionMeta } from "./api";
import { branchesOf, branchLabel, forkPlace } from "./branches";

const meta = (id: string, from?: string, checkpoint = 2): SessionMeta =>
  ({
    id,
    forked_from: from ? { session: from, turn: "t", checkpoint } : undefined,
  }) as SessionMeta;

describe("branchesOf", () => {
  it("finds every branch of a tree of forks, from any of them, and nothing else", () => {
    const sessions = [
      meta("root"),
      meta("a", "root"),
      meta("b", "root"),
      meta("aa", "a"),
      meta("other"),
      meta("other-fork", "other"),
    ];
    const ids = (id: string) => branchesOf(sessions, id).map((s) => s.id);
    expect(ids("aa")).toEqual(["root", "a", "b"]);
    expect(ids("root")).toEqual(["a", "b", "aa"]);
    expect(ids("other")).toEqual(["other-fork"]);
  });

  it("is empty for a conversation that was never forked", () => {
    expect(branchesOf([meta("solo"), meta("x")], "solo")).toEqual([]);
  });
});

describe("branchLabel", () => {
  it("says where a fork was made, and which one is the original", () => {
    expect(branchLabel(meta("root"))).toBe("the original");
    expect(branchLabel(meta("a", "root", 3))).toBe("fork at turn 3");
    expect(
      forkPlace({ session: "root", turn: "t", checkpoint: 3, read_only: true }),
    ).toBe("after turn 2");
  });
});
