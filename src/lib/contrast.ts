/**
 * Whether a palette can actually be read.
 *
 * The app's own two palettes were picked against this and it shows in the
 * stylesheet's comments: cyan at `#7cd2ff` carries 11:1 against ink and 1.6:1
 * against white, so the light theme re-picks every accent at the same job and
 * the same hue rather than reusing one value. That work is invisible until
 * somebody else gets to choose the colours — at which point the single most
 * likely outcome of a branding feature is a window nobody can read, arrived at
 * one plausible-looking swatch at a time.
 *
 * So the editor checks, and it checks the *pairs that exist on screen* rather
 * than the swatches on their own. A colour is neither legible nor illegible by
 * itself; it is legible against the thing it sits on, and which things those
 * are is a fact about this app's layout that nobody choosing a violet can be
 * expected to hold in their head.
 *
 * Pure and here rather than in the component, for the reason `palette.ts` and
 * `sql.ts` are: it is the part worth testing, and none of it needs a DOM.
 */

/** A colour, unpacked. Alpha is carried but ignored — see [`ratio`]. */
export type Rgb = { r: number; g: number; b: number };

/**
 * Reads `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
 *
 * Returns null for anything else, including the named colours and the
 * `rgb()` functions CSS would accept. The theme format is hex-only and says
 * so, which is the same decision made once in two places: Rust refuses to
 * save anything else, and this refuses to score it.
 */
export function parseHex(value: string): Rgb | null {
  const digits = value.trim().replace(/^#/, "");
  if (digits.length !== digits.replace(/[^0-9a-f]/gi, "").length) return null;

  if (digits.length === 3 || digits.length === 4) {
    const [r, g, b] = [...digits.slice(0, 3)].map((c) => parseInt(c + c, 16));
    return { r, g, b };
  }
  if (digits.length === 6 || digits.length === 8) {
    return {
      r: parseInt(digits.slice(0, 2), 16),
      g: parseInt(digits.slice(2, 4), 16),
      b: parseInt(digits.slice(4, 6), 16),
    };
  }
  return null;
}

/** WCAG relative luminance, on the sRGB curve. */
export function luminance({ r, g, b }: Rgb): number {
  const channel = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/**
 * The WCAG contrast ratio between two colours, 1 to 21, or null if either is
 * not a colour.
 *
 * Alpha is ignored rather than composited. A translucent foreground's real
 * contrast depends on what is behind it — which, in a window of stacked
 * surfaces, is not one answer — and a checker that guessed would report a
 * number that is wrong somewhere on screen. Ignoring it scores the colour the
 * author picked, which is the thing they can act on.
 */
export function ratio(a: string, b: string): number | null {
  const first = parseHex(a);
  const second = parseHex(b);
  if (!first || !second) return null;
  const [light, dark] = [luminance(first), luminance(second)].sort((x, y) => y - x);
  return (light + 0.05) / (dark + 0.05);
}

/** One pair the app actually puts on screen. */
export type Pair = {
  /** The palette key drawn on top. */
  fg: string;
  /** The palette key it is drawn on. */
  bg: string;
  /** The ratio this has to clear. */
  needs: number;
  /**
   * Where in the app this pair is, in words.
   *
   * "text-faint on surface-1" names two tokens; "the 10px labels under a
   * conversation title" names a thing somebody has looked at. A warning that
   * cannot be tied to something on screen is a warning people turn off.
   */
  what: string;
  /**
   * Whether failing this makes text unreadable or only makes an edge faint.
   *
   * Two levels rather than one, because they deserve different answers. A
   * 3:1 body text is a bug in somebody's theme. A hairline at 1.2:1 is a
   * choice — flat, borderless designs are a real thing people want — and an
   * editor that refused it would be enforcing a taste rather than a floor.
   */
  kind: "text" | "edge";
};

/**
 * Every pair worth checking, and what each one is on screen.
 *
 * Read off the app rather than generated from the palette: most of the 196
 * possible pairings never touch each other, and a checker reporting on
 * `danger` against `on-accent` would bury the eleven that matter.
 *
 * Small text takes 4.5:1 throughout, including the three weights that are
 * *meant* to recede. That is not a mistake about the design — `--text-faint`
 * carries the 10px mono micro-labels, and small text needs more contrast, not
 * less, which is exactly why the shipped light palette darkens it relative to
 * its job rather than lightening it.
 */
export const PAIRS: readonly Pair[] = [
  { fg: "text", bg: "ink", needs: 4.5, what: "A sentence in the transcript", kind: "text" },
  { fg: "text", bg: "surface-1", needs: 4.5, what: "A sentence on a panel", kind: "text" },
  { fg: "text", bg: "surface-2", needs: 4.5, what: "A sentence on a raised panel", kind: "text" },
  {
    fg: "text-dim",
    bg: "surface-1",
    needs: 4.5,
    what: "A conversation title in the rail",
    kind: "text",
  },
  {
    fg: "text-faint",
    bg: "surface-1",
    needs: 4.5,
    what: "The 10px labels under it — model, time, file count",
    kind: "text",
  },
  {
    fg: "text-faint",
    bg: "ink",
    needs: 4.5,
    what: "The same labels over the window itself",
    kind: "text",
  },
  {
    fg: "text-faint",
    bg: "surface-2",
    needs: 4.5,
    what: "The workspace path in the rail, while the pointer is on it",
    kind: "text",
  },
  {
    fg: "accent",
    bg: "surface-1",
    needs: 4.5,
    what: "An accented icon, link or count",
    kind: "text",
  },
  {
    fg: "on-accent",
    bg: "accent",
    needs: 4.5,
    what: "The label on the New conversation button",
    kind: "text",
  },
  { fg: "ok", bg: "surface-1", needs: 4.5, what: "A step that succeeded", kind: "text" },
  {
    fg: "warn",
    bg: "surface-1",
    needs: 4.5,
    what: "A warning, and the badge on an MCP server that is not answering",
    kind: "text",
  },
  {
    fg: "danger",
    bg: "surface-1",
    needs: 4.5,
    what: "The confirmation before a delete",
    kind: "text",
  },
  {
    /*
     * 1.3, and the number is measured rather than chosen: the two palettes
     * this app ships sit at 1.34 and 1.39, because the design has one hairline
     * weight and leans on the surface ladder to do the separating. A floor
     * above that would fail the app's own themes, which is a checker telling
     * everybody their palette is broken because its author preferred a
     * heavier rule.
     */
    fg: "line",
    bg: "surface-1",
    needs: 1.3,
    what: "The hairline between two panels",
    kind: "edge",
  },
];

/** One pair, scored. */
export type Result = Pair & {
  /** What it measured, or null when one of the two is not a hex colour. */
  ratio: number | null;
  ok: boolean;
};

/**
 * Scores a resolved palette.
 *
 * "Resolved" is load-bearing: a theme may name four colours and inherit the
 * other ten, and the pairs that matter are almost always one of each. Checking
 * only what the theme states would pass a violet accent that was never
 * measured against the ink it will actually sit on.
 *
 * A pair naming a colour that is missing entirely scores `null` and passes,
 * rather than failing. That case is a caller that has not resolved its palette
 * — a bug in this app, not in somebody's theme — and answering it with a red
 * warning on somebody's screen would be blaming them for it.
 */
export function check(palette: Record<string, string>): Result[] {
  return PAIRS.map((pair) => {
    const measured = ratio(palette[pair.fg] ?? "", palette[pair.bg] ?? "");
    return { ...pair, ratio: measured, ok: measured === null || measured >= pair.needs };
  });
}

/** Just the pairs that failed, worst first — what a warning should list. */
export function failures(palette: Record<string, string>): Result[] {
  return check(palette)
    .filter((r) => !r.ok)
    .sort((a, b) => (a.ratio ?? 0) - (b.ratio ?? 0));
}
