import { describe, expect, it } from "vitest";

import { place } from "./Tooltip";

/*
 * The placing arithmetic, which is the half of the tooltip a browser is not
 * needed for — and the half that is wrong in ways nobody notices until a
 * control happens to sit at the edge of the window.
 *
 * `place` takes the viewport as an argument for exactly this reason: jsdom
 * measures every element as zero and reports an `innerWidth` that has nothing
 * to do with the app, so a test that let it read the window would be asserting
 * that everything fits everywhere. The behaviour of the layer around it —
 * when it opens, what closes it, whether it reaches a disabled control — is
 * checked against a real browser, because every one of those questions is
 * about events and hit-testing that jsdom does not have.
 */

/** A 100×30 control, wherever it is asked for. */
const at = (left: number, top: number) => ({ left, top, width: 100, height: 30 });
const TIP = { width: 120, height: 40 };
const VIEW = { width: 1000, height: 800 };

describe("placing a tip", () => {
  it("centres it above the control it belongs to", () => {
    const { top, left, side } = place(at(400, 400), TIP, "top", VIEW);
    expect(side).toBe("top");
    // 8px of gap above the control, and the two centre lines agreeing.
    expect(top).toBe(400 - 40 - 8);
    expect(left + TIP.width / 2).toBe(400 + 100 / 2);
  });

  it("flips under a control too near the top to sit above one", () => {
    /*
     * The topbar's controls are the case: at 12px down, a tip above them would
     * be drawn off the window, which the browser renders as nothing at all
     * rather than as a clipped box. Nobody reports an invisible tooltip.
     */
    const { side, top } = place(at(400, 12), TIP, "top", VIEW);
    expect(side).toBe("bottom");
    expect(top).toBe(12 + 30 + 8);
  });

  it("flips a side-anchored tip that would run off the left edge", () => {
    // The rail's rows ask for `right`; a drawer's would ask for `left` and be
    // the same problem mirrored.
    const { side } = place(at(4, 400), TIP, "left", VIEW);
    expect(side).toBe("right");
  });

  it("keeps a tip on a control at the corner inside the window", () => {
    /*
     * Centring alone puts half of this one past the right edge. The clamp is
     * what the margin is for, and it must not also move the tip off the axis
     * the side chose — a bottom tip nudged left is still under its control,
     * and one nudged *up* would be on top of it.
     */
    const { top, left } = place(at(960, 12), TIP, "top", VIEW);
    expect(left + TIP.width).toBeLessThanOrEqual(VIEW.width - 8);
    expect(left).toBeGreaterThanOrEqual(8);
    expect(top).toBe(12 + 30 + 8);
  });

  it("keeps the readable end of a tip wider than the window it is in", () => {
    /*
     * A 34ch tip in a narrow window: there is no position that fits, and the
     * clamp's two bounds cross. Pinning to the near edge keeps the sentence
     * readable from its start, which is the half that says what the control is.
     */
    const narrow = { width: 100, height: 800 };
    const { left } = place(at(10, 400), { width: 300, height: 40 }, "top", narrow);
    expect(left).toBe(8);
  });
});
