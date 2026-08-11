import { describe, expect, it } from "vitest";

import { breakdown, group, span } from "./Transcript";
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
