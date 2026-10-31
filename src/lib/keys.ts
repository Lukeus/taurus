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
 * The terminal's key, which is Control on every platform, Mac included: ⌘`
 * is the system's own "next window". So it can't go through `chord`, and it
 * was spelled three ways in three places before it lived here.
 */
export const TERMINAL_SHORTCUT = APPLE ? "⌃`" : "Ctrl+`";

/** Completion in the query box: Control and Space everywhere. */
export const COMPLETE_SHORTCUT = APPLE ? "⌃Space" : "Ctrl+Space";

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

/**
 * Whether this key belongs to an input method rather than to the app.
 *
 * Enter is how a Japanese or Chinese writer accepts a candidate, so a box that
 * sends on Enter has to let that one through. Both checks are needed: WebKit,
 * the macOS webview, can end the composition before the keydown that ended it,
 * so `isComposing` is already false there and only `keyCode` 229 still says so.
 */
export function isComposing(
  e:
    | { isComposing: boolean; keyCode: number }
    | { nativeEvent: { isComposing: boolean; keyCode: number } },
): boolean {
  const key = "nativeEvent" in e ? e.nativeEvent : e;
  return key.isComposing || key.keyCode === 229;
}
