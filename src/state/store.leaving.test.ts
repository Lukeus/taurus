// Setting a conversation to unattended, and the offer that comes with it:
// what was allowed once is listed, granting is a separate press, and the
// switch itself never writes anything.
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

const OPEN = {
  id: "open",
  model: "qwen3.6:27b",
  provider_id: "ollama",
  native_tools: true,
  vision: false,
  context_length: 32_000,
};

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
    unattended: false,
    permission: null,
    leaving: null,
    error: null,
  });
});

const calls = () => invoke.mock.calls.map((call) => call[0] as string);

describe("setting a conversation to unattended", () => {
  it("offers what was allowed once, and only once there is something to offer", async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === "once_allowed_rules" ? ["run_command:cargo"] : undefined),
    );

    await useStore.getState().setUnattended(true);

    expect(useStore.getState().unattended).toBe(true);
    expect(useStore.getState().leaving).toEqual(["run_command:cargo"]);
    expect(calls()).toEqual(["set_unattended", "once_allowed_rules"]);
    expect(invoke.mock.calls[1][1]).toEqual({ sessionId: "open" });
  });

  it("switches without an offer when nothing was allowed once", async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === "once_allowed_rules" ? [] : undefined),
    );

    await useStore.getState().setUnattended(true);

    expect(useStore.getState().unattended).toBe(true);
    expect(useStore.getState().leaving).toBeNull();
  });

  it("grants only on the press, and clears the offer either way", async () => {
    invoke.mockImplementation(() => Promise.resolve(undefined));
    useStore.setState({ leaving: ["run_command:cargo", "run_command:cd"] });

    await useStore.getState().grantLeaving(["run_command:cargo"]);
    expect(useStore.getState().leaving).toBeNull();
    expect(calls()).toEqual(["grant_permission_rules"]);
    expect(invoke.mock.calls[0][1]).toEqual({ rules: ["run_command:cargo"] });

    useStore.setState({ leaving: ["run_command:cd"] });
    useStore.getState().dismissLeaving();
    expect(useStore.getState().leaving).toBeNull();
    expect(calls()).toEqual(["grant_permission_rules"]);
  });

  it("takes the offer down when the conversation is attended again", async () => {
    invoke.mockImplementation(() => Promise.resolve(undefined));
    useStore.setState({ unattended: true, leaving: ["run_command:cargo"] });

    await useStore.getState().setUnattended(false);

    expect(useStore.getState().leaving).toBeNull();
    expect(calls()).toEqual(["set_unattended"]);
  });
});
