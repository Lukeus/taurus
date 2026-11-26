// Planning: the next turn reads and writes a plan, and nothing changes until
// the person approves it. The backend holds the mode and builds the turn; the
// store's copy is what the composer and the plan panel read, so it must never
// claim more than the backend has.
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
import { pinnedPlan, useStore } from "./store";

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
    planning: {},
    sent: null,
    error: null,
  });
});

const sentText = () => sends().map(([, args]) => (args as { text: string }).text);

const ON_STEP: Remark = {
  anchor: { kind: "step", text: "Split the parser in two" },
  text: "keep the old module as a re-export",
};

const APPROVED = "The plan is approved. Carry it out.";

describe("planning a conversation", () => {
  it("is said to be planning only once the backend has taken it", async () => {
    let answer!: () => void;
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "set_planning") {
        expect(args).toEqual({ sessionId: "open", planning: true });
        return new Promise<void>((resolve) => (answer = resolve));
      }
      return idle(command);
    });

    const setting = useStore.getState().setPlanning("open", true);
    expect(useStore.getState().planning.open).toBeFalsy();
    answer();
    await setting;
    expect(useStore.getState().planning.open).toBe(true);
  });

  it("stays as it was when the backend refuses", async () => {
    invoke.mockImplementation((command: string) =>
      command === "set_planning" ? Promise.reject("no such conversation") : idle(command),
    );
    await useStore.getState().setPlanning("open", true);
    expect(useStore.getState().planning.open).toBeFalsy();
    expect(useStore.getState().error).toBe("no such conversation");
  });

  it("is approved in the backend's words, by the turn that carries it out", async () => {
    useStore.setState({ planning: { open: true } });
    let planning = true;
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "plan_approval") return Promise.resolve(APPROVED);
      if (command === "compose_message") {
        expect(args).toEqual({ text: APPROVED, remarks: [ON_STEP] });
        return Promise.resolve(`${APPROVED}\n\nComments:\n\n- Plan step`);
      }
      if (command === "send_message") {
        // The backend ends planning as it builds the turn, and only then.
        expect((args as { approvePlan: boolean }).approvePlan).toBe(true);
        planning = false;
        return Promise.resolve();
      }
      if (command === "is_planning") return Promise.resolve(planning);
      return idle(command);
    });
    useStore.getState().addRemark("open", ON_STEP);

    await useStore.getState().approvePlan();

    expect(sentText()).toEqual([`${APPROVED}\n\nComments:\n\n- Plan step`]);
    expect(useStore.getState().planning.open).toBe(false);
    expect(useStore.getState().remarks.open).toEqual([]);
  });

  it("stays planning when the approval never starts a turn", async () => {
    // Composing the step comments refused: nothing is sent, so nothing was
    // approved, and the next message must not be handed every tool.
    useStore.setState({ planning: { open: true } });
    invoke.mockImplementation((command: string) => {
      if (command === "plan_approval") return Promise.resolve(APPROVED);
      if (command === "compose_message") return Promise.reject("51 comments is more than one message can carry");
      return idle(command);
    });
    useStore.getState().addRemark("open", ON_STEP);

    await useStore.getState().approvePlan();

    expect(sends()).toEqual([]);
    expect(useStore.getState().planning.open).toBe(true);
    expect(invoke.mock.calls.map(([name]) => name)).not.toContain("set_planning");
    // Nobody typed the approval, so it isn't put in the box.
    expect(useStore.getState().returned).toBeNull();
    expect(useStore.getState().remarks.open).toEqual([ON_STEP]);
  });

  it("can't be approved while the plan is still being written", async () => {
    useStore.setState({ planning: { open: true }, busy: true });
    invoke.mockImplementation(idle);
    await useStore.getState().approvePlan();
    expect(sends()).toEqual([]);
    expect(useStore.getState().planning.open).toBe(true);
  });

  it("starts from no plan: one from before isn't pinned for approval", async () => {
    invoke.mockImplementation(idle);
    useStore.setState({
      entries: [
        {
          kind: "tool",
          id: "old",
          name: "update_plan",
          preview: "update_plan",
          status: "ok",
          steps: [],
          view: { type: "plan", steps: [{ text: "Rename the widget", state: "todo" }] },
        },
      ] as never,
    });
    expect(pinnedPlan(useStore.getState().entries)).not.toBeNull();

    await useStore.getState().setPlanning("open", true);

    expect(pinnedPlan(useStore.getState().entries)).toBeNull();
    expect(useStore.getState().entries.at(-1)).toMatchObject({ kind: "notice", planning: true });
  });
});
