// The delete action rather than the reducer, so Tauri has to be stood in for.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { useStore } from "./store";

const OPEN = {
  id: "open",
  model: "qwen3.6:27b",
  provider_id: "ollama",
  native_tools: true,
  context_length: 32_000,
};

/** Answers each command with something of the right shape. */
const backend = (overrides: Record<string, unknown> = {}) => {
  invoke.mockImplementation((command: string) => {
    if (command in overrides) {
      const answer = overrides[command];
      return answer instanceof Error
        ? Promise.reject(answer)
        : Promise.resolve(answer);
    }
    switch (command) {
      case "delete_session":
        return Promise.resolve(undefined);
      case "create_session":
        return Promise.resolve({ ...OPEN, id: "replacement" });
      case "list_sessions":
      case "list_checkpoints":
        return Promise.resolve([]);
      default:
        return Promise.resolve(undefined);
    }
  });
};

beforeEach(() => {
  invoke.mockReset();
  useStore.setState({
    session: OPEN,
    sessions: [],
    entries: [{ kind: "user", id: "e1", text: "hello" }],
    changed: ["a.txt"],
    error: null,
  });
});

const calls = (command: string) =>
  invoke.mock.calls.filter(([name]) => name === command);

describe("deleting a conversation", () => {
  it("asks the backend for the one that was named", async () => {
    backend();
    await useStore.getState().remove("older");
    expect(calls("delete_session")[0][1]).toEqual({ sessionId: "older" });
  });

  it("leaves the open conversation alone when a different one goes", async () => {
    backend();
    await useStore.getState().remove("older");
    expect(useStore.getState().session).toEqual(OPEN);
    expect(useStore.getState().entries).toHaveLength(1);
    expect(calls("create_session")).toHaveLength(0);
  });

  it("starts a replacement when the open conversation is the one deleted", async () => {
    // The alternative is a transcript on screen for a conversation that no
    // longer exists, and a composer that sends into it.
    backend();
    await useStore.getState().remove("open");
    expect(useStore.getState().session?.id).toBe("replacement");
    expect(useStore.getState().entries).toHaveLength(0);
    expect(useStore.getState().changed).toEqual([]);
    expect(calls("create_session")[0][1]).toEqual({
      providerId: "ollama",
      model: "qwen3.6:27b",
    });
  });

  it("reports a refusal instead of pretending the conversation went", async () => {
    // What the backend answers for a conversation that is mid-turn.
    backend({ delete_session: new Error("this conversation is mid-turn") });
    await useStore.getState().remove("open");
    expect(useStore.getState().error).toMatch(/mid-turn/);
    expect(useStore.getState().session).toEqual(OPEN);
    expect(calls("create_session")).toHaveLength(0);
  });

  it("still clears the dead conversation when the replacement cannot start", async () => {
    // A provider that has gone away since. Left holding the deleted session,
    // the composer would keep sending turns to an id Rust has dropped.
    backend({ create_session: new Error("ollama is not answering") });
    await useStore.getState().remove("open");
    expect(useStore.getState().session).toBeNull();
    expect(useStore.getState().error).toMatch(/not answering/);
  });
});
