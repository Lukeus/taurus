import { describe, expect, it } from "vitest";

import type { UiEvent } from "../lib/api";
import { reduce, type Entry } from "./store";

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
    expect(entries).toEqual([
      {
        kind: "tool",
        id: "t1",
        name: "read_file",
        preview: "Read a.txt",
        status: "ok",
        output: "contents",
      },
    ]);
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
