import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import {
  CHANGES_WIDTH,
  RAIL_WIDTH,
  ResizeHandle,
  TERMINAL_HEIGHT,
  clamp,
  sizedBy,
  type Resizable,
} from "./ResizeHandle";

const grabbed = { at: 400, size: 236 };

describe("dragging an edge", () => {
  it("moves the rail's edge with the pointer", () => {
    expect(sizedBy(grabbed, 460, 1, 180, 360)).toBe(296);
    expect(sizedBy(grabbed, 360, 1, 180, 360)).toBe(196);
  });

  it("widens a right-anchored drawer when the pointer goes the other way", () => {
    // The Changes panel is docked at the right of the window, so the room
    // it can take is all to its left. Sharing the rail's sign would shrink it
    // on exactly the gesture that should open it up.
    const drawer = { at: 900, size: 440 };
    expect(sizedBy(drawer, 800, -1, 320, 620)).toBe(540);
    expect(sizedBy(drawer, 1000, -1, 320, 620)).toBe(340);
  });

  it("stops at the limits rather than letting a pane be dragged shut", () => {
    expect(sizedBy(grabbed, -5000, 1, 180, 360)).toBe(180);
    expect(sizedBy(grabbed, 5000, 1, 180, 360)).toBe(360);
  });

  it("tracks the pointer again after it has overshot and come back", () => {
    // Sizing from the pane's current width instead of the width it was grabbed
    // at loses every pixel spent past the limit, and the edge only starts
    // moving again once the pointer has paid them all back.
    expect(sizedBy(grabbed, 5000, 1, 180, 360)).toBe(360);
    expect(sizedBy(grabbed, 420, 1, 180, 360)).toBe(256);
  });
});

describe("the handle", () => {
  const pane = (over: Partial<Resizable> = {}): Resizable => ({
    size: 236,
    dragging: false,
    min: 180,
    max: 360,
    start: () => {},
    nudge: () => {},
    ...over,
  });

  it("reports its position to a screen reader as the separator it is", () => {
    const html = renderToStaticMarkup(
      <ResizeHandle pane={pane()} label="Rail width" />,
    );
    expect(html).toContain('role="separator"');
    expect(html).toContain('aria-label="Rail width"');
    expect(html).toContain('aria-valuenow="236"');
    expect(html).toContain('aria-valuemin="180"');
    expect(html).toContain('aria-valuemax="360"');
  });

  it("can be reached by keyboard, so the pane is sizable without a mouse", () => {
    const html = renderToStaticMarkup(
      <ResizeHandle pane={pane()} label="Rail width" />,
    );
    expect(html).toContain('tabindex="0"');
  });

  it("marks itself while the drag is live", () => {
    const html = renderToStaticMarkup(
      <ResizeHandle pane={pane({ dragging: true })} label="Rail width" />,
    );
    expect(html).toContain("rail-handle dragging");
  });

  it("reports a dock's separator as the horizontal line it is", () => {
    // The orientation named is the separator's own, not the axis it sizes: the
    // line between the conversation and the dock runs across the window, and a
    // screen reader announcing it as vertical would describe the wrong gesture.
    const html = renderToStaticMarkup(
      <ResizeHandle
        pane={pane({ axis: "y", size: 300, min: 120, max: 900 })}
        label="Terminal height"
      />,
    );
    expect(html).toContain('aria-orientation="horizontal"');
    expect(html).toContain("dock-handle");
    expect(html).toContain('aria-valuenow="300"');
  });
});

describe("the defaults", () => {
  it("opens each pane at a size inside its own limits", () => {
    for (const size of [RAIL_WIDTH, CHANGES_WIDTH, TERMINAL_HEIGHT]) {
      expect(clamp(size.initial, size.min, size.max)).toBe(size.initial);
    }
  });
});
