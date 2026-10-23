// Reading aloud from the window's side, with Tauri stood in for. What each
// test pins is a promise: pieces reach the backend in the order they streamed,
// a stop drops whatever was still queued, and a refusal is said once.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import type { UiEvent } from "../lib/api";
import { endTurn, hear, startTurn, turnText, useSpeech } from "./speech";

const text = (t: string): UiEvent => ({ type: "text_delta", text: t });
const tool = { type: "tool_call_started", id: "t1", name: "read_file" } as UiEvent;

/** Everything sent to the backend, in order, as `command(text)`. */
const sent = () =>
  invoke.mock.calls.map(([cmd, args]) =>
    args && "text" in (args as object) ? `${cmd}(${(args as { text: string }).text})` : cmd,
  );

/** Lets the queued calls run. */
const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
  useSpeech.setState({ speaking: false, reading: null, failed: null });
  startTurn();
  invoke.mockClear();
});

describe("reading a turn as it streams", () => {
  it("sends a batch's prose as one piece, and flushes at a tool call", async () => {
    hear([text("Reading the "), text("config. "), tool, text("It sets")]);
    await settle();
    expect(sent()).toEqual([
      "speech_feed(Reading the config. )",
      "speech_flush",
      "speech_feed(It sets)",
    ]);
  });

  it("keeps the order even when the backend answers out of order", async () => {
    // The first call is slow. Without the queue the second would land first.
    let release!: () => void;
    invoke.mockImplementationOnce(() => new Promise<void>((r) => (release = r)));
    hear([text("one ")]);
    hear([text("two ")]);
    await settle();
    expect(sent()).toEqual(["speech_feed(one )"]);
    release();
    await settle();
    expect(sent()).toEqual(["speech_feed(one )", "speech_feed(two )"]);
  });

  it("flushes at the end of a turn only when something was fed", async () => {
    endTurn();
    await settle();
    expect(sent()).toEqual([]);
    hear([text("Done.")]);
    endTurn();
    await settle();
    expect(sent()).toEqual(["speech_feed(Done.)", "speech_flush"]);
  });
});

describe("stopping", () => {
  it("drops pieces still queued behind a slow one", async () => {
    let release!: () => void;
    invoke.mockImplementationOnce(() => new Promise<void>((r) => (release = r)));
    hear([text("first ")]);
    hear([text("second ")]);
    // The first is with the backend, the second waiting behind it.
    await settle();
    useSpeech.getState().stop();
    release();
    await settle();
    expect(sent()).toEqual(["speech_feed(first )", "speech_stop"]);
  });

  it("a new turn stops the old reading", async () => {
    useSpeech.setState({ speaking: true });
    startTurn();
    await settle();
    expect(sent()).toEqual(["speech_stop"]);
  });
});

describe("a refusal", () => {
  it("is said once, and the rest of the turn isn't sent", async () => {
    invoke.mockRejectedValueOnce("ElevenLabs needs an API key. Add one in Settings › Voice.");
    hear([text("one ")]);
    hear([text("two ")]);
    endTurn();
    await settle();
    expect(sent()).toEqual(["speech_feed(one )"]);
    expect(useSpeech.getState().failed).toBe(
      "ElevenLabs needs an API key. Add one in Settings › Voice.",
    );
  });

  it("holds for pieces that arrive after it, until the next turn", async () => {
    invoke.mockRejectedValueOnce("no key");
    hear([text("one ")]);
    await settle();
    hear([text("two ")]);
    endTurn();
    await settle();
    expect(sent()).toEqual(["speech_feed(one )"]);
    startTurn();
    hear([text("fresh ")]);
    await settle();
    expect(sent()).toEqual(["speech_feed(one )", "speech_feed(fresh )"]);
  });

  it("is cleared by the next turn", () => {
    useSpeech.setState({ failed: "old" });
    startTurn();
    expect(useSpeech.getState().failed).toBeNull();
  });
});

describe("the Read aloud button", () => {
  it("reads every reply in the turn and marks itself as the one reading", async () => {
    useSpeech.getState().say(turnText(["First part.", " ", "Second part."]), "turn-1");
    await settle();
    expect(sent()).toEqual(["speech_say(First part.\n\nSecond part.)"]);
    expect(useSpeech.getState().reading).toBe("turn-1");
  });
});
