import { describe, expect, it } from "vitest";

import { breakdown, busyWith, group, reuse, span, turns } from "./Transcript";
import type { Entry } from "../state/store";

type ToolEntry = Extract<Entry, { kind: "tool" }>;

const tool = (
  id: string,
  name: string,
  times?: { startedAt: number; endedAt?: number },
): ToolEntry => ({
  kind: "tool",
  id,
  name,
  preview: `${name} something`,
  status: "ok",
  steps: [],
  ...times,
});

const say = (id: string, text: string): Entry => ({
  kind: "assistant",
  id,
  text,
  thinking: "",
  open: false,
});

describe("grouping a turn", () => {
  it("folds consecutive tool calls into one run", () => {
    const grouped = group([
      say("a", "Checking."),
      tool("t1", "grep"),
      tool("t2", "read_file"),
      tool("t3", "edit_file"),
      say("b", "Done."),
    ]);
    expect(grouped).toHaveLength(3);
    expect(Array.isArray(grouped[1]) && grouped[1]).toHaveLength(3);
  });

  it("leaves a call that drew something standing on its own", () => {
    // Folded into the run, the answer would be filed under "4 steps" behind a
    // disclosure triangle, one click further away than the work that made it.
    const drew: ToolEntry = {
      ...tool("t3", "show_table"),
      view: {
        type: "table",
        title: "Crates by build time",
        caption: null,
        columns: [{ label: "Crate", kind: "text" }],
        rows: [["taurus-core"]],
      },
    };
    const grouped = group([tool("t1", "grep"), tool("t2", "read_file"), drew]);

    expect(grouped).toHaveLength(2);
    expect(Array.isArray(grouped[0]) && grouped[0]).toHaveLength(2);
    expect(grouped[1]).toBe(drew);
  });

  it("does not swallow the run that follows a drawn one", () => {
    const drew: ToolEntry = {
      ...tool("t1", "show_chart"),
      view: {
        type: "chart",
        title: "Turns",
        caption: null,
        labels: ["t1"],
        series: [{ name: "calls", unit: "", values: [4] }],
      },
    };
    const grouped = group([drew, tool("t2", "grep"), tool("t3", "read_file")]);

    expect(grouped).toHaveLength(2);
    expect(Array.isArray(grouped[1]) && grouped[1]).toHaveLength(2);
  });

  it("starts a new run after the model speaks between calls", () => {
    // Two runs separated by a sentence are two steps of the conversation, and
    // merging them would put the sentence after work it came before.
    const grouped = group([
      tool("t1", "grep"),
      say("a", "Found it."),
      tool("t2", "edit_file"),
    ]);
    expect(grouped.map((g) => (Array.isArray(g) ? "run" : g.kind))).toEqual([
      "run",
      "assistant",
      "run",
    ]);
  });

  it("groups a lone call too, so it does not change shape when a second arrives", () => {
    const grouped = group([tool("t1", "read_file")]);
    expect(Array.isArray(grouped[0])).toBe(true);
  });

  it("leaves a transcript with no tool calls alone", () => {
    const entries = [say("a", "Hello"), say("b", "Bye")];
    expect(group(entries)).toEqual(entries);
  });

  it("does not swallow a notice that lands mid-run", () => {
    // A compaction or an error between two calls has to stay where it fell.
    const grouped = group([
      tool("t1", "grep"),
      { kind: "notice", id: "n", tone: "error", text: "provider unreachable" },
      tool("t2", "grep"),
    ]);
    expect(grouped).toHaveLength(3);
  });
});

describe("the shape of a run", () => {
  it("counts steps by what they did to the workspace", () => {
    expect(
      breakdown([
        tool("t1", "read_file"),
        tool("t2", "grep"),
        tool("t3", "edit_file"),
        tool("t4", "run_command"),
      ]),
    ).toBe("2 read · 1 edited · 1 command");
  });

  it("pluralises the countable nouns and not the participles", () => {
    expect(breakdown([tool("t1", "run_command"), tool("t2", "run_command")])).toBe(
      "2 commands",
    );
    expect(breakdown([tool("t1", "edit_file"), tool("t2", "write_file")])).toBe(
      "2 edited",
    );
  });

  it("counts both web tools as requests, because both left the machine", () => {
    expect(breakdown([tool("t1", "web_search"), tool("t2", "fetch_url")])).toBe(
      "2 requests",
    );
    expect(breakdown([tool("t1", "web_search"), tool("t2", "read_file")])).toBe(
      "1 request · 1 read",
    );
  });

  it("counts a tool it does not know about rather than dropping it", () => {
    // MCP servers contribute tools this table has never heard of.
    expect(breakdown([tool("t1", "mcp__filesystem__move")])).toBe("1 tool");
  });
});

describe("how long a run took", () => {
  it("spans the first start to the last finish", () => {
    expect(
      span([
        tool("t1", "grep", { startedAt: 1_000, endedAt: 3_000 }),
        tool("t2", "read_file", { startedAt: 3_000, endedAt: 9_000 }),
      ]),
    ).toBe(8_000);
  });

  it("reports nothing for a replayed run rather than guessing", () => {
    expect(span([tool("t1", "grep")])).toBeNull();
  });

  it("reports nothing while a step is still running", () => {
    expect(
      span([
        tool("t1", "grep", { startedAt: 1_000, endedAt: 2_000 }),
        tool("t2", "run_command", { startedAt: 2_000 }),
      ]),
    ).toBeNull();
  });

  it("does not mistake a partly-replayed run for a timed one", () => {
    // One live call appended to a resumed transcript must not report the
    // live call's duration as the whole run's.
    expect(
      span([tool("t1", "grep"), tool("t2", "edit_file", { startedAt: 5, endedAt: 9 })]),
    ).toBeNull();
  });
});

const ask = (id: string, text: string): Entry => ({ kind: "user", id, text });

describe("cutting the transcript into turns", () => {
  it("heads each turn with the question that began it", () => {
    const [first, second] = turns([
      ask("u1", "why is it slow?"),
      say("a1", "Timing it."),
      ask("u2", "and now?"),
      say("a2", "Better."),
    ]);
    expect(first.prompt?.id).toBe("u1");
    expect(first.body).toEqual([expect.objectContaining({ id: "a1" })]);
    expect(second.prompt?.id).toBe("u2");
    expect(second.body).toEqual([expect.objectContaining({ id: "a2" })]);
  });

  it("folds a turn's tool calls into runs, as the flat list did", () => {
    // The two groupings have to agree: a run is still a run inside a turn, and
    // the rail hangs one segment off it rather than one per call.
    const [turn] = turns([
      ask("u1", "build it"),
      tool("t1", "grep"),
      tool("t2", "read_file"),
      say("a1", "Done."),
    ]);
    expect(turn.body).toHaveLength(2);
    expect(turn.body[0]).toEqual([
      expect.objectContaining({ id: "t1" }),
      expect.objectContaining({ id: "t2" }),
    ]);
  });

  it("does not carry a run across the question that interrupted it", () => {
    const [first, second] = turns([
      ask("u1", "one"),
      tool("t1", "grep"),
      ask("u2", "two"),
      tool("t2", "grep"),
    ]);
    expect(first.body).toEqual([[expect.objectContaining({ id: "t1" })]]);
    expect(second.body).toEqual([[expect.objectContaining({ id: "t2" })]]);
  });

  it("puts what precedes the first question in a turn with no question", () => {
    // The note a session opens with when its model has no native tool calling.
    // A thread hanging off nothing would say it belonged to something nobody
    // asked, so this turn draws no rail — see `.turn.unprompted`.
    const [preamble, asked] = turns([
      { kind: "notice", id: "n1", tone: "info", text: "prompted tools" },
      ask("u1", "go"),
      say("a1", "ok"),
    ]);
    expect(preamble.prompt).toBeNull();
    expect(preamble.body).toEqual([expect.objectContaining({ id: "n1" })]);
    expect(asked.prompt?.id).toBe("u1");
  });

  it("has nothing to draw for an empty conversation", () => {
    expect(turns([])).toEqual([]);
  });
});

/**
 * The transcript is memoized per turn, and a memo compares what it is handed.
 * These are the tests that say the comparison can succeed: without carried
 * identity every turn looks new on every token, the memo never skips anything,
 * and drawing one word costs a redraw of the whole conversation.
 */
describe("a note kept for the next conversation", () => {
  it("is counted as a note rather than folded in with the reads", () => {
    // `remember` writes nothing in the workspace, so it is not an edit — but
    // calling it a read would file the one step whose effect lands on the next
    // conversation under the background noise of the run.
    expect(breakdown([tool("t1", "remember"), tool("t2", "read_file")])).toContain("note");
  });
});

describe("carrying turns forward", () => {
  const conversation: Entry[] = [
    ask("u1", "first question"),
    say("a1", "first answer"),
    tool("t1", "read_file"),
    ask("u2", "second question"),
  ];

  it("keeps the turns a token did not touch", () => {
    const before = turns(conversation);
    // What a `text_delta` does: the entries already there are the same objects,
    // and one new one joins the last turn.
    const after = reuse(before, turns([...conversation, say("a2", "wor")]));

    expect(after[0]).toBe(before[0]);
    expect(after[1]).not.toBe(before[1]);
  });

  it("sees through the folding of a run of tool calls", () => {
    // `group` rebuilds its arrays on every call, so a run of tool calls never
    // matches by identity even when nothing in it moved. Comparing the steps
    // rather than the array holding them is what keeps a turn that ran commands
    // from being redrawn for the rest of the conversation.
    const before = turns(conversation);
    const after = reuse(before, turns([...conversation]));

    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(before[1]);
  });

  it("gives up the turn a tool call finished in", () => {
    const finished: Entry[] = conversation.map((e) =>
      e.id === "t1" ? { ...e, status: "error" as const } : e,
    );
    const before = turns(conversation);
    const after = reuse(before, turns(finished));

    expect(after[0]).not.toBe(before[0]);
  });

  it("has nothing to carry forward on the first render", () => {
    const built = turns(conversation);
    expect(reuse([], built)).toEqual(built);
  });
});

describe("what a turn is busy with", () => {
  /** A tool entry in whatever state, so a turn can be built around it. */
  const step = (
    name: string,
    status: "running" | "ok" | "error",
  ): Entry => ({ kind: "tool", id: `${name}-${status}`, name, preview: name, status, steps: [] });

  const turn = (body: Entry[]) => turns([{ kind: "user", id: "u", text: "go" }, ...body])[0];

  it("reports the category of the call that is still running", () => {
    // The category, not the name — it is what picks the waveform's shape, and
    // it is the same classification the row's own glyph is coloured by.
    expect(busyWith(turn([step("read_file", "running")]))).toBe("read");
    expect(busyWith(turn([step("edit_file", "running")]))).toBe("wrote");
    expect(busyWith(turn([step("run_command", "running")]))).toBe("ran");
  });

  it("reads the last call and not an earlier one", () => {
    // One call at a time: anything before the last has finished, and a turn
    // that showed the shape of a call it stopped making three seconds ago
    // would be describing the past.
    const t = turn([step("read_file", "ok"), step("edit_file", "running")]);
    expect(busyWith(t)).toBe("wrote");
  });

  it("says nothing when no call is running, which is a turn thinking", () => {
    expect(busyWith(turn([step("read_file", "ok")]))).toBeNull();
    expect(busyWith(turn([say("a", "Considering.")]))).toBeNull();
    expect(busyWith(turn([]))).toBeNull();
  });

  it("falls back to a category for a tool nothing has classified", () => {
    // An MCP tool, or one added since. It still gets a shape rather than
    // dropping to the thinking one, which would say the turn had paused.
    expect(busyWith(turn([step("acme__deploy", "running")]))).toBe("other");
  });
});
