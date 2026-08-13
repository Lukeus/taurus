// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyTheme, cachedTheme, resolve, watchSystemTheme } from "./theme";

/** jsdom has no `matchMedia`, so every test states what the OS is saying. */
const systemSays = (scheme: "light" | "dark") => {
  const listeners = new Set<() => void>();
  const query = {
    matches: scheme === "dark",
    addEventListener: (_: string, fn: () => void) => void listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => void listeners.delete(fn),
  };
  window.matchMedia = vi.fn(() => query) as unknown as typeof window.matchMedia;
  return {
    query,
    flipTo(next: "light" | "dark") {
      query.matches = next === "dark";
      listeners.forEach((fn) => fn());
    },
    get listenerCount() {
      return listeners.size;
    },
  };
};

beforeEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.theme;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("resolving a preference into a palette", () => {
  it("asks the system when the preference is to follow it", () => {
    systemSays("dark");
    expect(resolve("system")).toBe("dark");
  });

  it("ignores the system when the user has picked a side", () => {
    systemSays("dark");
    expect(resolve("light")).toBe("light");
  });
});

describe("applying a theme", () => {
  it("puts the resolved palette on the document, never the preference", () => {
    // What the stylesheet reads is `light` or `dark` and nothing else — a
    // `data-theme="system"` would match no rule and paint the dark defaults at
    // a user who asked for the opposite.
    systemSays("light");
    applyTheme("system");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("caches the preference rather than the palette it resolved to", () => {
    // Caching "light" for someone following a system that was light at noon
    // would hold them there through every sunset after.
    systemSays("light");
    applyTheme("system");
    expect(cachedTheme()).toBe("system");
  });

  it("survives a webview with storage turned off", () => {
    systemSays("dark");
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(() => applyTheme("dark")).not.toThrow();
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("falls back to following the system when nothing was cached", () => {
    localStorage.setItem("taurus.theme", "chartreuse");
    expect(cachedTheme()).toBe("system");
  });
});

describe("following the system while asked to", () => {
  it("repaints when the machine switches at dusk", () => {
    const system = systemSays("light");
    applyTheme("system");
    expect(document.documentElement.dataset.theme).toBe("light");

    system.flipTo("dark");
    expect(document.documentElement.dataset.theme).toBe("light");

    const stop = watchSystemTheme("system");
    system.flipTo("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    stop();
  });

  it("does not listen once a side has been picked", () => {
    // A listener left running under an explicit choice would repaint over it
    // the next time the OS changed — the one thing choosing a side rules out.
    const system = systemSays("light");
    applyTheme("dark");

    const stop = watchSystemTheme("dark");
    expect(system.listenerCount).toBe(0);

    system.flipTo("light");
    expect(document.documentElement.dataset.theme).toBe("dark");
    stop();
  });

  it("leaves nothing behind when it is torn down", () => {
    const system = systemSays("dark");
    const stop = watchSystemTheme("system");
    expect(system.listenerCount).toBe(1);
    stop();
    expect(system.listenerCount).toBe(0);
  });
});
