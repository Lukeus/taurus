// Moving a conversation to another model, which is the one thing this feature
// has to get right: the transcript survives. Actions rather than the reducer,
// so Tauri has to be stood in for.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { useStore, type Entry } from "./store";

const OPEN = {
  id: "open",
  model: "qwen3.6:27b",
  provider_id: "ollama",
  native_tools: true,
  vision: false,
  context_length: 32_000,
};

/** What the backend answers with once the conversation has moved. */
const MOVED = {
  ...OPEN,
  model: "claude-opus-5",
  provider_id: "anthropic",
  vision: true,
  context_length: 200_000,
};

const SAID: Entry[] = [
  { kind: "user", id: "e1", text: "why is the build slow?" },
  { kind: "assistant", id: "e2", text: "Let me time it.", thinking: "", open: false },
];

const calls = (command: string) =>
  invoke.mock.calls.filter(([name]) => name === command);

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation((command: string) =>
    command === "switch_model" ? Promise.resolve(MOVED) : Promise.resolve(undefined),
  );
  useStore.setState({
    session: OPEN,
    entries: [...SAID],
    changed: ["Cargo.toml"],
    busy: false,
    error: null,
  });
});

describe("moving a conversation to another model", () => {
  it("keeps everything that was said", () => {
    // The whole point. Changing either picker used to start a new
    // conversation, so a second opinion meant retyping the question.
    return useStore
      .getState()
      .switchModel("anthropic", "claude-opus-5")
      .then(() => {
        const entries = useStore.getState().entries;
        expect(entries.slice(0, 2)).toEqual(SAID);
        expect(useStore.getState().changed).toEqual(["Cargo.toml"]);
      });
  });

  it("takes the new model's capabilities, not the old one's", async () => {
    // A model that reads images where the last one did not changes what the
    // composer offers, so this cannot be left showing what it replaced.
    await useStore.getState().switchModel("anthropic", "claude-opus-5");
    expect(useStore.getState().session).toMatchObject({
      model: "claude-opus-5",
      provider_id: "anthropic",
      vision: true,
      context_length: 200_000,
    });
  });

  it("marks where it happened in the transcript", async () => {
    await useStore.getState().switchModel("anthropic", "claude-opus-5");
    const last = useStore.getState().entries.at(-1) as Extract<
      Entry,
      { kind: "notice" }
    >;
    expect(last.kind).toBe("notice");
    expect(last.rule).toMatchObject({ note: "anthropic · claude-opus-5" });
  });

  it("does nothing when the picker lands on what is already open", async () => {
    // Both `<select>`s fire on any change, and a provider switch resolves to a
    // model that may be the one already running.
    await useStore.getState().switchModel("ollama", "qwen3.6:27b");
    expect(calls("switch_model")).toHaveLength(0);
    expect(useStore.getState().entries).toEqual(SAID);
  });

  it("leaves the conversation alone when the backend refuses", async () => {
    // Refused mid-turn, and refused for a model the provider will not serve.
    // Either way the conversation is exactly where it was, and saying so in
    // the banner beats throwing it at a `<select>`.
    invoke.mockImplementation(() =>
      Promise.reject(new Error("this conversation is mid-turn")),
    );
    await useStore.getState().switchModel("anthropic", "claude-opus-5");

    expect(useStore.getState().session).toEqual(OPEN);
    expect(useStore.getState().entries).toEqual(SAID);
    expect(useStore.getState().error).toContain("mid-turn");
  });

  it("does nothing at all with no conversation open", async () => {
    useStore.setState({ session: null });
    await useStore.getState().switchModel("anthropic", "claude-opus-5");
    expect(calls("switch_model")).toHaveLength(0);
  });
});
