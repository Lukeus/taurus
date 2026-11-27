// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import { COMMANDS, leaveToMenu, nativeMenu } from "./commands";

/*
 * The menu bar is built in Rust and run in TypeScript, and the only thing
 * joining the two is a string. An id added to one and not the other is a menu
 * item that does nothing, or a command nothing can send — and neither fails a
 * build. So this reads `menu.rs` and holds the two lists to each other.
 */
// From the repository root: under jsdom `import.meta.url` is not a file URL.
const rust = readFileSync(resolve(process.cwd(), "src-tauri/src/menu.rs"), "utf8");

describe("the menu bar's commands", () => {
  it("are the same ids on both sides", () => {
    const tuples = [...rust.matchAll(/\("([a-z]+)", "[^"]+", (?:Some|None)/g)].map(([, id]) => id);
    const texts = [...rust.matchAll(/\.text\("([a-z]+)"/g)].map(([, id]) => id);
    expect(tuples.length).toBeGreaterThan(5);
    expect(new Set([...tuples, ...texts])).toEqual(new Set(COMMANDS));
  });

  it("leaves every key to the page where there is no menu bar", () => {
    // jsdom is a browser with no Tauri in it, like the screenshot build and
    // like Windows and Linux as far as the menu is concerned.
    expect(nativeMenu()).toBe(false);
    expect(leaveToMenu(true)).toBe(false);
  });

  it("steps aside for the menu only where the app's frame says there is one", () => {
    // A Tauri mock defines `__TAURI_INTERNALS__` too; that alone must not
    // make a browser believe it has a menu bar.
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    expect(leaveToMenu(true)).toBe(false);
    document.documentElement.dataset.chrome = "overlay";
    expect(leaveToMenu(true)).toBe(true);
    expect(leaveToMenu(false)).toBe(false);
    delete document.documentElement.dataset.chrome;
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });
});
