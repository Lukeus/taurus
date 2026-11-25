// Comments left on a turn's diff go with the next message that actually starts
// a turn — composed into its text by the backend, so the message on screen is
// the one the transcript will replay — and with nothing else: not a held
// message, not a retry, not another conversation's send.
import { afterEach, beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {
    onmessage: unknown;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import type { Remark } from "../lib/api";
import { useStore } from "./store";

/** The commands that answer with a list, which `idle` answers with an empty one. */
const LISTS = new Set(["list_sessions", "list_checkpoints", "running_sessions"]);

/**
 * What every command a test is not about answers with: the empty version of
 * itself.
 *
 * `undefined` for a listing is a crash inside the store, which catches it and
 * logs a TypeError — from a test that then passes, and a green run that prints
 * errors teaches everyone to skim past the one that matters.
 */
function idle(command: string): Promise<unknown> {
  // Attaching answers with a shape rather than a list: a conversation with no
  // turn in it, which is what every conversation in these tests is.
  if (command === "attach_session") return Promise.resolve({ turn: null, dropped: 0 });
  return Promise.resolve(LISTS.has(command) ? [] : undefined);
}

// The store logs what it could not do and carries on, so a warning here is a
// failure the assertions cannot see: a stub answering the wrong shape, or a
// real one. Either way the test says so rather than passing.
let warn: MockInstance<typeof console.warn>;
beforeEach(() => {
  warn = vi.spyOn(console, "warn").mockImplementation(() => {});
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

/** Every `send_message` this test made, in order. */
const sends = () => invoke.mock.calls.filter(([name]) => name === "send_message");

beforeEach(() => {
  invoke.mockReset();
  useStore.setState({
    session: OPEN,
    entries: [],
    busy: false,
    stopping: false,
    queued: null,
    remarks: {},
    sent: null,
    error: null,
  });
});

const ON_LINE: Remark = {
  anchor: {
    kind: "diff",
    turn: 3,
    path: "src/widget.rs",
    side: "added",
    line: 42,
    quoted: "pub fn new(name: &str) -> Self {",
  },
  text: "keep the old constructor too",
};

/** What the backend's `compose_message` would say, near enough to tell apart. */
const COMPOSED = "looks close\n\nComments:\n\n- `src/widget.rs` line 42";

const sentText = () => sends().map(([, args]) => (args as { text: string }).text);

describe("comments waiting on a conversation", () => {
  it("go with the next message, composed into it", async () => {
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "compose_message") {
        expect(args).toEqual({ text: "looks close", remarks: [ON_LINE] });
        return Promise.resolve(COMPOSED);
      }
      return idle(command);
    });
    useStore.getState().addRemark("open", ON_LINE);

    await useStore.getState().send("looks close");

    expect(sentText()).toEqual([COMPOSED]);
    // The bubble on screen is the text the transcript keeps, so a reopen
    // replays the same message rather than a longer one.
    expect(useStore.getState().entries[0]).toMatchObject({ kind: "user", text: COMPOSED });
    expect(useStore.getState().remarks.open).toEqual([]);
  });

  it("are a message on their own, with nothing typed", async () => {
    invoke.mockImplementation((command: string) =>
      command === "compose_message" ? Promise.resolve("Please address these comments:") : idle(command),
    );
    useStore.getState().addRemark("open", ON_LINE);

    await useStore.getState().send("");

    expect(sentText()).toEqual(["Please address these comments:"]);
  });

  it("stay put while a turn runs, and go with the held message", async () => {
    let finish!: () => void;
    const first = new Promise<void>((resolve) => (finish = resolve));
    let calls = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "send_message") return calls++ === 0 ? first : Promise.resolve();
      if (command === "compose_message") return Promise.resolve("and then profile it + comments");
      return idle(command);
    });

    const running = useStore.getState().send("time the build");
    useStore.getState().addRemark("open", ON_LINE);
    // Nothing typed: there is no message to hold, and the comments wait.
    await useStore.getState().send("");
    expect(useStore.getState().queued).toBeNull();
    await useStore.getState().send("and then profile it");
    expect(useStore.getState().queued?.text).toBe("and then profile it");
    expect(useStore.getState().remarks.open).toHaveLength(1);

    finish();
    await running;
    // The drain's send composes first, which is a round trip of its own.
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(sentText()).toEqual(["time the build", "and then profile it + comments"]);
    expect(useStore.getState().remarks.open).toEqual([]);
  });

  it("are not added again to a retry, which already carries them", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "compose_message") return Promise.resolve(COMPOSED);
      if (command === "send_message" && sends().length === 1) return Promise.reject("overloaded");
      return idle(command);
    });
    useStore.getState().addRemark("open", ON_LINE);
    await useStore.getState().send("looks close");
    // A new comment written after the failure is for the next message, not
    // smuggled into a resend of the last one.
    useStore.getState().addRemark("open", { ...ON_LINE, text: "and rename it" });

    await useStore.getState().retry();

    expect(sentText()).toEqual([COMPOSED, COMPOSED]);
    expect(invoke.mock.calls.filter(([name]) => name === "compose_message")).toHaveLength(1);
    expect(useStore.getState().remarks.open).toHaveLength(1);
  });

  it("belong to their own conversation", async () => {
    invoke.mockImplementation((command: string) => idle(command));
    useStore.getState().addRemark("elsewhere", ON_LINE);

    await useStore.getState().send("unrelated");

    expect(invoke.mock.calls.some(([name]) => name === "compose_message")).toBe(false);
    expect(sentText()).toEqual(["unrelated"]);
    expect(useStore.getState().remarks.elsewhere).toHaveLength(1);
  });

  it("are kept when composing them is refused", async () => {
    invoke.mockImplementation((command: string) =>
      command === "compose_message" ? Promise.reject("51 comments is more than one message can carry") : idle(command),
    );
    useStore.getState().addRemark("open", ON_LINE);

    await useStore.getState().send("go");

    expect(sends()).toHaveLength(0);
    expect(useStore.getState().error).toContain("more than one message");
    expect(useStore.getState().remarks.open).toHaveLength(1);
  });

  it("on a turn a rewind undid are dropped, and the rest kept", () => {
    useStore.getState().addRemark("open", ON_LINE); // turn 3
    useStore.getState().addRemark("open", { ...ON_LINE, anchor: { ...ON_LINE.anchor, turn: 2 } as Remark["anchor"] });
    useStore.getState().addRemark("open", { anchor: { kind: "step", text: "Add the flag" }, text: "plan" });

    useStore.getState().dropRemarksFrom("open", 3);

    expect(useStore.getState().remarks.open.map((r) => r.text)).toEqual([
      "keep the old constructor too",
      "plan",
    ]);
    expect((useStore.getState().remarks.open[0].anchor as { turn: number }).turn).toBe(2);
  });

  it("are taken back one at a time", () => {
    useStore.getState().addRemark("open", ON_LINE);
    useStore.getState().addRemark("open", { ...ON_LINE, text: "second" });
    useStore.getState().removeRemark("open", 0);
    expect(useStore.getState().remarks.open.map((r) => r.text)).toEqual(["second"]);
  });
});
