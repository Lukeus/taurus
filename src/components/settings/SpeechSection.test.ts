// The system voice picker's grouping: nearly two hundred voices on macOS, so
// the reader's own language has to come first.
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { groupVoices } from "./SpeechSection";

const voice = (name: string, detail: string) => ({ id: name, name, detail });

describe("groupVoices", () => {
  it("puts the reader's own locale first, then its language, then the rest", () => {
    const groups = groupVoices(
      [
        voice("Anna", "de_DE"),
        voice("Daniel", "en_GB"),
        voice("Samantha", "en_US"),
        voice("Alex", "en_US"),
        voice("Amélie", "fr_CA"),
      ],
      "en-US",
    );
    expect(groups.map(([tag]) => tag)).toEqual(["en_US", "en_GB", "de_DE", "fr_CA"]);
    expect(groups[0][1].map((v) => v.name)).toEqual(["Alex", "Samantha"]);
  });

  it("files a voice with no language under Other", () => {
    expect(groupVoices([voice("Mystery", "")], "en-US")).toEqual([
      ["Other", [voice("Mystery", "")]],
    ]);
  });
});
