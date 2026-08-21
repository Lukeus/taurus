// The states that exist only to say something is happening. Each of them is a
// window between a click and its result, and each was invisible before.
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
  vision: false,
  context_length: 32_000,
};

const META = (id: string, title: string) => ({
  id,
  workspace: "/w",
  model: "qwen3.6:27b",
  started: 0,
  updated: 0,
  title,
  turns: 1,
  changed: [],
});

beforeEach(() => {
  invoke.mockReset();
  useStore.setState({
    session: OPEN as never,
    sessions: [META("open", "Old name") as never],
    entries: [],
    busy: false,
    stopping: false,
    resuming: false,
    error: null,
  });
});

describe("stopping a turn", () => {
  it("says so while the cancel is in flight", async () => {
    // A cancel has to reach the loop, the running tool call has to notice, and
    // the stream has to drain. Through all of that the button read "Stop", so
    // the only feedback a second press gave was a second cancel.
    let release: () => void = () => {};
    invoke.mockImplementation(() => new Promise<void>((r) => (release = r)));

    const stopped = useStore.getState().stop();
    expect(useStore.getState().stopping).toBe(true);

    release();
    await stopped;
    // Still true: what clears it is the turn ending, in `send`'s `finally`,
    // which is the only thing that knows the cancel actually took.
    expect(useStore.getState().stopping).toBe(true);
  });

  it("becomes pressable again if the cancel itself failed", async () => {
    invoke.mockRejectedValue(new Error("no such session"));
    await useStore.getState().stop();

    expect(useStore.getState().stopping).toBe(false);
    expect(useStore.getState().error).toContain("no such session");
  });
});

describe("reopening a conversation", () => {
  it("marks the wait and clears it however the read ends", async () => {
    invoke.mockRejectedValue(new Error("gone"));
    await useStore.getState().resume("other").catch(() => {});
    expect(useStore.getState().resuming).toBe(false);
  });
});

describe("renaming a conversation", () => {
  it("shows the new name before the backend confirms it", async () => {
    // The confirmation arrives on `EVENT_SESSION`, a round trip later. Until
    // this, the committed name flicked back to the old one for a frame, which
    // reads as the rename having been refused.
    let release: () => void = () => {};
    invoke.mockImplementation(() => new Promise<void>((r) => (release = r)));

    const renamed = useStore.getState().rename("open", "New name");
    expect(useStore.getState().sessions[0].title).toBe("New name");

    release();
    await renamed;
  });

  it("puts the old name back when the rename does not land", async () => {
    invoke.mockRejectedValue(new Error("read-only"));
    await useStore.getState().rename("open", "New name");

    expect(useStore.getState().sessions[0].title).toBe("Old name");
    expect(useStore.getState().error).toContain("read-only");
  });

  it("leaves an empty title to the backend, which is the only thing that knows it", async () => {
    // Clearing a name restores the one derived from the conversation's first
    // question. Guessing at it here would be inventing a title, not echoing one.
    invoke.mockResolvedValue(undefined);
    await useStore.getState().rename("open", "   ");

    expect(useStore.getState().sessions[0].title).toBe("Old name");
  });
});
