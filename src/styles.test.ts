import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/*
 * Comments are stripped first. Without that, `[^{]*` runs straight through a
 * comment block and the "selector" it captures is the comment plus the real
 * selector — which is how the first version of this test passed while the
 * duplicate it was written to catch was still in the file.
 */
const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  "",
);

/** The selector group of every top-level rule. */
function rules(): string[] {
  // A rule that starts at column 0 — anything indented is inside a block.
  return [...css.matchAll(/^([^\s@}][^{]*)\{/gm)].map(([, group]) =>
    group.trim().replace(/\s+/g, " "),
  );
}

describe("the stylesheet", () => {
  it("finds the rules it is meant to be checking", () => {
    // The guard on the guard. A regex that matched nothing would make the test
    // below pass forever, which is exactly how it failed the first time.
    const found = rules();
    expect(found.length).toBeGreaterThan(250);
    expect(found).toContain(".chip");
    expect(found).toContain(".thinking-body");
  });

  it("gives each bare class a rule of its own exactly once", () => {
    /*
     * The bug this exists for: `.chip` was the "N files changed" button in the
     * header, and a component added later declared `.chip` again for its own
     * tool pills. The second rule won, the header button silently became 10px
     * mono, and nothing failed — not the typecheck, not a test, not the build.
     *
     * Only rules whose whole selector is one bare class count. Sharing a class
     * across grouped rules is how this file layers on purpose — `.tool-output,
     * .proposal-body { }` narrowing a block those two share is fine. A second
     * `.chip { }` is a name taken twice.
     */
    const owned = rules().filter((r) => /^\.[a-z][a-z0-9-]*$/i.test(r));
    const counts = new Map<string, number>();
    for (const r of owned) counts.set(r, (counts.get(r) ?? 0) + 1);

    const taken = [...counts].filter(([, n]) => n > 1).map(([s]) => s);
    expect(taken).toEqual([]);
  });

  /** The body of the first top-level rule whose selector group contains `sel`. */
  function block(sel: string): string | undefined {
    for (const [, group, body] of css.matchAll(/^([^\s@}][^{]*)\{([^}]*)\}/gm)) {
      if (group.includes(sel)) return body;
    }
  }

  /** The `outline-offset` a rule declares, in px. */
  function offset(body: string | undefined): number | undefined {
    const px = body?.match(/outline-offset:\s*(-?[\d.]+)px/)?.[1];
    return px === undefined ? undefined : Number(px);
  }

  it("offsets the focus ring far enough off a control to read as a ring", () => {
    /*
     * At the 1px this shipped with, a ring in --accent sitting off a button
     * *filled* with --accent reads as a slightly thicker border rather than as
     * a state. The gap is what makes it a ring.
     */
    expect(offset(block(":focus-visible"))).toBeGreaterThanOrEqual(2);
  });

  it("turns the ring inward on the controls whose parent clips", () => {
    /*
     * The bug this exists for: an outline is painted outside the border box, so
     * a control filling a parent that carries `overflow: hidden` has its ring
     * clipped away completely rather than merely trimmed. `.run-head` — the
     * fold control on every tool-call card — was keyboard-focusable with
     * nothing on screen to say so, and rendered pixel-identical to an unfocused
     * card. Nothing caught it: the ring was declared, it just never reached a
     * pixel. A positive offset on any of these is that bug coming back.
     */
    for (const control of [".run-head", ".run-row-head", ".table-sort"]) {
      const body = block(`${control}:focus-visible`);
      expect(body, `${control} has no :focus-visible rule`).toBeDefined();
      expect(offset(body), `${control} would have its ring clipped away`).toBeLessThan(0);
    }
  });

  it("gives the one destructive control in the rail a pointer-sized target", () => {
    /*
     * Measured at 23×23 in the running app — the smallest thing in it, and the
     * only irreversible one, four pixels from the row that merely selects.
     */
    const body = block(".rail-delete");
    expect(body).toBeDefined();
    for (const axis of ["min-width", "min-height"]) {
      const px = Number(body!.match(new RegExp(`${axis}:\\s*([\\d.]+)px`))?.[1]);
      expect(px, `.rail-delete declares no ${axis}`).toBeGreaterThanOrEqual(28);
    }
  });

  it("does not fade a filled button into an unreadable one", () => {
    /*
     * `opacity` composites fill and label together, so a disabled primary took
     * its own label down with it: 1.65:1 on dark, 1.29:1 on light, both
     * measured. The filled variant has to drop its fill instead.
     */
    const body = block("button.primary:disabled");
    expect(body, "no disabled rule for the filled variant").toBeDefined();
    expect(body).toMatch(/opacity:\s*1\b/);
    expect(body).toMatch(/color:/);
  });

  it("paints the window in the same two inks the stylesheet does", () => {
    /*
     * A webview holds its host's default ground — white — until the document
     * first paints, so the window itself has to be told the palette before
     * there is a stylesheet to read it from. That means these two values exist
     * twice, in `src-tauri`, and nothing in either file can derive the other.
     *
     * This is the thing that notices when the palette moves and the window
     * does not: the symptom otherwise is a one-frame flash of the *old* theme
     * on every launch, which nobody reports and nobody can screenshot.
     */
    const ink = (body: string | undefined) =>
      body?.match(/--lk-ink:\s*(#[0-9a-f]{6})/i)?.[1].toLowerCase();

    const dark = ink(block(":root"));
    const light = ink(block(':root[data-theme="light"]'));
    expect(dark, "the dark palette declares no --lk-ink").toBeDefined();
    expect(light, "the light palette declares no --lk-ink").toBeDefined();

    const rust = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
    const constant = (name: string) => {
      const hex = rust
        .match(new RegExp(`${name}: tauri::window::Color = [^;]+;`))?.[0]
        .match(/0x([0-9a-f]{2}), 0x([0-9a-f]{2}), 0x([0-9a-f]{2})/i);
      return hex && `#${hex[1]}${hex[2]}${hex[3]}`.toLowerCase();
    };
    expect(constant("DARK"), "the window's dark ground has drifted").toBe(dark);
    expect(constant("LIGHT"), "the window's light ground has drifted").toBe(light);

    // The config value is what the window is created with, before any of the
    // above runs; it is the dark one because that is what the app defaults to.
    const conf = JSON.parse(
      readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
    );
    expect(conf.app.windows[0].backgroundColor?.toLowerCase()).toBe(dark);
  });

  it("lets the browser skip a turn that is scrolled off", () => {
    /*
     * The transcript has no virtualiser: every turn ever rendered stays in the
     * document, so layout and paint cost grow with conversation length however
     * well the React memos hold. `content-visibility` is what buys that back,
     * and it is only worth having with an intrinsic size beside it — without
     * one every off-screen turn collapses to zero height and the scrollbar
     * jumps. The `auto` keyword is the part that stops a turn already measured
     * from being guessed at again.
     */
    const body = block(".turn");
    expect(body).toBeDefined();
    expect(body, "off-screen turns still cost a full layout").toMatch(
      /content-visibility:\s*auto/,
    );
    expect(body, "a skipped turn would collapse to nothing").toMatch(
      /contain-intrinsic-size:\s*auto\s+[\d.]+px/,
    );
  });

  it("keeps empty, loading and failed as three different states", () => {
    /*
     * All three used to share `.drawer-empty`, so a drawer whose read failed
     * looked exactly like one that succeeded and found nothing.
     */
    const colour = (sel: string) => block(sel)?.match(/color:\s*([^;]+);/)?.[1].trim();
    const states = [".drawer-empty", ".drawer-loading", ".drawer-error"].map((s) => {
      const c = colour(s);
      expect(c, `${s} declares no colour`).toBeDefined();
      return c;
    });
    expect(new Set(states).size, "two of the states paint the same colour").toBe(3);
    expect(colour(".drawer-error")).toContain("--danger");
  });
});
