import { describe, expect, it } from "vitest";

import type { Message, UiEvent } from "../lib/api";
import { entriesFromMessages, reduce, type Entry } from "./store";

/** Folds a whole event sequence, as a real turn would arrive. */
const run = (...events: UiEvent[]): Entry[] => events.reduce(reduce, []);

const text = (t: string): UiEvent => ({ type: "text_delta", text: t });

describe("transcript reducer", () => {
  it("merges consecutive text deltas into one entry", () => {
    const entries = run(text("Hello "), text("there"));
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({ kind: "assistant", text: "Hello there" });
  });

  it("keeps reasoning in the same entry as the answer but in its own field", () => {
    // One bubble with a collapsible reasoning section, not two bubbles.
    const entries = run(
      { type: "thinking_delta", text: "let me think" },
      text("the answer"),
    );
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      kind: "assistant",
      thinking: "let me think",
      text: "the answer",
    });
  });

  it("opens a tool entry and completes it in place", () => {
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "read_file", preview: "Read a.txt" },
      { type: "tool_call_finished", id: "t1", ok: true, output: "contents" },
    );
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      kind: "tool",
      id: "t1",
      name: "read_file",
      preview: "Read a.txt",
      status: "ok",
      output: "contents",
    });
  });

  it("times the call so a run of steps can report how long it took", () => {
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "run_command", preview: "run" },
      { type: "tool_call_finished", id: "t1", ok: true, output: "" },
    );
    const tool = entries[0] as Extract<Entry, { kind: "tool" }>;
    expect(tool.startedAt).toEqual(expect.any(Number));
    expect(tool.endedAt).toBeGreaterThanOrEqual(tool.startedAt!);
  });

  it("collects a delegation's progress under the call it belongs to", () => {
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "spawn_subagent", preview: "Delegate" },
      { type: "tool_progress", id: "t1", label: "read_file config.rs" },
      { type: "tool_progress", id: "t1", label: "grep load_config" },
    );
    const tool = entries[0] as Extract<Entry, { kind: "tool" }>;
    expect(tool.steps).toEqual(["read_file config.rs", "grep load_config"]);
  });

  it("does not attach progress to a different call", () => {
    // Two delegations can run at once, and steps landing on the wrong card
    // would read as one agent doing all the work.
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "spawn_subagent", preview: "A" },
      { type: "tool_call_started", id: "t2", name: "spawn_subagent", preview: "B" },
      { type: "tool_progress", id: "t2", label: "only B did this" },
    );
    const [a, b] = entries as Extract<Entry, { kind: "tool" }>[];
    expect(a.steps).toEqual([]);
    expect(b.steps).toEqual(["only B did this"]);
  });

  it("leaves a resumed call untimed rather than inventing a duration", () => {
    // The transcript on disk records what happened, not when. A run of
    // replayed steps must show its shape without claiming a wall-clock time.
    const [tool] = entriesFromMessages([
      {
        role: "assistant",
        content: [{ type: "tool_use", id: "t1", name: "read_file", input: {} }],
      },
    ]) as Extract<Entry, { kind: "tool" }>[];
    expect(tool.startedAt).toBeUndefined();
    expect(tool.endedAt).toBeUndefined();
  });

  it("marks a failed tool call as an error", () => {
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "read_file", preview: "Read x" },
      { type: "tool_call_finished", id: "t1", ok: false, output: "not found" },
    );
    expect(entries[0]).toMatchObject({ status: "error" });
  });

  it("completes the right call when several are in flight", () => {
    const entries = run(
      { type: "tool_call_started", id: "a", name: "glob", preview: "glob" },
      { type: "tool_call_started", id: "b", name: "grep", preview: "grep" },
      { type: "tool_call_finished", id: "b", ok: true, output: "hit" },
    );
    expect(entries.find((e) => e.id === "a")).toMatchObject({ status: "running" });
    expect(entries.find((e) => e.id === "b")).toMatchObject({ status: "ok" });
  });

  it("starts a new text entry after a tool call rather than reopening the old one", () => {
    const entries = run(
      text("first, let me look"),
      { type: "tool_call_started", id: "t1", name: "list_dir", preview: "list" },
      { type: "tool_call_finished", id: "t1", ok: true, output: "files" },
      text("here is what I found"),
    );
    const assistants = entries.filter((e) => e.kind === "assistant");
    expect(assistants).toHaveLength(2);
    expect(assistants[0]).toMatchObject({ text: "first, let me look" });
    expect(assistants[1]).toMatchObject({ text: "here is what I found" });
  });

  it("reports compaction to the user", () => {
    const entries = run({ type: "compacted", messages_removed: 12 });
    expect(entries[0]).toMatchObject({ kind: "notice", tone: "info" });
    expect((entries[0] as { text: string }).text).toContain("12");
  });

  it("surfaces errors as an error notice", () => {
    const entries = run({ type: "error", message: "provider unreachable" });
    expect(entries[0]).toMatchObject({
      kind: "notice",
      tone: "error",
      text: "provider unreachable",
    });
  });

  it("ignores events that carry no transcript content", () => {
    const entries = run(
      { type: "iteration_started", iteration: 1 },
      {
        type: "turn_finished",
        stop_reason: "end_turn",
        usage: { input_tokens: 10, output_tokens: 5 },
      },
    );
    expect(entries).toEqual([]);
  });

  it("handles a full multi-step turn in order", () => {
    const entries = run(
      { type: "iteration_started", iteration: 1 },
      text("Checking."),
      { type: "tool_call_started", id: "t1", name: "read_file", preview: "Read a" },
      { type: "tool_call_finished", id: "t1", ok: true, output: "body" },
      { type: "iteration_started", iteration: 2 },
      text("Done."),
      {
        type: "turn_finished",
        stop_reason: "end_turn",
        usage: { input_tokens: 1, output_tokens: 1 },
      },
    );
    expect(entries.map((e) => e.kind)).toEqual(["assistant", "tool", "assistant"]);
  });
});

describe("replaying a saved conversation", () => {
  const saved: Message[] = [
    { role: "user", content: [{ type: "text", text: "check the readme" }] },
    {
      role: "assistant",
      content: [
        { type: "thinking", text: "I should read it first." },
        { type: "text", text: "Checking." },
        {
          type: "tool_use",
          id: "t1",
          name: "read_file",
          input: { path: "README.md" },
        },
      ],
    },
    {
      role: "user",
      content: [
        { type: "tool_result", tool_use_id: "t1", content: "body", is_error: false },
      ],
    },
    { role: "assistant", content: [{ type: "text", text: "Done." }] },
  ];

  it("produces the same shape a streamed turn would have", () => {
    // A resumed conversation must be indistinguishable from a live one, or the
    // transcript visibly changes the moment you reopen it.
    const entries = entriesFromMessages(saved);
    expect(entries.map((e) => e.kind)).toEqual([
      "user",
      "assistant",
      "tool",
      "assistant",
    ]);
  });

  it("reunites a tool call with the result that answers it", () => {
    const entries = entriesFromMessages(saved);
    expect(entries[2]).toMatchObject({
      kind: "tool",
      name: "read_file",
      status: "ok",
      output: "body",
    });
  });

  it("marks a failed tool call as failed", () => {
    const entries = entriesFromMessages([
      saved[1],
      {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "t1", content: "nope", is_error: true },
        ],
      },
    ]);
    expect(entries.find((e) => e.kind === "tool")).toMatchObject({
      status: "error",
      output: "nope",
    });
  });

  it("leaves a call whose result never reached disk running", () => {
    // What a crash mid-turn looks like on reload; it must not read as success.
    const entries = entriesFromMessages([saved[0], saved[1]]);
    expect(entries.find((e) => e.kind === "tool")).toMatchObject({
      status: "running",
    });
  });

  it("keeps reasoning attached to the answer it belongs to", () => {
    const entries = entriesFromMessages(saved);
    expect(entries[1]).toMatchObject({
      kind: "assistant",
      thinking: "I should read it first.",
      text: "Checking.",
      open: false,
    });
  });

  it("closes every assistant entry so the next turn opens a new one", () => {
    const entries = entriesFromMessages(saved);
    const open = entries.filter((e) => e.kind === "assistant" && e.open);
    expect(open).toHaveLength(0);
  });

  it("replays an empty transcript as an empty view", () => {
    expect(entriesFromMessages([])).toEqual([]);
  });
});
