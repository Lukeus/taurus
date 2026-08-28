import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { check, failures, luminance, PAIRS, parseHex, ratio } from "./contrast";
import { COLOR_TOKENS } from "./theme";

describe("reading a colour", () => {
  it("reads the four hex forms CSS accepts", () => {
    expect(parseHex("#fff")).toEqual({ r: 255, g: 255, b: 255 });
    expect(parseHex("#ffff")).toEqual({ r: 255, g: 255, b: 255 });
    expect(parseHex("#7cd2ff")).toEqual({ r: 124, g: 210, b: 255 });
    expect(parseHex("#7cd2ff80")).toEqual({ r: 124, g: 210, b: 255 });
  });

  it("expands the short form the way CSS does", () => {
    // `#abc` is `#aabbcc`, not `#a0b0c0`. Getting this wrong shifts every
    // hand-written short-form colour by a barely visible amount, which is the
    // kind of bug that is never noticed and never right.
    expect(parseHex("#abc")).toEqual(parseHex("#aabbcc"));
  });

  it("refuses everything that is not one of them", () => {
    // The theme format is hex-only and Rust refuses to save anything else, so
    // scoring a named colour here would report a number for a value that can
    // never reach the file.
    for (const value of ["rebeccapurple", "rgb(0,0,0)", "#12345", "", "#gg0000"]) {
      expect(parseHex(value), value).toBeNull();
    }
  });
});

describe("the ratio", () => {
  it("puts black against white at the top of the scale", () => {
    expect(ratio("#000000", "#ffffff")).toBeCloseTo(21, 5);
  });

  it("puts a colour against itself at the bottom", () => {
    expect(ratio("#7cd2ff", "#7cd2ff")).toBeCloseTo(1, 5);
  });

  it("does not care which way round the two are given", () => {
    expect(ratio("#0b0f14", "#eef2f6")).toBeCloseTo(ratio("#eef2f6", "#0b0f14")!, 10);
  });

  it("is null when either side is not a colour", () => {
    expect(ratio("#000", "chartreuse")).toBeNull();
  });

  it("weights green the most, the way the eye does", () => {
    // The sanity check on the coefficients: pure green is far brighter than
    // pure blue at the same channel value, and a transcription error in the
    // luminance formula would show up here before it showed up as a palette
    // that scores fine and reads badly.
    expect(luminance({ r: 0, g: 255, b: 0 })).toBeGreaterThan(
      luminance({ r: 0, g: 0, b: 255 }),
    );
  });
});

describe("checking a palette", () => {
  /** The two palettes this app ships, read out of the stylesheet. */
  const shipped = (selector: string): Record<string, string> => {
    const css = readFileSync("src/styles.css", "utf8");
    const block = css.slice(css.indexOf(selector));
    const body = block.slice(0, block.indexOf("\n}"));
    const out: Record<string, string> = {};
    for (const [name, token] of Object.entries(COLOR_TOKENS)) {
      const found = body.match(new RegExp(`${token}:\\s*(#[0-9a-f]{3,8}|var\\(--[a-z-]+\\))`, "i"));
      if (!found) continue;
      // One token in the dark palette is declared as another token rather than
      // as a colour — `--lk-on-cyan: var(--lk-ink)` — because what sits on the
      // accent is the ground it came from. Left unresolved, the pair that
      // checks a filled button's label would silently score nothing.
      const alias = found[1].match(/^var\((--[a-z-]+)\)$/);
      out[name] = alias
        ? (body.match(new RegExp(`${alias[1]}:\\s*(#[0-9a-f]{3,8})`, "i"))?.[1] ?? found[1])
        : found[1];
    }
    return out;
  };

  it("finds the palettes it is meant to be checking", () => {
    // The guard on the guard: a stylesheet that stopped declaring these in one
    // block would make both tests below pass against an empty object.
    expect(Object.keys(shipped(":root {"))).toHaveLength(14);
    expect(Object.keys(shipped(':root[data-theme="light"]'))).toHaveLength(14);
  });

  /*
   * The app's own two palettes have to pass the check it holds everyone else
   * to. This is not a formality: the light theme exists because the dark
   * theme's accents could not simply be reused — cyan at `#7cd2ff` carries
   * 11:1 against ink and 1.6:1 against white — and that work is exactly what
   * this would catch being undone.
   */
  /*
   * The app's own two palettes have to pass the check it holds everyone else
   * to. This is the tool that found they did not: `--text-faint` in the dark
   * palette measured 3.19:1 on a panel, where its 10px labels want 4.5:1,
   * while the light palette had always had the same weight right and said so
   * in its own comment. Both clear it now, and this is what keeps that true.
   */
  it("passes the dark palette the app ships", () => {
    expect(failures(shipped(":root {"))).toEqual([]);
  });

  it("passes the light palette the app ships", () => {
    expect(failures(shipped(':root[data-theme="light"]'))).toEqual([]);
  });

  it("catches an accent that cannot be read on the surface it sits on", () => {
    const palette = { ...shipped(":root {"), accent: "#101820" };
    const found = failures(palette);
    expect(found.some((f) => f.fg === "accent")).toBe(true);
  });

  it("names where on screen each failure is", () => {
    // A warning that cannot be tied to something the user has looked at is a
    // warning they turn off. "text-faint on surface-1" names two tokens; "the
    // 10px labels under a conversation title" names a thing.
    for (const pair of PAIRS) {
      expect(pair.what.length, pair.fg).toBeGreaterThan(10);
    }
  });

  it("checks only pairs whose colours the app has", () => {
    // A pair naming a key that is not in the palette would be a check nobody
    // can act on, because there is no field in the editor to change.
    const known = new Set(Object.keys(COLOR_TOKENS));
    for (const pair of PAIRS) {
      expect(known.has(pair.fg), pair.fg).toBe(true);
      expect(known.has(pair.bg), pair.bg).toBe(true);
    }
  });

  it("passes rather than fails a palette it was handed incomplete", () => {
    // An unresolved palette is a bug in this app, not in somebody's theme, and
    // answering it with red warnings on their screen would blame them for it.
    expect(check({}).every((r) => r.ok)).toBe(true);
  });

  it("puts the worst failure first", () => {
    const palette = { ...shipped(":root {"), text: "#0c1014", accent: "#0d1116" };
    const found = failures(palette);
    expect(found.length).toBeGreaterThan(1);
    expect(found[0].ratio!).toBeLessThanOrEqual(found[1].ratio!);
  });
});
