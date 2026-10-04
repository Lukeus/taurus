// A conversation Taurus stopped in the middle of: what the store offers, and
// what continuing it sends. Actions rather than the reducer, so Tauri is
// stood in for, as in `store.queue.test.ts`.
import { afterEach, beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {
    onmessage: unknown;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { CONTINUE_PROMPT, useStore } from "./store";

const LISTS = new Set(["list_sessions", "list_checkpoints", "running_sessions"]);

function idle(command: string): Promise<unknown> {
  if (command === "attach_session") return Promise.resolve({ turn: null, dropped: 0 });
  return Promise.resolve(LISTS.has(command) ? [] : undefined);
}

let warn: MockInstance<typeof console.warn>;
beforeEach(() => {
  warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  invoke.mockReset();
  invoke.mockImplementation(idle);
});
afterEach(() => {
  const warned = warn.mock.calls;
  warn.mockRestore();
  expect(warned).toEqual([]);
});

const OPEN = {
  id: "open",
  model: "qwen3.6:27b",
  provider_id: "ollama",
  native_tools: true,
  vision: false,
  context_length: 32_000,
};

const INTERRUPTED = { unanswered: 1, attempts: 1, max_attempts: 3 };

describe("a turn Taurus stopped in", () => {
  it("is offered on reopening, with its running call marked unknown", async () => {
    invoke.mockImplementation((command: string) =>
      command === "resume_session"
        ? Promise.resolve({
            ...OPEN,
            switches: [],
            interrupted: INTERRUPTED,
            messages: [
              { role: "user", content: [{ type: "text", text: "write it" }] },
              {
                role: "assistant",
                content: [{ type: "tool_use", id: "w1", name: "write_file", input: {} }],
              },
            ],
          })
        : idle(command),
    );
    await useStore.getState().resume("open");

    const state = useStore.getState();
    expect(state.interrupted).toEqual(INTERRUPTED);
    const call = state.entries.find((e) => e.kind === "tool");
    expect(call).toMatchObject({ status: "error" });
    expect(call?.kind === "tool" && call.output).toContain("Outcome unknown");
  });

  it("continues with the flag set and a notice, not a message from you", async () => {
    useStore.setState({
      session: OPEN,
      entries: [],
      busy: false,
      interrupted: INTERRUPTED,
      sent: null,
      queued: null,
    });
    await useStore.getState().continueInterrupted();

    const sent = invoke.mock.calls.find(([name]) => name === "send_message");
    expect(sent?.[1]).toMatchObject({ text: CONTINUE_PROMPT, continueInterrupted: true });
    const state = useStore.getState();
    expect(state.interrupted).toBeNull();
    expect(state.entries.some((e) => e.kind === "user")).toBe(false);
    expect(state.entries[0]).toMatchObject({ kind: "notice" });
    expect(state.sent, "a continuation isn't a message to try again").toBeNull();
  });

  it("is ended by a typed message too", async () => {
    useStore.setState({ session: OPEN, entries: [], busy: false, interrupted: INTERRUPTED });
    await useStore.getState().send("never mind, do this instead");
    const sent = invoke.mock.calls.find(([name]) => name === "send_message");
    expect(sent?.[1]).toMatchObject({ continueInterrupted: false });
    expect(useStore.getState().interrupted).toBeNull();
  });
});
