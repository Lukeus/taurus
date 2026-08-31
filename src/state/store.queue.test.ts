// The two things that used to happen to a message the moment a turn was
// already running: nothing at all, silently. Typing ahead while the model works
// is most of how this gets used, and Enter dropped the sentence on the floor.
//
// Actions rather than the reducer, because the whole behaviour lives in what
// `send` does at its two ends — the guard at the top and the drain in the
// `finally`. Tauri has to be stood in for.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {
    onmessage: unknown;
  },
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

/** Every `send_message` this test made, in order. */
const sends = () => invoke.mock.calls.filter(([name]) => name === "send_message");

/**
 * A turn that does not finish until it is told to.
 *
 * The whole subject here is what happens *during* one, so a stub that resolves
 * immediately would leave nothing to type ahead of.
 */
function turn() {
  let finish!: () => void;
  let fail!: (e: unknown) => void;
  const running = new Promise<void>((resolve, reject) => {
    finish = () => resolve();
    fail = reject;
  });
  return { running, finish, fail };
}

beforeEach(() => {
  invoke.mockReset();
  useStore.setState({
    session: OPEN,
    entries: [],
    busy: false,
    stopping: false,
    queued: null,
    sent: null,
    error: null,
  });
});

describe("a message typed while a turn is running", () => {
  it("is held rather than dropped", async () => {
    const first = turn();
    invoke.mockImplementation((command: string) =>
      command === "send_message" ? first.running : Promise.resolve(undefined),
    );

    const running = useStore.getState().send("time the build");
    expect(useStore.getState().busy).toBe(true);

    await useStore.getState().send("and then profile it");
    // Not a second turn. One is running, and the harness takes one at a time.
    expect(sends()).toHaveLength(1);
    expect(useStore.getState().queued?.text).toBe("and then profile it");

    first.finish();
    await running;
  });

  it("goes as its own turn once the one in front of it finishes", async () => {
    const first = turn();
    invoke.mockImplementation((command: string) =>
      command === "send_message" ? first.running : Promise.resolve(undefined),
    );

    const running = useStore.getState().send("time the build");
    await useStore.getState().send("and then profile it");

    first.finish();
    await running;
    // The drain runs inside the first turn's `finally`, and the second turn's
    // own send is not awaited there — one tick is what it takes to land.
    await Promise.resolve();

    expect(sends().map(([, args]) => (args as { text: string }).text)).toEqual([
      "time the build",
      "and then profile it",
    ]);
    expect(useStore.getState().queued).toBeNull();
  });

  it("stays put when the turn in front of it was stopped", async () => {
    // Pressing Stop and then watching the thing you typed while it worked fire
    // anyway is the opposite of what the button said.
    const first = turn();
    invoke.mockImplementation((command: string) =>
      command === "send_message" ? first.running : Promise.resolve(undefined),
    );

    const running = useStore.getState().send("time the build");
    await useStore.getState().send("and then profile it");
    await useStore.getState().stop();

    first.finish();
    await running;
    await Promise.resolve();

    expect(sends()).toHaveLength(1);
    expect(useStore.getState().queued?.text).toBe("and then profile it");
  });

  it("stays put when the turn in front of it died", async () => {
    // Whatever broke is still broken a millisecond later, and an automatic
    // resend would spend the rate limit rather than report it.
    const first = turn();
    invoke.mockImplementation((command: string) =>
      command === "send_message" ? first.running : Promise.resolve(undefined),
    );

    const running = useStore.getState().send("time the build");
    await useStore.getState().send("and then profile it");

    first.fail(new Error("ollama is not answering"));
    await running;
    await Promise.resolve();

    expect(sends()).toHaveLength(1);
    expect(useStore.getState().queued?.text).toBe("and then profile it");
  });

  it("keeps only the newest one", async () => {
    // A queue of three is a conversation nobody is having, and the box offers
    // no way to see or reorder one.
    const first = turn();
    invoke.mockImplementation((command: string) =>
      command === "send_message" ? first.running : Promise.resolve(undefined),
    );

    const running = useStore.getState().send("time the build");
    await useStore.getState().send("no, profile it");
    await useStore.getState().send("actually, just the tests");
    expect(useStore.getState().queued?.text).toBe("actually, just the tests");

    first.finish();
    await running;
    await Promise.resolve();
    expect(sends()).toHaveLength(2);
  });

  it("can be thrown away without being sent", () => {
    useStore.setState({
      busy: true,
      queued: { text: "never mind", images: [], onScreen: null },
    });
    useStore.getState().unqueue();
    expect(useStore.getState().queued).toBeNull();
  });
});

describe("trying a failed turn again", () => {
  it("sends the same message, with whatever travelled with it", async () => {
    invoke.mockImplementation((command: string) =>
      command === "send_message"
        ? Promise.reject(new Error("ollama is not answering"))
        : Promise.resolve(undefined),
    );
    const screen = { document: { path: "docs/retries.md", unsaved: false } };

    await useStore.getState().send("what does this say?", [], screen);
    expect(useStore.getState().busy).toBe(false);

    await useStore.getState().retry();

    const [, second] = sends();
    expect((second[1] as { text: string }).text).toBe("what does this say?");
    // Not only the text: a retry that dropped the pane the question was asked
    // from would be re-asking a different question.
    expect((second[1] as { onScreen: unknown }).onScreen).toEqual(screen);
  });

  it("marks the failure as one worth offering a way back from", async () => {
    invoke.mockImplementation((command: string) =>
      command === "send_message"
        ? Promise.reject(new Error("ollama is not answering"))
        : Promise.resolve(undefined),
    );

    await useStore.getState().send("time the build");
    const last = useStore.getState().entries.at(-1);
    expect(last).toMatchObject({ kind: "notice", tone: "error", failed: true });
  });

  it("does nothing under a running turn", async () => {
    // `send` would otherwise queue it, and a retry that fires a minute later
    // is not one anybody asked for.
    const first = turn();
    invoke.mockImplementation((command: string) =>
      command === "send_message" ? first.running : Promise.resolve(undefined),
    );

    const running = useStore.getState().send("time the build");
    await useStore.getState().retry();
    expect(sends()).toHaveLength(1);
    expect(useStore.getState().queued).toBeNull();

    first.finish();
    await running;
  });
});

describe("changing conversation", () => {
  it("leaves neither the queue nor the way back behind", async () => {
    // A message typed ahead in one conversation must not fire in another, and
    // a retry must not resend one conversation's question into the next.
    invoke.mockImplementation((command: string) =>
      command === "resume_session"
        ? Promise.resolve({ ...OPEN, id: "other", messages: [], switches: [] })
        : Promise.resolve(undefined),
    );
    useStore.setState({
      queued: { text: "and then profile it", images: [], onScreen: null },
      sent: { text: "time the build", images: [], onScreen: null },
    });

    await useStore.getState().resume("other");

    expect(useStore.getState().queued).toBeNull();
    expect(useStore.getState().sent).toBeNull();
  });
});
