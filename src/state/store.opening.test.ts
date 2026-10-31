// Two clicks in the rail are two round trips, and the one that answers last
// isn't always the one asked last. And a click has nothing to catch a failure
// with, so that has to reach the banner by another route.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { useStore } from "./store";

const created = (id: string) => ({
  id,
  model: "m",
  provider_id: "ollama",
  native_tools: true,
  vision: false,
  context_length: 32_000,
});
const opened = (id: string) => ({ ...created(id), messages: [], switches: [] });

/** Resume answers wait to be let go, by conversation. */
let land: Record<string, (fail?: unknown) => void>;
const closed = () =>
  invoke.mock.calls
    .filter(([command]) => command === "close_session")
    .map(([, args]) => (args as { sessionId: string }).sessionId);

beforeEach(() => {
  land = {};
  invoke.mockReset();
  invoke.mockImplementation((command: string, raw: unknown) => {
    if (command === "resume_session") {
      const id = (raw as { sessionId: string }).sessionId;
      return new Promise((resolve, reject) => {
        land[id] = (fail) => (fail ? reject(fail) : resolve(opened(id)));
      });
    }
    if (command === "attach_session") {
      return Promise.resolve({ turn: null, unattended: false, dropped: 0 });
    }
    return Promise.resolve([]);
  });
  useStore.setState({ session: created("x"), entries: [], error: null, resuming: false });
});

describe("opening conversations in quick succession", () => {
  it("ends on the one asked for last, whichever answers last", async () => {
    const first = useStore.getState().resume("a");
    const second = useStore.getState().resume("b");

    land.b();
    await second;
    expect(useStore.getState().session?.id).toBe("b");

    land.a();
    await first;
    expect(useStore.getState().session?.id).toBe("b");
    // What the overtaken click opened doesn't stay open behind the window.
    expect(closed()).toContain("a");
    expect(closed()).not.toContain("b");
  });

  it("keeps the wait up until the last one has answered", async () => {
    const first = useStore.getState().resume("a");
    const second = useStore.getState().resume("b");

    land.a();
    await first;
    expect(useStore.getState().resuming).toBe(true);
    expect(useStore.getState().session?.id).toBe("x");

    land.b();
    await second;
    expect(useStore.getState().resuming).toBe(false);
    expect(useStore.getState().session?.id).toBe("b");
    expect(closed()).toContain("x");
  });

  it("doesn't close a conversation clicked again after being overtaken", async () => {
    const first = useStore.getState().resume("a");
    const firstLand = land.a;
    const second = useStore.getState().resume("b");
    const third = useStore.getState().resume("a");
    land.b();
    await second;
    firstLand();
    await first;
    expect(closed()).not.toContain("a");
    land.a();
    await third;
    expect(useStore.getState().session?.id).toBe("a");
    expect(closed()).not.toContain("a");
  });

  it("drops an open overtaken by starting a new conversation", async () => {
    const pending = useStore.getState().resume("a");
    invoke.mockImplementationOnce(() => Promise.resolve(created("new")));
    await useStore.getState().startSession("ollama", "m");
    land.a();
    await pending;
    expect(useStore.getState().session?.id).toBe("new");
    expect(useStore.getState().resuming).toBe(false);
  });

  it("says nothing about a failure nobody is waiting on", async () => {
    const first = useStore.getState().openConversation("a");
    const second = useStore.getState().resume("b");
    land.a("unreadable");
    await first;
    expect(useStore.getState().error).toBeNull();
    land.b();
    await second;
    expect(useStore.getState().session?.id).toBe("b");
  });
});

describe("a conversation that can't be opened", () => {
  it("says why in the banner when clicked, and leaves the one on screen", async () => {
    const click = useStore.getState().openConversation("a");
    land.a("the transcript is unreadable");
    await click;

    expect(useStore.getState().error).toContain("the transcript is unreadable");
    expect(useStore.getState().session?.id).toBe("x");
    expect(useStore.getState().resuming).toBe(false);
  });

  it("still rejects from `resume`, which opening a workspace falls through on", async () => {
    const pending = useStore.getState().resume("a");
    land.a("gone");
    await expect(pending).rejects.toBe("gone");
  });
});
