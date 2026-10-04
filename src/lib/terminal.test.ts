import { describe, expect, it } from "vitest";

import { acknowledger, askAboutBlock, bytes, duration, fade } from "./terminal";

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

describe("acknowledging what was drawn", () => {
  it("holds what is drawn before the shell has a name, and sends it once named", () => {
    // Output can arrive before `terminal_open` answers. A first screen that is
    // never acknowledged is room the shell never gets back.
    const sent: [string, number][] = [];
    const acks = acknowledger((id, n) => sent.push([id, n]));
    acks.drawn(100);
    acks.drawn(20);
    expect(sent).toEqual([]);

    acks.bind("t1");

    expect(sent).toEqual([["t1", 120]]);
  });

  it("acknowledges each write as it is drawn once the shell is named", () => {
    const sent: [string, number][] = [];
    const acks = acknowledger((id, n) => sent.push([id, n]));
    acks.bind("t1");
    acks.drawn(64);
    acks.drawn(8);
    // Nothing was held, so naming the shell sent nothing of its own.
    expect(sent).toEqual([
      ["t1", 64],
      ["t1", 8],
    ]);
  });
});

describe("askAboutBlock", () => {
  const block = { id: 7, command: "cargo test", running: false, full_screen: false };

  it("quotes a failure and asks what went wrong", () => {
    const text = askAboutBlock({ ...block, exit: 101 }, "test a ... FAILED");
    expect(text).toBe(
      "I ran this in the terminal and it exited 101:\n\n```\n$ cargo test\ntest a ... FAILED\n```\n\nWhat went wrong?",
    );
  });

  it("leaves a success open for the person to finish", () => {
    const text = askAboutBlock({ ...block, exit: 0 }, "ok");
    expect(text.startsWith("I ran this in the terminal:\n")).toBe(true);
    expect(text.endsWith("```")).toBe(true);
  });

  it("keeps the end of long output and says where the rest is", () => {
    const output = Array.from({ length: 200 }, (_, n) => `line ${n}`).join("\n");
    const text = askAboutBlock({ ...block, exit: 1 }, output);
    expect(text).toContain("line 199");
    expect(text).not.toContain("line 0\n");
    expect(text).toContain("…\n");
    expect(text).toContain("command #7 in the terminal");
  });

  it("can't be closed early by a fence in the output", () => {
    const text = askAboutBlock({ ...block, exit: 0 }, "```js\nx\n```");
    expect(text).toContain("````\n$ cargo test");
  });

  it("says when there's nothing to quote", () => {
    expect(askAboutBlock({ ...block, exit: 0 }, "")).toContain("(no output)");
    expect(askAboutBlock({ ...block, exit: 0, full_screen: true }, "")).toContain(
      "full-screen program",
    );
    expect(askAboutBlock({ ...block, running: true }, "listening")).toContain(
      "it's still running",
    );
  });
});

describe("duration", () => {
  it("reads the way a person says it", () => {
    expect(duration(850)).toBe("850ms");
    expect(duration(4_200)).toBe("4.2s");
    expect(duration(125_000)).toBe("2m05s");
  });
});
