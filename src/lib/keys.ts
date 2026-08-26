/**
 * The one thing a shortcut differs by across platforms: which key holds it.
 *
 * Kept here rather than checked at each site, because the two halves — what a
 * row *says* the shortcut is, and what the handler actually listens for — are
 * the pair that goes wrong. A label reading ⌘K beside a handler watching
 * `ctrlKey` is a shortcut that is documented and does not work, and nothing
 * about it is visible in either file on its own.
 */

/**
 * Whether the modifier is Command rather than Control.
 *
 * Read from the user agent once. `navigator.platform` is the older spelling
 * and is deprecated; `userAgentData` is not in every engine this ships to. The
 * string is the one that works in all of them, and being wrong costs a label
 * that says Ctrl on a Mac rather than anything breaking — both keys are
 * checked below.
 */
export const APPLE =
  typeof navigator !== "undefined" && /mac|iphone|ipad/i.test(navigator.userAgent);

/** How the modifier is written on a row. */
export const MOD = APPLE ? "⌘" : "Ctrl";

/** `K` → `⌘K` or `Ctrl+K`. */
export const chord = (key: string): string => `${MOD}${APPLE ? "" : "+"}${key}`;

/**
 * Whether this event is the platform's own modifier plus `key`, and nothing
 * else.
 *
 * The other modifier is refused rather than ignored: ⌃⌘K and ⌘K are different
 * chords, and a handler that fires on both is one that steals a shortcut
 * somebody's window manager has. Alt likewise — on several layouts it is how
 * a character is typed.
 */
export function isChord(e: KeyboardEvent, key: string): boolean {
  if (e.key.toLowerCase() !== key.toLowerCase()) return false;
  if (e.altKey) return false;
  return APPLE ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
}
