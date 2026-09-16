// A turn that outlives the view of it: what the window does when it opens a
// conversation that is already working, and what it does not do to the one it
// left behind. Actions rather than the reducer, so Tauri is stood in for.
import { afterEach, beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {
    onmessage: unknown;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { useStore } from "./store";

const session = (id: string) => ({
  id,
  model: "qwen3.6:27b",
  provider_id: "ollama",
  native_tools: true,
  vision: false,
  context_length: 32_000,
});

const OPEN = session("open");

/** Unix seconds, as the backend reports a turn's start. */
const STARTED = 1_700_000_000;

/** What every command a test is not about answers with. */
const idle = (command: string): Promise<unknown> => {
  if (command === "attach_session") return Promise.resolve({ turn: null, dropped: 0 });
  if (command === "resume_session") {
    return Promise.resolve({ ...session("other"), messages: [], switches: [] });
  }
  return Promise.resolve(["list_sessions", "list_checkpoints"].includes(command) ? [] : undefined);
};

// A warning is a failure the assertions cannot see — a stub answering the
// wrong shape, or a real one. Either way the test says so rather than passing.
let warn: MockInstance<typeof console.warn>;
beforeEach(() => {
  warn = vi.spyOn(console, "warn").mockImplementation(() => {});
});
afterEach(() => {
  const warned = warn.mock.calls;
  warn.mockRestore();
  expect(warned).toEqual([]);
});

beforeEach(() => {
  invoke.mockReset();
  useStore.setState({
    session: OPEN,
    sessions: [],
    entries: [],
    busy: false,
    turn: null,
    running: [],
    stopping: false,
    queued: null,
    sent: null,
    error: null,
  });
});

describe("opening a conversation that is already working", () => {
  it("comes back working, with the time the turn started", async () => {
    // The window cannot know this. It did not start the turn — another window
    // did, or this one did before it was reloaded — and the transcript it just
    // read says nothing about whether anything is still writing to it.
    invoke.mockImplementation((command: string) =>
      command === "attach_session"
        ? Promise.resolve({ turn: { started_at: STARTED, iteration: 4 }, dropped: 0 })
        : idle(command),
    );

    await useStore.getState().attach(OPEN.id);

    expect(useStore.getState().busy).toBe(true);
    expect(useStore.getState().turn).toEqual({ started_at: STARTED, iteration: 4 });
  });

  it("leaves an idle one idle, and says nothing about it", async () => {
    invoke.mockImplementation(idle);

    await useStore.getState().attach(OPEN.id);

    expect(useStore.getState().busy).toBe(false);
    expect(useStore.getState().turn).toBeNull();
    expect(useStore.getState().entries).toEqual([]);
  });

  it("says where the round on screen begins, when the start of it is gone", async () => {
    // The replay holds the round in progress, capped. A window arriving after
    // a long round overran it would otherwise begin mid-sentence with nothing
    // saying so.
    invoke.mockImplementation((command: string) =>
      command === "attach_session"
        ? Promise.resolve({ turn: { started_at: STARTED, iteration: 9 }, dropped: 12 })
        : idle(command),
    );

    await useStore.getState().attach(OPEN.id);

    const notice = useStore.getState().entries.at(-1);
    expect(notice?.kind).toBe("notice");
    expect(notice).toMatchObject({ tone: "info" });
  });

  it("does not answer for a conversation the window has left again", async () => {
    let answer!: (value: unknown) => void;
    invoke.mockImplementation((command: string) =>
      command === "attach_session"
        ? new Promise((resolve) => {
            answer = resolve;
          })
        : idle(command),
    );

    const attaching = useStore.getState().attach(OPEN.id);
    // Opened, then left again before the answer came back.
    useStore.setState({ session: session("elsewhere") });
    answer({ turn: { started_at: STARTED, iteration: 1 }, dropped: 0 });
    await attaching;

    expect(useStore.getState().busy).toBe(false);
  });
});

describe("the push that says a conversation is working", () => {
  /**
   * Reaching for the store's own listener would mean running `init`, which
   * builds the whole app. What the listener does with the event is the part
   * worth pinning, and it is the same shape either way.
   */
  const pushed = (id: string, turn: { started_at: number; iteration: number } | null) =>
    useStore.setState((s) => {
      const running = turn
        ? s.running.includes(id)
          ? s.running
          : [...s.running, id]
        : s.running.filter((each) => each !== id);
      if (s.session?.id !== id) return { running };
      return turn
        ? { running, busy: true, turn }
        : { running, busy: false, turn: null };
    });

  it("marks a conversation that is not on screen without touching the composer", () => {
    pushed("elsewhere", { started_at: STARTED, iteration: 1 });

    expect(useStore.getState().running).toEqual(["elsewhere"]);
    expect(useStore.getState().busy).toBe(false);
  });

  it("ends a turn on screen that this window never started", () => {
    useStore.setState({ busy: true, turn: { started_at: STARTED, iteration: 3 } });

    pushed(OPEN.id, null);

    expect(useStore.getState().busy).toBe(false);
    expect(useStore.getState().turn).toBeNull();
  });
});

describe("a turn left running in another conversation", () => {
  it("does not put the composer back when it ends over one that is still working", async () => {
    // "The turn ended" and "the turn on screen ended" used to be the same
    // sentence. They are not any more: the first can happen while somebody is
    // watching a different conversation work, and answering it there would
    // offer Send into a turn that is still running.
    let finish!: () => void;
    const running = new Promise<void>((resolve) => {
      finish = () => resolve();
    });
    invoke.mockImplementation((command: string) =>
      command === "send_message"
        ? running
        : command === "attach_session"
          ? Promise.resolve({ turn: { started_at: STARTED, iteration: 2 }, dropped: 0 })
          : idle(command),
    );

    const turn = useStore.getState().send("time the build");
    expect(useStore.getState().busy).toBe(true);

    // The window moves on to a conversation with a turn of its own.
    useStore.setState({ session: session("elsewhere"), queued: null });
    await useStore.getState().attach("elsewhere");
    expect(useStore.getState().busy).toBe(true);

    finish();
    await turn;

    expect(useStore.getState().busy).toBe(true);
    expect(useStore.getState().turn).toEqual({ started_at: STARTED, iteration: 2 });
  });

  it("does not report how it ended over the conversation on screen", async () => {
    let fail!: (e: unknown) => void;
    const running = new Promise<void>((_, reject) => {
      fail = reject;
    });
    invoke.mockImplementation((command: string) =>
      command === "send_message" ? running : idle(command),
    );

    const turn = useStore.getState().send("time the build");
    useStore.setState({ session: session("elsewhere"), entries: [], busy: false });
    fail("the backend went away");
    await turn;

    // The transcript it belongs to keeps the error, and reopening reads it.
    expect(useStore.getState().entries).toEqual([]);
    expect(useStore.getState().busy).toBe(false);
  });
});
