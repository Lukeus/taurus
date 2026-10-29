/**
 * Whether a font family draws every letter the same width.
 *
 * The terminal needs that and cannot fake it. xterm sizes every cell to one
 * "W" and places each glyph at the start of its cell, so a proportional face
 * comes out as letters adrift in wide boxes — "W e l c o m e" — with box
 * drawing broken into dashes, because a "─" no longer reaches the next one.
 * A theme's mono face or a name typed in Settings can be either kind, and
 * nothing about the name says which.
 *
 * Measured: an "i" run against a "W" run, in the family with `monospace`
 * behind it. A family that isn't installed falls through to `monospace` and
 * passes, which is right — the browser skips it in the stack too, so it can't
 * do any harm there. The same goes for a symbols-only face like the Nerd Font
 * ones: it has no letters, so its letters are the fallback's.
 *
 * Where there is nothing to measure with — the tests' DOM has no canvas —
 * every family passes, which is the behavior from before this existed.
 */
export function fixedPitch(family: string): boolean {
  const name = family.trim();
  if (!name || GENERIC.has(name)) return true;
  const known = verdicts.get(name);
  if (known !== undefined) return known;
  const ctx = context();
  if (!ctx) return true;

  // Set to the fallback first: a family the canvas can't parse leaves `font`
  // as it was, and then this measures the fallback rather than whatever the
  // last call left behind.
  ctx.font = "64px monospace";
  ctx.font = `64px ${name}, monospace`;
  const narrow = ctx.measureText("i".repeat(16)).width;
  const wide = ctx.measureText("W".repeat(16)).width;
  // A hair of slack for hinting; a proportional face misses by half or more.
  const fixed = Math.abs(wide - narrow) <= wide * 0.02;
  verdicts.set(name, fixed);
  return fixed;
}

/**
 * The families in a CSS `font-family` list, each as written.
 *
 * Split on the commas that are outside quotes, so `"Fira, Code"` stays one.
 */
export function families(list: string): string[] {
  const out: string[] = [];
  let current = "";
  let quote: string | null = null;
  for (const ch of list) {
    if (quote) {
      if (ch === quote) quote = null;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (ch === ",") {
      if (current.trim()) out.push(current.trim());
      current = "";
      continue;
    }
    current += ch;
  }
  if (current.trim()) out.push(current.trim());
  return out;
}

/** A family's name without the quotes round it, for a sentence. */
export function bare(family: string): string {
  return family.trim().replace(/^(["'])(.*)\1$/, "$2");
}

const GENERIC = new Set(["monospace", "ui-monospace"]);

// Per family, for the life of the window. A font installed while it's open
// is the one case this gets wrong, and a restart puts it right.
const verdicts = new Map<string, boolean>();

let canvas: OffscreenCanvasRenderingContext2D | null | undefined;
function context(): OffscreenCanvasRenderingContext2D | null {
  if (canvas === undefined) {
    try {
      canvas =
        typeof OffscreenCanvas === "undefined"
          ? null
          : new OffscreenCanvas(1, 1).getContext("2d");
    } catch {
      canvas = null;
    }
  }
  return canvas;
}
