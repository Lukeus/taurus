// A reopened conversation's delegation cards: each gets its transcript back
// from the delegate's own header, which is the only place that says where it
// was written.
import { describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {
    onmessage: unknown;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { useStore } from "./store";

const DELEGATED = [
  { role: "user", content: [{ type: "text", text: "look into it" }] },
  {
    role: "assistant",
    content: [
      { type: "tool_use", id: "d1", name: "spawn_subagent", input: { agent_type: "explorer" } },
      { type: "tool_use", id: "d2", name: "spawn_subagent", input: { agent_type: "explorer" } },
    ],
  },
  {
    role: "user",
    content: [
      { type: "tool_result", tool_use_id: "d1", content: [{ type: "text", text: "found it" }], is_error: false },
      { type: "tool_result", tool_use_id: "d2", content: [{ type: "text", text: "nothing" }], is_error: false },
    ],
  },
];

const delegate = (id: string, call?: string) => ({
  id,
  workspace: "/w",
  model: "m",
  started: 0,
  updated: 0,
  title: "",
  agent: "explorer",
  call,
});

describe("reopening a conversation that delegated", () => {
  it("puts each transcript back on the card of the call that started it", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "resume_session") {
        return Promise.resolve({
          id: "parent",
          model: "m",
          provider_id: "ollama",
          native_tools: true,
          vision: false,
          context_length: 32_000,
          messages: DELEGATED,
          switches: [],
          // `d2` was recorded before delegates kept their call: no link.
          delegates: [delegate("child1", "d1")],
        });
      }
      if (command === "attach_session") return Promise.resolve({ turn: null, dropped: 0 });
      return Promise.resolve(["list_sessions", "list_checkpoints"].includes(command) ? [] : undefined);
    });

    await useStore.getState().resume("parent");

    const cards = useStore.getState().entries.filter((e) => e.kind === "tool");
    expect(cards.find((c) => c.id === "d1")).toMatchObject({
      transcript: { session: "child1", agent: "explorer" },
    });
    expect(cards.find((c) => c.id === "d2")).not.toHaveProperty("transcript");
  });
});
