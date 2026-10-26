import { describe, expect, it } from "vitest";

import type { DelegateStep, UiEvent } from "./api";
import {
  currentTool,
  MAX_AGENT_TEXT,
  mergeAgents,
  NO_AGENTS,
  stateOf,
  type Agents,
} from "./agents";

const step = (id: string, s: DelegateStep): UiEvent => ({
  type: "delegate_activity",
  id,
  step: s,
});

const run = (events: UiEvent[]): Agents => events.reduce(mergeAgents, NO_AGENTS);

describe("mergeAgents", () => {
  it("builds a delegate's run from its steps", () => {
    const agents = run([
      step("c1", { kind: "started", agent: "explorer", model: "qwen3", max_rounds: 20, at: 1_000 }),
      { type: "tool_transcript", id: "c1", session: "child-1", agent: "explorer" },
      step("c1", { kind: "round", n: 1 }),
      step("c1", { kind: "text", text: "Looking " }),
      step("c1", { kind: "text", text: "around." }),
      step("c1", { kind: "tool_started", call: "r1", name: "grep", preview: "grep port", at: 1_100 }),
      step("c1", { kind: "tool_finished", call: "r1", ok: true, at: 1_400 }),
      step("c1", { kind: "round", n: 2 }),
      step("c1", { kind: "finished", input_tokens: 900, output_tokens: 40, at: 2_000 }),
    ]);
    const c1 = agents.c1;
    expect(c1.agent).toBe("explorer");
    expect(c1.session).toBe("child-1");
    expect(c1.maxRounds).toBe(20);
    expect(c1.round).toBe(2);
    // A break between rounds, so two rounds don't read as one sentence.
    expect(c1.text).toBe("Looking around.\n\n");
    expect(c1.tools).toEqual([
      { call: "r1", name: "grep", preview: "grep port", startedAt: 1_100, ok: true, endedAt: 1_400 },
    ]);
    expect(c1.usage).toEqual({ input: 900, output: 40 });
    expect(stateOf(c1)).toBe("done");
  });

  it("keeps concurrent delegates apart", () => {
    const agents = run([
      step("a", { kind: "text", text: "first" }),
      step("b", { kind: "text", text: "second" }),
      step("a", { kind: "text", text: " still" }),
    ]);
    expect(agents.a.text).toBe("first still");
    expect(agents.b.text).toBe("second");
  });

  it("hands back the same object for an event about nothing it holds", () => {
    const before = run([step("a", { kind: "round", n: 1 })]);
    expect(mergeAgents(before, { type: "text_delta", text: "hi" })).toBe(before);
  });

  it("keeps the end of a long run of prose", () => {
    const long = "x".repeat(MAX_AGENT_TEXT + 10);
    const agents = run([step("a", { kind: "text", text: long }), step("a", { kind: "text", text: "END" })]);
    expect(agents.a.text.length).toBe(MAX_AGENT_TEXT);
    expect(agents.a.text.endsWith("END")).toBe(true);
  });

  it("says a delegate waiting for a slot is queued, not running", () => {
    const agents = run([step("a", { kind: "queued", at: 5 })]);
    expect(stateOf(agents.a)).toBe("queued");
    const started = mergeAgents(
      agents,
      step("a", { kind: "started", agent: "explorer", model: "m", max_rounds: 5, at: 9 }),
    );
    expect(stateOf(started.a)).toBe("running");
  });

  it("takes how it stopped from its report", () => {
    const agents = run([
      step("a", { kind: "started", agent: "coder", model: "m", max_rounds: 5, at: 1 }),
      {
        type: "delegate_report",
        id: "a",
        report: { disposition: "blocked", owner: "user", needs: "pick one", summary: "", files: [] },
      },
    ]);
    expect(stateOf(agents.a)).toBe("blocked");
  });

  it("knows which call it's in the middle of", () => {
    const agents = run([
      step("a", { kind: "tool_started", call: "1", name: "read_file", preview: "read a", at: 1 }),
      step("a", { kind: "tool_finished", call: "1", ok: true, at: 2 }),
      step("a", { kind: "tool_started", call: "2", name: "grep", preview: "grep b", at: 3 }),
    ]);
    expect(currentTool(agents.a)?.call).toBe("2");
  });
});
