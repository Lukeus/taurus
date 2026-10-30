// A turn's end and the last of its text reach the window by different roads:
// the end is a pushed event, the text a channel the store holds for a frame.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const listeners = new Map<string, (event: { payload: unknown }) => void>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {
    onmessage: unknown;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return Promise.resolve(() => {});
  },
}));

import { EVENT_TURN } from "../lib/api";
import { useStore } from "./store";

const OPEN = {
  id: "open",
  model: "qwen3.6:27b",
  provider_id: "ollama",
  native_tools: true,
  vision: false,
  context_length: 32_000,
};

beforeEach(async () => {
  invoke.mockReset();
  invoke.mockImplementation((command: string) => {
    if (command === "attach_session") {
      return Promise.resolve({
        turn: { started_at: 1_700_000_000, iteration: 1 },
        dropped: 0,
        unattended: false,
      });
    }
    return Promise.resolve(
      ["list_sessions", "list_checkpoints", "once_allowed_rules", "running_sessions"].includes(command)
        ? []
        : undefined,
    );
  });
  await useStore.getState().init();
  useStore.setState({ session: OPEN, entries: [], busy: false, turn: null, running: [] });
});

describe("a turn ending with text still waiting out its frame", () => {
  it("keeps the tail in the bubble it belongs to", async () => {
    await useStore.getState().attach(OPEN.id);
    const channel = invoke.mock.calls.find(([command]) => command === "attach_session")![1].onEvent;

    channel.onmessage({ type: "text_delta", text: "to close `TODO(ARCHETYPE_L" });
    // The start of it is already on screen, so there is a bubble to close.
    await new Promise((resolve) => setTimeout(resolve, 100));
    channel.onmessage({ type: "text_delta", text: "IST)`." });
    listeners.get(EVENT_TURN)!({ payload: { session: OPEN.id, turn: null } });

    // Past the frame the tail was held for, which is when it used to land.
    await new Promise((resolve) => setTimeout(resolve, 100));

    const answers = useStore.getState().entries.filter((e) => e.kind === "assistant");
    expect(answers).toHaveLength(1);
    expect(answers[0]).toMatchObject({ text: "to close `TODO(ARCHETYPE_LIST)`.", open: false });
  });
});
