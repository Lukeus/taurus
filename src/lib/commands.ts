/**
 * The window-wide commands, by the names the macOS menu bar sends them as.
 *
 * `src-tauri/src/menu.rs` lists the same ids. Each reaches `App` one of two
 * ways — the window's keyboard handler, or a menu item — and both go through
 * one function there, so "New Conversation" from the menu and ⌘N from the
 * keyboard are the same code rather than two that agree today.
 */
export type Command =
  | "settings"
  | "new"
  | "switch"
  | "finder"
  | "palette"
  | "sidebar"
  | "terminal"
  | "composer"
  | "docs";

export const COMMANDS: readonly Command[] = [
  "settings",
  "new",
  "switch",
  "finder",
  "palette",
  "sidebar",
  "terminal",
  "composer",
  "docs",
];

/**
 * Whether there is a native menu bar to own some of the keys.
 *
 * Read off the flag `applyChrome` sets on `<html>`, not worked out again from
 * the user agent and `__TAURI_INTERNALS__`. That global is also what any Tauri
 * mock defines — the screenshot build's does — so testing for it made a
 * browser on a Mac believe it had a menu bar, and step aside for ⌘K with
 * nothing there to take it. `applyChrome` runs only from the app's real entry
 * point, on the one platform `menu.rs` builds a menu for, so the flag and the
 * menu are true together or not at all.
 */
export function nativeMenu(): boolean {
  return typeof document !== "undefined" && document.documentElement.dataset.chrome === "overlay";
}

/**
 * Whether the page should leave this key to the menu bar.
 *
 * A menu item with an accelerator and a page handler for the same key would
 * both run, in an order that depends on how WebKit routes key equivalents —
 * and for a toggle (⌘B), running twice is not running at all. So where the
 * menu owns a key the page steps aside, and does not `preventDefault`, which
 * is what guarantees the menu still receives it whichever of the two is asked
 * first. `owned` is per key, not per command: ⌘⇧P opens the palette too, and
 * the menu does not answer to it.
 */
export function leaveToMenu(owned: boolean): boolean {
  return owned && nativeMenu();
}

/** Runs `run` for each menu item the app owns. Returns the unsubscribe. */
export function onMenu(run: (command: Command) => void): () => void {
  if (!nativeMenu()) return () => {};
  let unlisten: (() => void) | undefined;
  let gone = false;
  void import("@tauri-apps/api/event").then(({ listen }) =>
    listen<string>("menu", ({ payload }) => {
      if ((COMMANDS as readonly string[]).includes(payload)) run(payload as Command);
    }).then((stop) => {
      if (gone) stop();
      else unlisten = stop;
    }),
  );
  return () => {
    gone = true;
    unlisten?.();
  };
}

export const DOCS_URL = "https://github.com/Lukeus/taurus#readme";
