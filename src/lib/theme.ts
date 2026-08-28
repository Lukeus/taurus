/**
 * Which palette the window paints in, and whose colours those are.
 *
 * Two questions, kept apart. `Theme` is light, dark, or a standing instruction
 * to keep asking the OS — which is why resolving it lives here in JS rather
 * than in a media query. The stylesheet only ever sees the answer, since
 * `data-theme` on <html> is always "light" or "dark", so the light palette is
 * written once instead of once per route to it.
 *
 * A [`CustomTheme`] is the second question: whose ink, whose accent, whose
 * typeface and wordmark. It is applied *over* whichever of the two palettes is
 * showing, as inline custom properties on <html> — which beat both the `:root`
 * block and the `[data-theme="light"]` one on specificity without either of
 * them having to know custom themes exist. The whole of what makes that
 * possible is a stylesheet that names its raw values exactly once and speaks in
 * roles everywhere else: fourteen colours here move six thousand lines below.
 *
 * Both settings belong to Rust, in the same `settings.json` as everything else
 * the app remembers. What is kept here is a copy, and only to cover the gap:
 * settings arrive over IPC a tick or two after the window paints, and somebody
 * who chose light on a dark-mode machine — or who branded the app violet —
 * should not watch it open wrong and correct itself every single time.
 */

import type { CustomTheme, Theme } from "./api";

const CACHE_KEY = "taurus.theme";
/** The resolved custom properties from last time, for the frame before IPC. */
const TOKENS_KEY = "taurus.themeTokens";
const DARK_QUERY = "(prefers-color-scheme: dark)";

export type ResolvedTheme = "light" | "dark";

export function systemTheme(): ResolvedTheme {
  return window.matchMedia?.(DARK_QUERY).matches ? "dark" : "light";
}

export function resolve(theme: Theme): ResolvedTheme {
  return theme === "system" ? systemTheme() : theme;
}

/**
 * Which palette is actually painted, once a custom theme has had its say.
 *
 * A theme that fills in only one of the two is a statement — this brand is
 * dark — and honouring the preference anyway would paint half a palette: the
 * light surfaces the theme never gave, under the dark text it did. So the
 * theme wins, and the UI says so rather than leaving a preference that appears
 * to do nothing. See `ThemePicker`.
 */
export function resolveWith(theme: Theme, custom: CustomTheme | null): ResolvedTheme {
  switch (custom?.modes) {
    case "dark_only":
      return "dark";
    case "light_only":
      return "light";
    default:
      return resolve(theme);
  }
}

/** Whether `mode` is one this theme can actually paint. */
export function supports(custom: CustomTheme | null, mode: ResolvedTheme): boolean {
  return custom === null || custom.modes === "both" || custom.modes === `${mode}_only`;
}

/**
 * What a theme file's colour names are called in the stylesheet.
 *
 * The right-hand side is the design system's own vocabulary, which names its
 * accents after the colours they happen to be. That is fine for a system with
 * one palette and absurd in a theme file, where the entire point is that the
 * accent might be violet — so a theme names the *job* and this table is where
 * the two meet. The same table exists in `crates/taurus-host/src/theme.rs`,
 * which is the authority; `theme.test.ts` fails if they drift apart.
 */
export const COLOR_TOKENS: Record<string, string> = {
  ink: "--lk-ink",
  "surface-1": "--lk-surface-1",
  "surface-2": "--lk-surface-2",
  "surface-hover": "--lk-surface-hover",
  line: "--lk-line",
  text: "--lk-text",
  "text-dim": "--lk-text-dim",
  "text-faint": "--lk-text-faint",
  accent: "--lk-cyan",
  "accent-hover": "--lk-cyan-hover",
  "on-accent": "--lk-on-cyan",
  ok: "--lk-mint",
  warn: "--lk-peach",
  danger: "--lk-red",
};

/** The same, for the three typefaces. */
export const FONT_TOKENS: Record<string, string> = {
  display: "--lk-display",
  body: "--lk-body",
  mono: "--lk-mono-face",
};

/**
 * How the colour fields are grouped and ordered in the editor.
 *
 * A flat list of fourteen swatches is a list nobody can hold; four groups of
 * three or four is four decisions. The order within each is the order they
 * stack on screen — the window, then what is raised off it — so picking them
 * top to bottom builds a ladder rather than a set.
 */
export const COLOR_GROUPS: readonly { label: string; hint: string; keys: string[] }[] = [
  {
    label: "Surfaces",
    hint: "The window, the panels raised off it, and the one hairline between them.",
    keys: ["ink", "surface-1", "surface-2", "surface-hover", "line"],
  },
  {
    label: "Text",
    hint: "Three weights. The faint one carries the 10px labels, so it needs the most contrast, not the least.",
    keys: ["text", "text-dim", "text-faint"],
  },
  {
    label: "Accent",
    hint: "The lead colour, its hover, and what stays legible on top of it.",
    keys: ["accent", "accent-hover", "on-accent"],
  },
  {
    label: "Signals",
    hint: "Spent sparingly. A surface that is 5% accent reads as signal; one that is 30% reads as decoration.",
    keys: ["ok", "warn", "danger"],
  },
];

/**
 * The corner-radius ladder as the stylesheet ships it, which `shape.radius`
 * scales. Named here because a multiplier needs something to multiply.
 */
const RADII: Record<string, number> = {
  "--r-sm": 6,
  "--r": 9,
  "--r-md": 10,
  "--r-lg": 12,
  "--r-xl": 14,
};

/** Every custom property a theme is allowed to set. */
function everyToken(): string[] {
  return [
    ...Object.values(COLOR_TOKENS),
    ...Object.values(FONT_TOKENS),
    ...Object.keys(RADII),
    "--gutter",
    "--gutter-rail",
  ];
}

/**
 * The custom properties a theme sets, for one of the two modes.
 *
 * Only what the theme states. Everything it leaves out falls through to the
 * stylesheet, which is what lets the common case — a different accent — be
 * four lines rather than a transcription of the palette, and what keeps a
 * theme written today working after the app adds a token tomorrow.
 */
export function tokensFor(
  custom: CustomTheme | null,
  mode: ResolvedTheme,
): Record<string, string> {
  const out: Record<string, string> = {};
  if (!custom) return out;

  for (const [name, value] of Object.entries(mode === "dark" ? custom.dark : custom.light)) {
    const token = COLOR_TOKENS[name];
    if (token) out[token] = value;
  }

  for (const [name, token] of Object.entries(FONT_TOKENS)) {
    const family = custom.fonts[name as keyof CustomTheme["fonts"]];
    // Quoted, because the stylesheet interpolates this straight into a
    // `font-family` list where a two-word family name is two families.
    if (family) out[token] = /^["']/.test(family) ? family : `"${family}"`;
  }

  const { radius, gutter } = custom.shape;
  if (radius !== null && radius !== undefined) {
    // Rounded to whole pixels: a 9.6px corner and a 9px one beside it are the
    // kind of difference that reads as a mistake rather than as a choice.
    for (const [token, base] of Object.entries(RADII)) {
      out[token] = `${Math.round(base * radius)}px`;
    }
  }
  if (gutter !== null && gutter !== undefined) out["--gutter"] = `${gutter}px`;
  const rail = custom.shape["rail-gutter"];
  if (rail !== null && rail !== undefined) out["--gutter-rail"] = `${rail}px`;

  return out;
}

/**
 * Writes a token set onto <html>, and clears whatever the last one set.
 *
 * Every token is removed before the new set is applied rather than diffed
 * against it. The set is twenty-two properties, so the diff would buy nothing
 * measurable — and the bug it avoids is the one that matters: switching from a
 * theme that sets `--gutter` to one that does not has to give the stylesheet's
 * gutter back, and a version of this that only ever wrote would leave the old
 * one standing with nothing on screen to say where it came from.
 */
function paint(tokens: Record<string, string>): void {
  const style = document.documentElement.style;
  for (const token of everyToken()) style.removeProperty(token);
  for (const [token, value] of Object.entries(tokens)) style.setProperty(token, value);
}

/**
 * Paints `theme` and remembers it for the next cold start.
 *
 * The preference is cached, not the resolution: caching "light" for someone who
 * asked to follow a system that happened to be light at the time would freeze
 * them there through every sunset after. The custom theme's *tokens* are
 * cached, though, and that asymmetry is deliberate — they are the resolution
 * already, they cannot go stale by the clock, and re-deriving them needs a
 * theme file that has not arrived over IPC yet at the moment they are wanted.
 *
 * The logo is not cached. It is the one part that is large, and a mark that
 * appears a tick into the launch is a far smaller thing than a window that
 * opens in the wrong palette.
 */
export function applyTheme(theme: Theme, custom: CustomTheme | null = null): void {
  const mode = resolveWith(theme, custom);
  document.documentElement.dataset.theme = mode;
  const tokens = tokensFor(custom, mode);
  paint(tokens);
  try {
    localStorage.setItem(CACHE_KEY, theme);
    localStorage.setItem(TOKENS_KEY, JSON.stringify(tokens));
  } catch {
    // A webview with storage disabled costs a flash on the next start, which
    // is not worth failing a theme change over.
  }
}

/**
 * Repaints the frame before settings arrive, from what was showing last time.
 *
 * Called once at startup. Without it, a branded app opens in the shipped cyan
 * and corrects itself a round trip later — which is the same flash the theme
 * preference has always been cached to avoid, and more visible, because a
 * whole palette moves rather than one of two.
 */
export function applyCachedTheme(): void {
  const theme = cachedTheme();
  document.documentElement.dataset.theme = resolve(theme);
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(TOKENS_KEY) ?? "{}");
    if (stored && typeof stored === "object" && !Array.isArray(stored)) {
      // Filtered against the allowed set rather than written through. This is
      // the one path where a value reaches a style attribute without having
      // come from Rust's validation this run, and local storage is editable.
      const allowed = new Set(everyToken());
      const tokens: Record<string, string> = {};
      for (const [token, value] of Object.entries(stored as Record<string, unknown>)) {
        if (allowed.has(token) && typeof value === "string") tokens[token] = value;
      }
      paint(tokens);
    }
  } catch {
    // Unreadable or not JSON. The stylesheet's own palette is the answer.
  }
}

/**
 * Paints a theme without remembering it.
 *
 * What the editor shows while somebody is dragging a colour picker. The
 * difference from [`applyTheme`] is the cache, and it is the whole reason this
 * exists separately: a draft that was cached would be replayed on the next
 * cold start as though it had been saved, so cancelling an edit would still
 * change what the app looks like tomorrow.
 */
export function previewTheme(mode: ResolvedTheme, custom: CustomTheme | null): void {
  document.documentElement.dataset.theme = mode;
  paint(tokensFor(custom, mode));
}

/**
 * The file name a theme called `name` is stored under.
 *
 * A copy of `slug` in `crates/taurus-host/src/theme.rs`, which is the
 * authority — Rust refuses to write a file under any id that does not survive
 * it, so a frontend that guessed differently would show one path in the editor
 * and write to another. `theme.test.ts` checks the two agree, character for
 * character, on the cases that distinguish them.
 */
export function slug(name: string): string {
  let out = "";
  for (const c of name.trim().toLowerCase()) {
    if (/[a-z0-9]/.test(c)) out += c;
    else if (!out.endsWith("-")) out += "-";
  }
  return out.replace(/^-+|-+$/g, "") || "theme";
}

/** The value a token resolves to right now, whoever set it. */
export function currentToken(token: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(token).trim();
}

/**
 * The palette as it is actually painting — the theme's own values over the
 * stylesheet's, read back off the document.
 *
 * What the contrast check needs, and what "duplicate this into a new theme"
 * starts from. Reading it off the document rather than restating the shipped
 * palette in TypeScript is what keeps this from becoming a third copy of it
 * that drifts: `styles.css` stays the only place those fourteen values live.
 */
export function livePalette(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [name, token] of Object.entries(COLOR_TOKENS)) out[name] = currentToken(token);
  return out;
}

/** What was showing last time, for the frame before settings arrive. */
export function cachedTheme(): Theme {
  try {
    const stored = localStorage.getItem(CACHE_KEY);
    if (stored === "light" || stored === "dark" || stored === "system") {
      return stored;
    }
  } catch {
    // Same as above: fall through to the system's answer.
  }
  return "system";
}

/**
 * Repaints when the OS flips, while the preference is to follow it.
 *
 * Returns the unsubscribe, so a caller that switches to an explicit theme stops
 * listening rather than leaving a listener that would repaint over the choice.
 *
 * The custom theme has to come back through here with it. An earlier version
 * took only the preference and repainted with `applyTheme("system")`, which
 * cleared every token a custom theme had set — so a branded window reverted to
 * the shipped palette at dusk, and stayed there until something else wrote
 * settings.
 */
export function watchSystemTheme(theme: Theme, custom: CustomTheme | null = null): () => void {
  // Nothing to follow: either the preference is explicit, or the theme paints
  // one mode and the system's opinion cannot change the answer.
  if (theme !== "system" || !window.matchMedia) return () => {};
  if (custom && custom.modes !== "both") return () => {};
  const query = window.matchMedia(DARK_QUERY);
  const onChange = () => applyTheme("system", custom);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}
