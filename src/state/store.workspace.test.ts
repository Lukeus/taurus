// Switching folders, which is a transition rather than a setting: everything
// on screen belongs to the workspace being left. Actions rather than the
// reducer, so Tauri has to be stood in for.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { useStore } from "./store";

const OPEN = {
  id: "in-project-a",
  model: "qwen3.6:27b",
  provider_id: "ollama",
  native_tools: true,
  vision: false,
  context_length: 32_000,
};

const STATUS = {
  workspace: "/src/project-b",
  providers: [{ id: "ollama", default_model: "qwen3.6:27b" }],
  settings: { last_provider: "ollama", last_model: "qwen3.6:27b" },
};

/** One saved conversation, as `list_sessions` reports it. */
const SAVED = {
  id: "in-project-b",
  workspace: "/src/project-b",
  model: "qwen3.6:27b",
  updated: 1_700_000_000,
  title: "the other folder's work",
  branch: null,
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
      case "get_status":
        return Promise.resolve(STATUS);
      case "set_workspace":
        return Promise.resolve(STATUS.workspace);
      case "list_sessions":
        return Promise.resolve([SAVED]);
      case "resume_session":
        return Promise.resolve({
          ...OPEN,
          id: SAVED.id,
          messages: [{ role: "user", content: [{ type: "text", text: "hi" }] }],
        });
      case "create_session":
        return Promise.resolve({ ...OPEN, id: "fresh" });
      case "list_checkpoints":
        return Promise.resolve([]);
      case "list_models":
        return Promise.resolve([{ id: "qwen3.6:27b", display_name: "Qwen" }]);
      default:
        return Promise.resolve(undefined);
    }
  });
};

beforeEach(() => {
  invoke.mockReset();
  useStore.setState({
    status: STATUS as never,
    session: OPEN,
    sessions: [],
    entries: [{ kind: "user", id: "e1", text: "hello" }],
    changed: ["a.txt"],
    busy: false,
    permission: { id: "p1" } as never,
    proposals: [{ id: "s1" }] as never,
    agentProposals: [{ id: "a1" }] as never,
    error: null,
  });
});

const calls = (command: string) =>
  invoke.mock.calls.filter(([name]) => name === command);

describe("switching workspace", () => {
  it("closes the conversation belonging to the folder being left", async () => {
    // Its transcript is written under the old workspace's directory and its
    // checkpoints are keyed by it. A turn sent after the switch would edit one
    // folder and record into another.
    backend();
    await useStore.getState().setWorkspace("/src/project-b");
    expect(calls("close_session")[0][1]).toEqual({ sessionId: "in-project-a" });
  });

  it("clears everything that described the old folder", async () => {
    backend({ list_sessions: [] });
    await useStore.getState().setWorkspace("/src/project-b");
    const state = useStore.getState();
    expect(state.entries).toHaveLength(0);
    expect(state.changed).toEqual([]);
    expect(state.permission).toBeNull();
    expect(state.proposals).toEqual([]);
    expect(state.agentProposals).toEqual([]);
  });

  it("opens the new folder's most recent conversation", async () => {
    // The backend has always been able to answer this; nothing used to ask.
    backend();
    await useStore.getState().setWorkspace("/src/project-b");
    expect(calls("resume_session")[0][1]).toMatchObject({
      sessionId: "in-project-b",
    });
    expect(useStore.getState().session?.id).toBe("in-project-b");
    expect(useStore.getState().entries).toHaveLength(1);
  });

  it("starts a fresh conversation in a folder that has none", async () => {
    backend({ list_sessions: [] });
    await useStore.getState().setWorkspace("/src/project-b");
    expect(useStore.getState().session?.id).toBe("fresh");
    expect(calls("resume_session")).toHaveLength(0);
  });

  it("switches after the old conversation is closed, never before", async () => {
    // The other order leaves a window in which the backend is in the new
    // folder while a session from the old one is still live and sendable.
    backend();
    await useStore.getState().setWorkspace("/src/project-b");
    const order = invoke.mock.calls.map(([name]) => name);
    expect(order.indexOf("close_session")).toBeLessThan(
      order.indexOf("set_workspace"),
    );
  });

  it("refuses mid-turn rather than pulling the folder out from under it", async () => {
    backend();
    useStore.setState({ busy: true });
    await useStore.getState().setWorkspace("/src/project-b");
    expect(calls("set_workspace")).toHaveLength(0);
    expect(useStore.getState().session).toEqual(OPEN);
    expect(useStore.getState().error).toMatch(/middle of a turn/);
  });

  it("does not strand the app when the new folder's conversation will not open", async () => {
    // A transcript written by a newer build, or a model that is gone. The
    // folder still has to end up usable.
    backend({ resume_session: new Error("no saved session") });
    await useStore.getState().setWorkspace("/src/project-b");
    expect(useStore.getState().session?.id).toBe("fresh");
  });
});

describe("letting go of a conversation", () => {
  it("closes the outgoing one when another is opened", async () => {
    backend();
    await useStore.getState().resume("in-project-b");
    expect(calls("close_session")[0][1]).toEqual({ sessionId: "in-project-a" });
  });

  it("closes the outgoing one when a new conversation is started", async () => {
    backend();
    await useStore.getState().startSession("ollama", "qwen3.6:27b");
    expect(calls("close_session")[0][1]).toEqual({ sessionId: "in-project-a" });
  });

  it("keeps the conversation when the replacement never arrives", async () => {
    // Closing it in exchange for nothing would leave the composer sending into
    // an id Rust has dropped.
    backend({ create_session: new Error("ollama is not answering") });
    await expect(
      useStore.getState().startSession("ollama", "qwen3.6:27b"),
    ).rejects.toThrow();
    expect(calls("close_session")).toHaveLength(0);
    expect(useStore.getState().session).toEqual(OPEN);
  });

  it("does not close the conversation it was asked to reopen", async () => {
    backend({ resume_session: { ...OPEN, messages: [] } });
    await useStore.getState().resume("in-project-a");
    expect(calls("close_session")).toHaveLength(0);
  });

  it("survives a close the backend refuses", async () => {
    backend({ close_session: new Error("gone already") });
    await useStore.getState().resume("in-project-b");
    expect(useStore.getState().session?.id).toBe("in-project-b");
    expect(useStore.getState().error).toBeNull();
  });
});
