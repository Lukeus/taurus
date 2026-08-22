import { describe, expect, it } from "vitest";

import { bytes, fade } from "./terminal";

describe("decoding what the shell printed", () => {
  it("hands back the bytes rather than a string", () => {
    // "hi\n" — the ordinary case, and the one that says the loop is not off by
    // one at either end.
    expect([...bytes("aGkK")]).toEqual([104, 105, 10]);
  });

  it("keeps a multi-byte character whole across the chunk it was split by", () => {
    /*
     * The bug this exists to prevent. A read returns whatever was ready, so the
     * two bytes of "é" can arrive one per chunk whenever a terminal is busy.
     * Decoded as text here, each half becomes a replacement character and the
     * emulator never sees the character at all; passed on as bytes, the two
     * chunks concatenate back into what was printed.
     */
    const first = bytes("ww=="); // 0xc3, the lead byte of "é"
    const second = bytes("qQ=="); // 0xa9, the one that completes it
    const whole = new Uint8Array([...first, ...second]);
    expect(new TextDecoder().decode(whole)).toBe("é");
    // And each half on its own is not a character, which is the point: nothing
    // in this file is in a position to decide what these bytes mean.
    expect(first).toHaveLength(1);
    expect(second).toHaveLength(1);
  });

  it("survives escape sequences, which are most of what a terminal sends", () => {
    // ESC [ 3 1 m — the red a `git diff` asks for. Nothing here may treat the
    // escape byte as anything but a byte.
    expect([...bytes("G1szMW0=")]).toEqual([27, 91, 51, 49, 109]);
  });

  it("says nothing at all rather than throwing on an empty chunk", () => {
    expect(bytes("")).toHaveLength(0);
  });
});

describe("the selection highlight", () => {
  it("turns the palette's accent into something the emulator can parse", () => {
    // The emulator understands hex and rgba. `color-mix`, which the stylesheet
    // uses everywhere for exactly this, would be dropped in silence.
    expect(fade("#7cd2ff", 0.3)).toBe("rgba(124, 210, 255, 0.3)");
  });

  it("expands the short form, which is a legal thing to write in CSS", () => {
    expect(fade("#0af", 0.5)).toBe("rgba(0, 170, 255, 0.5)");
  });

  it("falls back to the accent when the property resolved to nothing", () => {
    /*
     * `getComputedStyle` answers with an empty string in a document that has
     * not painted, and with a `color-mix(...)` for any token defined as one. A
     * highlight that is quietly absent is worse than one that is the wrong
     * shade of the right colour.
     */
    for (const missing of ["", "color-mix(in srgb, red 30%, transparent)", "#nothex"]) {
      expect(fade(missing, 0.3)).toBe("rgba(124, 210, 255, 0.3)");
    }
  });
});
