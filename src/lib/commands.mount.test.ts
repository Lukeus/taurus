// @vitest-environment jsdom
import { describe, expect, it } from "vitest";

import { leaveToMenu, nativeMenu } from "./commands";

describe("who answers a window shortcut", () => {
  it("is the page, wherever there is no menu bar", () => {
    // jsdom is a browser with no Tauri in it, like the screenshot build and
    // like Windows and Linux as far as the menu is concerned.
    expect(nativeMenu()).toBe(false);
    expect(leaveToMenu(true)).toBe(false);
  });

  it("is the menu bar only where the app's frame says there is one", () => {
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
