// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { readFileSync } from "node:fs";

import {
  applyCachedTheme,
  applyTheme,
  cachedTheme,
  COLOR_GROUPS,
  COLOR_TOKENS,
  FONT_TOKENS,
  resolve,
  previewTheme,
  resolveWith,
  slug,
  tokensFor,
  watchSystemTheme,
} from "./theme";
import type { CustomTheme } from "./api";

/** A custom theme with only the fields a given test cares about. */
const brand = (over: Partial<CustomTheme> = {}): CustomTheme => ({
  id: "midnight",
  name: "Midnight",
  path: "",
  scope: "global",
  dark: {},
  light: {},
  fonts: { display: null, body: null, mono: null },
  wordmark: null,
  logo: null,
  shape: { radius: null, gutter: null, "rail-gutter": null },
  modes: "both",
  ...over,
});

/** What is actually set on <html> right now. */
const painted = (token: string) =>
  document.documentElement.style.getPropertyValue(token);

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
  document.documentElement.removeAttribute("style");
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

describe("the token table", () => {
  /*
   * The colour names a theme file uses, and the tokens they set, are declared
   * twice: once in `crates/taurus-host/src/theme.rs`, which validates what
   * people write, and once here, which applies it. Rust is the authority — it
   * refuses to save a name this app has no token for — so a name that exists
   * on only one side is a name that either cannot be saved or cannot be
   * painted, and neither failure says anything on screen.
   *
   * Read out of the Rust source rather than restated, for the reason
   * `styles.test.ts` reads `InkKind` out of `ink.ts`: a list that is copied
   * cannot be half-updated, and a list that is parsed can.
   */
  // From the working directory rather than from `import.meta.url`: this file
  // runs under jsdom, where that is an `http://localhost` URL and not a path.
  const rust = readFileSync("crates/taurus-host/src/theme.rs", "utf8");

  /** The `(name, token)` pairs out of one `const` table in `theme.rs`. */
  const table = (name: string): [string, string][] => {
    const body = rust.slice(rust.indexOf(`pub const ${name}:`));
    return [...body.slice(0, body.indexOf("];")).matchAll(/\("([^"]+)",\s*"([^"]+)"\)/g)].map(
      ([, key, token]) => [key, token],
    );
  };

  it("finds the tables it is meant to be checking", () => {
    // The guard on the guard: a regex that matched nothing would make both
    // tests below pass forever.
    expect(table("COLORS")).toHaveLength(14);
    expect(table("FONTS")).toHaveLength(3);
  });

  it("maps every colour to the same token Rust does", () => {
    expect(Object.fromEntries(table("COLORS"))).toEqual(COLOR_TOKENS);
  });

  it("maps every typeface to the same token Rust does", () => {
    expect(Object.fromEntries(table("FONTS"))).toEqual(FONT_TOKENS);
  });

  it("gives the editor a field for every colour, exactly once", () => {
    // A colour missing from the groups is one Rust will happily store and no
    // screen can reach; a colour in two groups is two controls writing to one
    // value, where whichever was touched last silently wins.
    const grouped = COLOR_GROUPS.flatMap((g) => g.keys);
    expect([...grouped].sort()).toEqual(Object.keys(COLOR_TOKENS).sort());
  });

  it("agrees with Rust about what a name slugs to", () => {
    // The editor shows the path a theme will be written to before it is
    // written, and Rust refuses any id that does not survive its own slug. Two
    // implementations that disagreed would show one path and write another.
    const cases = [...rust.matchAll(/assert_eq!\(slug\("([^"]*)"\),\s*"([^"]*)"\)/g)];
    expect(cases.length).toBeGreaterThan(3);
    for (const [, name, expected] of cases) expect(slug(name)).toBe(expected);
  });
});

describe("turning a theme into custom properties", () => {
  it("sets only what the theme states", () => {
    // The common case is wanting a different accent. Everything else has to
    // fall through to the stylesheet, or a four-line theme would freeze the
    // other thirteen colours at whatever they were the day it was written.
    const tokens = tokensFor(brand({ dark: { accent: "#b48cff" } }), "dark");
    expect(tokens).toEqual({ "--lk-cyan": "#b48cff" });
  });

  it("takes the palette for the mode being painted", () => {
    const theme = brand({ dark: { ink: "#000" }, light: { ink: "#fff" } });
    expect(tokensFor(theme, "light")["--lk-ink"]).toBe("#fff");
  });

  it("ignores a colour name the app has no token for", () => {
    // Rust drops these on the way in, so this is the second line rather than
    // the first — but a theme file is hand-editable and this is what runs.
    expect(tokensFor(brand({ dark: { chartreuse: "#7fff00" } }), "dark")).toEqual({});
  });

  it("quotes a typeface, because a two-word family is otherwise two families", () => {
    const tokens = tokensFor(brand({ fonts: { display: "IBM Plex Sans", body: null, mono: null } }), "dark");
    expect(tokens["--lk-display"]).toBe('"IBM Plex Sans"');
  });

  it("does not quote one that is quoted already", () => {
    const tokens = tokensFor(brand({ fonts: { display: '"Already"', body: null, mono: null } }), "dark");
    expect(tokens["--lk-display"]).toBe('"Already"');
  });

  it("scales the whole radius ladder from one multiplier", () => {
    const tokens = tokensFor(brand({ shape: { radius: 0, gutter: null, "rail-gutter": null } }), "dark");
    expect(tokens["--r-sm"]).toBe("0px");
    expect(tokens["--r-xl"]).toBe("0px");
  });

  it("rounds a scaled radius to whole pixels", () => {
    // A 9.6px corner beside a 9px one reads as a mistake rather than a choice.
    const tokens = tokensFor(brand({ shape: { radius: 1.6, gutter: null, "rail-gutter": null } }), "dark");
    expect(tokens["--r-sm"]).toBe("10px");
  });
});

describe("applying a custom theme", () => {
  it("puts the theme's tokens on the document", () => {
    systemSays("dark");
    applyTheme("dark", brand({ dark: { ink: "#010203" } }));
    expect(painted("--lk-ink")).toBe("#010203");
  });

  it("clears the last theme's tokens rather than layering onto them", () => {
    // The bug this exists for: a version that only ever wrote left the old
    // theme's ink standing after a switch to one that does not name it, with
    // nothing on screen to say where the colour had come from.
    systemSays("dark");
    applyTheme("dark", brand({ dark: { ink: "#010203" } }));
    applyTheme("dark", brand({ dark: { accent: "#b48cff" } }));
    expect(painted("--lk-ink")).toBe("");
    expect(painted("--lk-cyan")).toBe("#b48cff");
  });

  it("gives the stylesheet back when the theme is removed", () => {
    systemSays("dark");
    applyTheme("dark", brand({ dark: { ink: "#010203" } }));
    applyTheme("dark", null);
    expect(painted("--lk-ink")).toBe("");
  });

  it("lets a theme that paints one mode overrule the preference", () => {
    // Honouring "light" against a theme with no light palette would paint half
    // a palette: the stylesheet's light surfaces under the theme's dark text.
    systemSays("light");
    const dark = brand({ modes: "dark_only", dark: { ink: "#010203" } });
    expect(resolveWith("light", dark)).toBe("dark");
    applyTheme("light", dark);
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("does not follow the system for a theme that paints one mode", () => {
    // There is nothing to follow it to, and a listener that repainted anyway
    // would be a listener that can only ever repaint the same answer.
    const system = systemSays("dark");
    const stop = watchSystemTheme("system", brand({ modes: "light_only" }));
    expect(system.listenerCount).toBe(0);
    stop();
  });

  it("keeps the theme when the system flips at dusk", () => {
    // The bug this exists for: the watcher took only the preference and
    // repainted with `applyTheme("system")`, which cleared every token the
    // theme had set — so a branded window reverted at sunset and stayed that
    // way until something else wrote settings.
    const system = systemSays("light");
    const theme = brand({ dark: { ink: "#010203" }, light: { ink: "#fefefe" } });
    applyTheme("system", theme);
    const stop = watchSystemTheme("system", theme);

    system.flipTo("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(painted("--lk-ink")).toBe("#010203");
    stop();
  });
});

describe("the frame before settings arrive", () => {
  it("replays the tokens that were showing last time", () => {
    // A branded window that opened in the shipped palette and turned violet a
    // round trip later moves a whole palette, where the preference only ever
    // moves one of two.
    systemSays("dark");
    applyTheme("dark", brand({ dark: { ink: "#010203" } }));
    document.documentElement.removeAttribute("style");

    applyCachedTheme();
    expect(painted("--lk-ink")).toBe("#010203");
  });

  it("does not replay a property a theme is not allowed to set", () => {
    // This is the one path where a value reaches a style attribute without
    // having been through Rust's validation this run, and local storage is
    // editable by anything else running in the webview.
    systemSays("dark");
    localStorage.setItem("taurus.theme", "dark");
    localStorage.setItem(
      "taurus.themeTokens",
      JSON.stringify({ "--lk-ink": "#010203", "--not-a-token": "red" }),
    );
    applyCachedTheme();
    expect(painted("--lk-ink")).toBe("#010203");
    expect(painted("--not-a-token")).toBe("");
  });

  it("paints the stylesheet's own palette when there is nothing cached", () => {
    systemSays("dark");
    applyCachedTheme();
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(painted("--lk-ink")).toBe("");
  });

  it("does not cache a draft the editor was only previewing", () => {
    // `previewTheme` is what the editor calls while somebody drags a picker,
    // and the difference from `applyTheme` is exactly this. A draft that was
    // cached would be replayed on the next cold start as though it had been
    // saved, so cancelling an edit would still change how the app looks
    // tomorrow.
    systemSays("dark");
    applyTheme("dark", brand({ dark: { ink: "#010203" } }));

    previewTheme("dark", brand({ dark: { ink: "#ff0000" } }));
    expect(painted("--lk-ink")).toBe("#ff0000");
    expect(JSON.parse(localStorage.getItem("taurus.themeTokens")!)).toEqual({
      "--lk-ink": "#010203",
    });

    // And backing out puts the saved one back, which is what the editor does
    // on its way off the screen.
    previewTheme("dark", brand({ dark: { ink: "#010203" } }));
    expect(painted("--lk-ink")).toBe("#010203");
  });
});
