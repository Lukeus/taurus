// Whether a turn this window sent is read aloud. Off by default, so the one
// thing to pin besides "on reads it" is that off sends nothing at all: a
// speech command per streamed piece is a backend round trip nobody asked for,
// and with ElevenLabs chosen, a keychain read.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {
    onmessage: ((event: unknown) => void) | undefined;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import type { AppStatus } from "../lib/api";
import { useSpeech } from "./speech";
import { useStore } from "./store";

const OPEN = {
  id: "open",
  model: "qwen3.6:27b",
  provider_id: "ollama",
  native_tools: true,
  vision: false,
  context_length: 32_000,
};

/** A status whose only setting that matters here is `read_aloud`. */
const status = (readAloud: boolean) =>
  ({
    workspace: "/w",
    settings: { speech: { read_aloud: readAloud } },
  }) as unknown as AppStatus;

/** The speech commands the turn caused, in order. */
const spoken = () =>
  invoke.mock.calls
    .filter(([name]) => String(name).startsWith("speech_"))
    .map(([name, args]) => (args && "text" in args ? `${name}(${args.text})` : name));

beforeEach(() => {
  invoke.mockReset();
  // The turn streams one sentence and ends.
  invoke.mockImplementation((name: string, args: { onEvent?: { onmessage?: (e: unknown) => void } }) => {
    if (name === "send_message") {
      args.onEvent?.onmessage?.({ type: "text_delta", text: "Hello there. " });
      args.onEvent?.onmessage?.({ type: "turn_finished" });
    }
    if (name === "list_sessions" || name === "list_checkpoints") return Promise.resolve([]);
    return Promise.resolve(undefined);
  });
  useSpeech.setState({ speaking: false, reading: null, failed: null });
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

const settle = () => new Promise((r) => setTimeout(r, 0));

describe("reading a sent turn aloud", () => {
  it("feeds the reply and flushes when read-aloud is on", async () => {
    useStore.setState({ status: status(true) });
    await useStore.getState().send("hi");
    await settle();
    expect(spoken()).toEqual(["speech_feed(Hello there. )", "speech_flush"]);
  });

  it("sends nothing when it's off", async () => {
    useStore.setState({ status: status(false) });
    await useStore.getState().send("hi");
    await settle();
    expect(spoken()).toEqual([]);
  });

  it("stops a reply still being read when the next message is sent", async () => {
    useStore.setState({ status: status(false) });
    useSpeech.setState({ speaking: true });
    await useStore.getState().send("next");
    await settle();
    expect(spoken()).toEqual(["speech_stop"]);
  });
});
