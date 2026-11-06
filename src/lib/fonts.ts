/**
 * Whether a font family is installed on this machine.
 *
 * A theme names families it can't bring with it (the window loads no remote
 * stylesheet), and one that isn't installed falls back to the app's own stack
 * without a word. This is how the app finds out, so it can say so.
 *
 * `document.fonts.check` doesn't answer it: it reports whether a family needs
 * loading, and a system family that doesn't exist needs none, so it says
 * `true`. What does answer it is drawing. Text set in the family, with each
 * generic family behind it, is measured against the generic family alone. A
 * family that's there changes the width against at least one of them. One that
 * isn't falls through to the generic every time and changes nothing. Three
 * generics, because an installed family can be the very face one of them maps
 * to (Times is often `serif`), and it can't be all three.
 */

const GENERICS = ["monospace", "sans-serif", "serif"] as const;

/** Wide and narrow letters both, so two faces of similar width still differ. */
const SAMPLE = "mmmmmmmmmmlli1WQ@#&";

const known = new Map<string, boolean | null>();

/**
 * `true` if `family` is installed, `false` if it isn't, and `null` where this
 * can't be told (no canvas to measure with). A caller says nothing on `null`.
 *
 * `family` is one family, as a theme names it, with or without quotes.
 * Remembered per name for the life of the window: a font installed while the
 * app is open shows up after a reload, and nobody installs fonts often enough
 * for that to matter.
 */
export function installed(family: string): boolean | null {
  const name = family.trim().replace(/^(["'])(.*)\1$/, "$2").trim();
  if (name === "") return null;
  const cached = known.get(name);
  if (cached !== undefined) return cached;
  const answer = measure(name);
  known.set(name, answer);
  return answer;
}

function measure(name: string): boolean | null {
  let context: CanvasRenderingContext2D | null = null;
  try {
    context = document.createElement("canvas").getContext("2d");
  } catch {
    return null;
  }
  if (!context) return null;
  const width = (font: string) => {
    context.font = `72px ${font}`;
    return context.measureText(SAMPLE).width;
  };
  const quoted = `"${name.replace(/["\\]/g, "\\$&")}"`;
  return GENERICS.some((generic) => width(`${quoted}, ${generic}`) !== width(generic));
}

/** Forgets every answer. For tests. */
export function forgetFonts(): void {
  known.clear();
}
