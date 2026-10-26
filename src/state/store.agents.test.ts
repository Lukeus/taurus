// A reopened conversation's delegates, read back from their transcripts for
// the agents panel. Tauri is stood in for.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import type { Message, SessionMeta } from "../lib/api";
import { runFromMessages, useStore } from "./store";

const META = {
  id: "child-1",
  workspace: "/src/a",
  model: "qwen3",
  started: 1_700_000_000,
  updated: 1_700_000_042,
  title: "find the port",
  agent: "explorer",
  call: "c1",
} as unknown as SessionMeta;

const TRANSCRIPT = [
  { role: "user", content: [{ type: "text", text: "Find the port." }] },
  {
    role: "assistant",
    content: [
      { type: "text", text: "Looking at the config." },
      { type: "tool_use", id: "r1", name: "grep", input: { pattern: "port" } },
      { type: "tool_use", id: "r2", name: "read_file", input: { path: "missing.toml" } },
    ],
  },
  {
    role: "user",
    content: [
      { type: "tool_result", tool_use_id: "r1", content: [], is_error: false },
      { type: "tool_result", tool_use_id: "r2", content: [], is_error: true },
    ],
  },
  { role: "assistant", content: [{ type: "text", text: "It's 8080." }] },
] as unknown as Message[];

describe("runFromMessages", () => {
  it("reads its rounds, calls and words back, and says it was read back", () => {
    const run = runFromMessages("c1", META, TRANSCRIPT);
    expect(run.agent).toBe("explorer");
    expect(run.session).toBe("child-1");
    expect(run.round).toBe(2);
    expect(run.tools.map((t) => [t.name, t.ok])).toEqual([
      ["grep", true],
      ["read_file", false],
    ]);
    // No times on the calls: a transcript doesn't record them.
    expect(run.tools[0].startedAt).toBeUndefined();
    expect(run.text).toBe("Looking at the config.\n\nIt's 8080.");
    expect(run.startedAt).toBe(1_700_000_000_000);
    expect(run.finishedAt).toBe(1_700_000_042_000);
    expect(run.restored).toBe(true);
  });
});

describe("restoreAgents", () => {
  beforeEach(() => {
    invoke.mockReset();
    useStore.setState({
      session: { id: "parent" } as never,
      agents: {},
      delegates: [META],
    });
  });

  it("reads each delegate the window didn't watch, once", async () => {
    invoke.mockResolvedValue(TRANSCRIPT);
    await useStore.getState().restoreAgents();
    expect(invoke).toHaveBeenCalledWith("read_subagent_transcript", {
      sessionId: "parent",
      subagentId: "child-1",
    });
    expect(useStore.getState().agents.c1?.restored).toBe(true);

    await useStore.getState().restoreAgents();
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("leaves a run this window watched alone", async () => {
    const watched = { id: "c1", round: 5, tools: [], text: "live", thinking: "", detached: false };
    useStore.setState({ agents: { c1: watched } });
    await useStore.getState().restoreAgents();
    expect(invoke).not.toHaveBeenCalled();
    expect(useStore.getState().agents.c1).toBe(watched);
  });

  it("says why a transcript couldn't be read", async () => {
    invoke.mockRejectedValue("no such file");
    await useStore.getState().restoreAgents();
    expect(useStore.getState().agents.c1?.error).toContain("no such file");
  });
});
