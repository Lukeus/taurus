// @vitest-environment jsdom
//
// Mounted, because the whole behaviour is an observer on the document noticing
// something else change it.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { useWindowPalette } from "./windowTheme";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

afterEach(() => {
  document.documentElement.removeAttribute("style");
  delete document.documentElement.dataset.theme;
});

describe("the window's palette", () => {
  it("changes when a theme repaints the same mode, and when the mode flips", async () => {
    // The terminal re-themed on the preference alone. One custom theme swapped
    // for another in the same mode, or the system turning dark under "system",
    // left an open shell in the old colours.
    const seen: string[] = [];
    function Probe() {
      seen.push(useWindowPalette());
      return null;
    }
    document.documentElement.dataset.theme = "dark";
    const host = document.createElement("div");
    const root = createRoot(host);
    act(() => root.render(<Probe />));
    const first = seen.at(-1);

    await act(async () => {
      document.documentElement.style.setProperty("--lk-ink", "#123456");
    });
    const repainted = seen.at(-1);
    expect(repainted, "a token repainted in the same mode went unseen").not.toBe(first);

    await act(async () => {
      document.documentElement.dataset.theme = "light";
    });
    expect(seen.at(-1)).not.toBe(repainted);

    act(() => root.unmount());
  });
});
