// Permission prompts a page wasn't listening for, and prompts behind another.
// Tauri is stood in for, with a listener this file can fire.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
const listeners = new Map<string, (event: { payload: unknown }) => void>();
const order: string[] = [];
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(event, handler);
    order.push(`listen ${event}`);
    return Promise.resolve(() => {});
  },
}));

import { foldEvents, useStore } from "./store";

const PROMPT = "taurus://permission-request";
const prompt = (id: string, session?: string) => ({ id, session }) as never;
const raise = (id: string) => listeners.get(PROMPT)?.({ payload: prompt(id) });

let waiting: unknown[] = [];
beforeEach(() => {
  waiting = [];
  order.length = 0;
  invoke.mockReset();
  invoke.mockImplementation(async (command: string) => {
    order.push(command);
    if (command === "get_status") throw "no status in these tests";
    return command === "pending_permissions" ? waiting : [];
  });
  useStore.setState({ permission: null, session: null, status: null, error: null });
});

const settled = () => new Promise((resolve) => setTimeout(resolve));

describe("a prompt raised while the page was loading", () => {
  it("is asked for once the page is listening, and reaches the screen", async () => {
    waiting = [prompt("gap")];
    await useStore.getState().init();
    await settled();

    expect(useStore.getState().permission).toEqual(prompt("gap"));
    expect(order.indexOf(`listen ${PROMPT}`)).toBeLessThan(order.indexOf("pending_permissions"));
  });

  it("shows once when it arrives both ways", async () => {
    waiting = [prompt("both")];
    await useStore.getState().syncPermission();
    const shown = useStore.getState().permission;
    await useStore.getState().init();
    raise("both");
    await settled();

    expect(useStore.getState().permission).toBe(shown);
    invoke.mockClear();
    waiting = [];
    await useStore.getState().answerPermission("deny");
    expect(invoke.mock.calls.filter(([command]) => command === "respond_permission")).toHaveLength(1);
    expect(useStore.getState().permission).toBeNull();
  });
});

describe("two prompts at once", () => {
  it("keeps the one on screen and shows the next when it's answered", async () => {
    await useStore.getState().init();
    await settled();
    raise("first");
    raise("second");
    expect(useStore.getState().permission).toEqual(prompt("first"));

    waiting = [prompt("second")];
    await useStore.getState().answerPermission("allow_once");
    expect(useStore.getState().permission).toEqual(prompt("second"));
  });

  it("doesn't show again the prompt whose answer is still on its way", async () => {
    useStore.setState({ permission: prompt("mine") });
    waiting = [prompt("mine")];
    invoke.mockImplementation(async (command: string) => {
      if (command === "respond_permission") {
        await useStore.getState().syncPermission();
        waiting = [];
      }
      return command === "pending_permissions" ? waiting : [];
    });
    await useStore.getState().answerPermission("deny");
    expect(useStore.getState().permission).toBeNull();
  });
});

describe("a turn ending in a window that only attached to it", () => {
  it("takes down the dialog it was waiting on", async () => {
    await useStore.getState().init();
    await settled();
    useStore.setState({ permission: prompt("stopped", "s1") });
    waiting = [];
    listeners.get("taurus://turn")?.({ payload: { session: "s1" } });
    await settled();
    expect(useStore.getState().permission).toBeNull();
  });
});

describe("leaving a conversation to run unattended", () => {
  const responses = () => invoke.mock.calls.filter(([command]) => command === "respond_permission");

  it("refuses its own prompt", async () => {
    useStore.setState({ session: { id: "s1" } as never, permission: prompt("mine", "s1") });
    await useStore.getState().setUnattended(true);
    expect(responses()).toHaveLength(1);
  });

  it("leaves another conversation's prompt on screen", async () => {
    useStore.setState({ session: { id: "s1" } as never, permission: prompt("other", "s2") });
    await useStore.getState().setUnattended(true);
    expect(responses()).toHaveLength(0);
    expect(useStore.getState().permission).toEqual(prompt("other", "s2"));
  });
});

describe("a call that's both in the transcript and in the replay", () => {
  it("is one card, and a finished call with the same id is left alone", () => {
    const started = { type: "tool_call_started", id: "c1", name: "ask_user", preview: "live" } as never;
    const running = { kind: "tool", id: "c1", name: "ask_user", preview: "", status: "running", steps: [] } as never;
    const once = foldEvents([started], [running]);
    expect(once).toHaveLength(1);
    expect(once[0]).toMatchObject({ preview: "live" });

    const done = { kind: "tool", id: "c1", name: "ask_user", preview: "", status: "ok", steps: [] } as never;
    expect(foldEvents([started], [done])).toHaveLength(2);
  });
});

describe("a turn ending while the prompts are being read", () => {
  it("still puts up another conversation's prompt", async () => {
    // A's prompt is on screen; A's turn ends and clears it while the read is
    // out. B's prompt, waiting the whole time, must reach the screen.
    useStore.setState({ permission: prompt("a1", "A"), running: ["B"] });
    let answer: (value: unknown) => void = () => {};
    invoke.mockImplementation((command: string) =>
      command === "pending_permissions"
        ? new Promise((resolve) => {
            answer = resolve;
          })
        : Promise.resolve([]),
    );
    const syncing = useStore.getState().syncPermission();
    useStore.setState({ permission: null });
    // Read just before A withdrew its own.
    answer([prompt("a1", "A"), prompt("b1", "B")]);
    await syncing;
    expect(useStore.getState().permission).toEqual(prompt("b1", "B"));
  });
});
