// A batch of streamed pieces is folded as one: the same text as folding them
// one by one, at one copy of the conversation instead of one per piece.
import { describe, expect, it } from "vitest";

import { foldEvents, pinnedPlan, type Entry } from "./store";

const delta = (text: string) => ({ type: "text_delta", text }) as never;
const thought = (text: string) => ({ type: "thinking_delta", text }) as never;

describe("folding a batch of deltas", () => {
  it("gives the same text as folding them one at a time", () => {
    const pieces = ["The ", "release ", "build ", "is ", "slow."];
    const together = foldEvents(pieces.map(delta), []);
    const apart = pieces.reduce<Entry[]>((entries, p) => foldEvents([delta(p)], entries), []);
    expect(together).toHaveLength(1);
    expect(together[0]).toMatchObject({ kind: "assistant", text: "The release build is slow." });
    expect((apart[0] as { text: string }).text).toBe("The release build is slow.");
  });

  it("keeps thinking and text apart, in order", () => {
    const [entry] = foldEvents([thought("hm"), thought("m. "), delta("It's "), delta("LTO."), thought(" ok")], []);
    expect(entry).toMatchObject({ text: "It's LTO.", thinking: "hmm.  ok" });
  });
});

describe("the pinned plan's lookup", () => {
  it("is worked out once per transcript", () => {
    const entries: Entry[] = [
      {
        kind: "tool",
        id: "p",
        name: "update_plan",
        preview: "update_plan",
        status: "ok",
        steps: [],
        view: { type: "plan", steps: [{ text: "Read it", state: "todo" }] },
      } as never,
    ];
    expect(pinnedPlan(entries)).toBe(pinnedPlan(entries));
    expect(pinnedPlan([...entries])).toEqual(pinnedPlan(entries));
  });
});
