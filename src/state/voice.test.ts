// The mic's state machine, with Tauri stood in for. What each test pins is a
// promise the composer makes: nothing downloads without a yes, Escape always
// wins, and a cancel is never reported as a failure.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const channels: { onmessage?: (v: unknown) => void }[] = [];
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {
    onmessage?: (v: unknown) => void;
    constructor() {
      channels.push(this);
    }
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { clock, megabytes, useVoice } from "./voice";

const status = (installed: boolean) => ({
  model: "base.en",
  models: [
    { id: "tiny.en", summary: "", bytes: 32_166_155, english_only: true, installed: false },
    { id: "base.en", summary: "", bytes: 59_721_011, english_only: true, installed },
  ],
  microphone: "MacBook Pro Microphone",
  folder: "/home/me/.taurus/models/whisper",
});

/** Answers each command the way the backend would, from a table. */
function backend(answers: Record<string, (args: unknown) => unknown>) {
  invoke.mockImplementation((cmd: string, args: unknown) => {
    const answer = answers[cmd];
    if (!answer) return Promise.resolve(undefined);
    try {
      return Promise.resolve(answer(args));
    } catch (e) {
      return Promise.reject(e);
    }
  });
}

const calls = (cmd: string) => invoke.mock.calls.filter(([c]) => c === cmd);

beforeEach(() => {
  invoke.mockReset();
  channels.length = 0;
  useVoice.setState({
    phase: { kind: "idle" },
    level: 0,
    status: null,
    download: null,
    notice: null,
    heard: null,
  });
});

describe("pressing the mic", () => {
  it("offers the download when the model isn't on disk, and opens nothing", async () => {
    backend({ voice_status: () => status(false) });
    await useVoice.getState().press();

    const { phase } = useVoice.getState();
    expect(phase.kind).toBe("offer");
    expect(phase.kind === "offer" && phase.model.id).toBe("base.en");
    expect(calls("voice_start")).toHaveLength(0);
    expect(calls("voice_download")).toHaveLength(0);
  });

  it("listens, then hands over what it heard", async () => {
    backend({ voice_status: () => status(true), voice_stop: () => "run the tests" });
    await useVoice.getState().press();
    expect(useVoice.getState().phase.kind).toBe("listening");

    // The level arrives over the channel the start was given.
    channels[0].onmessage?.(0.4);
    expect(useVoice.getState().level).toBe(0.4);

    await useVoice.getState().press();
    expect(useVoice.getState().phase.kind).toBe("idle");
    expect(useVoice.getState().heard).toEqual({ text: "run the tests" });
  });

  it("says why when the microphone won't open", async () => {
    backend({
      voice_status: () => status(true),
      voice_start: () => {
        throw "No microphone found. Plug one in…";
      },
    });
    await useVoice.getState().press();
    expect(useVoice.getState().phase.kind).toBe("idle");
    expect(useVoice.getState().notice).toEqual({
      text: "No microphone found. Plug one in…",
      tone: "error",
    });
  });

  it("reports a transcription failure, but not a cancel", async () => {
    backend({
      voice_status: () => status(true),
      voice_stop: () => {
        throw "Taurus didn't hear any speech.";
      },
    });
    await useVoice.getState().press();
    await useVoice.getState().finish();
    expect(useVoice.getState().notice?.text).toBe("Taurus didn't hear any speech.");

    useVoice.setState({ notice: null });
    backend({
      voice_status: () => status(true),
      voice_stop: () => {
        throw "Canceled.";
      },
    });
    await useVoice.getState().press();
    await useVoice.getState().finish();
    expect(useVoice.getState().notice).toBeNull();
  });
});

describe("escape", () => {
  it("closes the microphone and keeps nothing", async () => {
    backend({ voice_status: () => status(true), voice_stop: () => "never" });
    await useVoice.getState().press();
    useVoice.getState().cancel();

    expect(useVoice.getState().phase.kind).toBe("idle");
    expect(calls("voice_cancel")).toHaveLength(1);
    expect(calls("voice_stop")).toHaveLength(0);
    expect(useVoice.getState().heard).toBeNull();
  });

  it("wins over a start still waiting on the device", async () => {
    // Opening can sit behind a permission dialog. Answering it after Escape
    // must not turn the mic on.
    let open: () => void = () => {};
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "voice_status") return Promise.resolve(status(true));
      if (cmd === "voice_start") return new Promise<void>((r) => (open = r));
      return Promise.resolve(undefined);
    });
    const pressing = useVoice.getState().press();
    await vi.waitFor(() => expect(useVoice.getState().phase.kind).toBe("opening"));

    useVoice.getState().cancel();
    open();
    await pressing;

    expect(useVoice.getState().phase.kind).toBe("idle");
    // Once for Escape, once for the start that arrived after it.
    expect(calls("voice_cancel")).toHaveLength(2);
  });

  it("drops a transcription that finishes after it", async () => {
    let done: (text: string) => void = () => {};
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "voice_status") return Promise.resolve(status(true));
      if (cmd === "voice_stop") return new Promise<string>((r) => (done = r));
      return Promise.resolve(undefined);
    });
    await useVoice.getState().press();
    const finishing = useVoice.getState().finish();
    expect(useVoice.getState().phase.kind).toBe("transcribing");

    useVoice.getState().cancel();
    done("too late");
    await finishing;
    expect(useVoice.getState().heard).toBeNull();
    expect(useVoice.getState().phase.kind).toBe("idle");
  });
});

describe("downloading", () => {
  it("reports progress, then says the mic is ready", async () => {
    let finish: () => void = () => {};
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "voice_status") return Promise.resolve(status(false));
      if (cmd === "voice_download") return new Promise<void>((r) => (finish = r));
      return Promise.resolve(undefined);
    });
    await useVoice.getState().press();
    const downloading = useVoice.getState().startDownload("base.en", true);

    expect(useVoice.getState().phase.kind).toBe("idle");
    channels[0].onmessage?.({ done: 30, total: 60 });
    expect(useVoice.getState().download).toMatchObject({ done: 30, total: 60 });

    finish();
    await downloading;
    expect(useVoice.getState().download).toBeNull();
    expect(useVoice.getState().notice?.tone).toBe("info");
  });

  it("started from Settings, finishes without a notice in the composer", async () => {
    backend({ voice_status: () => status(true) });
    await useVoice.getState().startDownload("tiny.en", false);
    expect(useVoice.getState().notice).toBeNull();
  });

  it("canceled, says nothing", async () => {
    backend({
      voice_status: () => status(false),
      voice_download: () => {
        throw "Canceled.";
      },
    });
    await useVoice.getState().startDownload("base.en", true);
    expect(useVoice.getState().notice).toBeNull();
    expect(useVoice.getState().download).toBeNull();
  });

  it("while one runs, a press doesn't ask again", async () => {
    backend({ voice_status: () => status(false) });
    useVoice.setState({ download: { model: "base.en", done: 1, total: 2, fromMic: true } });
    await useVoice.getState().press();
    expect(useVoice.getState().phase.kind).toBe("idle");
  });
});

describe("formatting", () => {
  it("sizes and times read the way the strip says them", () => {
    expect(megabytes(59_721_011)).toBe("60 MB");
    expect(clock(7.9)).toBe("0:07");
    expect(clock(102)).toBe("1:42");
  });
});
