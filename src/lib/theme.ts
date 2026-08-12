/**
 * Which palette the window paints in.
 *
 * Three preferences, two palettes: `system` is not a third look but a standing
 * instruction to keep asking the OS, which is why resolving it lives here in JS
 * rather than in a media query. The stylesheet only ever sees the answer —
 * `data-theme` on <html> is always "light" or "dark" — so the light palette is
 * written once instead of once per route to it.
 *
 * The setting itself belongs to Rust, in the same `settings.json` as everything
 * else the app remembers. What is kept here is a copy, and only to cover the
 * gap: settings arrive over IPC a tick or two after the window paints, and a
 * user who chose light on a dark-mode machine should not watch the app open
 * dark and correct itself every single time.
 */

import type { Theme } from "./api";

const CACHE_KEY = "taurus.theme";
const DARK_QUERY = "(prefers-color-scheme: dark)";

export type ResolvedTheme = "light" | "dark";

export function systemTheme(): ResolvedTheme {
  return window.matchMedia?.(DARK_QUERY).matches ? "dark" : "light";
}

export function resolve(theme: Theme): ResolvedTheme {
  return theme === "system" ? systemTheme() : theme;
}

/**
 * Paints `theme` and remembers it for the next cold start.
 *
 * The preference is cached, not the resolution: caching "light" for someone who
 * asked to follow a system that happened to be light at the time would freeze
 * them there through every sunset after.
 */
export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = resolve(theme);
  try {
    localStorage.setItem(CACHE_KEY, theme);
  } catch {
    // A webview with storage disabled costs a flash on the next start, which
    // is not worth failing a theme change over.
  }
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
 */
export function watchSystemTheme(theme: Theme): () => void {
  if (theme !== "system" || !window.matchMedia) return () => {};
  const query = window.matchMedia(DARK_QUERY);
  const onChange = () => applyTheme("system");
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}
