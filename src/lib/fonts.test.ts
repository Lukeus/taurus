// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

import { forgetFonts, installed } from "./fonts";
import { fakeFonts } from "./fonts.fake";

afterEach(() => {
  forgetFonts();
  vi.restoreAllMocks();
});

describe("whether a font is installed", () => {
  it("tells one that draws from one that falls through", () => {
    fakeFonts(["Fira Code"]);
    expect(installed("Fira Code")).toBe(true);
    expect(installed('"Fira Code"')).toBe(true);
    expect(installed("Comic Mono")).toBe(false);
  });

  it("isn't fooled by a face that is one of the generics", () => {
    // Times is often what `serif` draws, so against serif alone it vanishes.
    fakeFonts(["Times"], { sameAs: { Times: "serif" } });
    expect(installed("Times")).toBe(true);
  });

  it("measures each name once", () => {
    const measure = fakeFonts([]);
    installed("Comic Mono");
    const calls = measure.mock.calls.length;
    installed("Comic Mono");
    expect(measure.mock.calls.length).toBe(calls);
  });

  it("says nothing where there's nothing to measure with, or nothing named", () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
    expect(installed("Fira Code")).toBeNull();
    expect(installed("  ")).toBeNull();
  });
});
