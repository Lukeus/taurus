// @vitest-environment jsdom
//
// The voice list is read per engine, and why it couldn't be read is the one
// thing that tells someone on Linux what to install.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { SpeechSection } from "./SpeechSection";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const STATUS = {
  settings: {
    read_aloud: false,
    engine: "system",
    system: {},
    elevenlabs: { model: "eleven_flash_v2_5" },
    openai: { model: "gpt-4o-mini-tts", voice: "alloy" },
  },
  key_statuses: [],
  speaking: false,
};

afterEach(() => {
  document.body.innerHTML = "";
  invoke.mockReset();
});

describe("the system voice", () => {
  it("says why its list couldn't be read", async () => {
    invoke.mockImplementation((command: string) =>
      command === "speech_status"
        ? Promise.resolve(STATUS)
        : command === "speech_voices"
          ? Promise.reject("Linux has no built-in voice Taurus can use. Install espeak-ng.")
          : Promise.resolve([]),
    );
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => root.render(<SpeechSection keychain />));
    await act(async () => {});
    expect(host.textContent).toContain("Install espeak-ng");
    act(() => root.unmount());
  });
});
