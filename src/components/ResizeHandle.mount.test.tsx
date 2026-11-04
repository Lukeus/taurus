// @vitest-environment jsdom
//
// Widening a side pane to the room it has and putting it back. What's checked
// is the arithmetic against a laid-out column, and that the edge moving forgets
// the width there was to go back to.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { roomFor, useResizableWidth, type Resizable } from "./ResizeHandle";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** A slot in a column, with the conversation's floor set where `.app` sets it. */
function laidOut(columnWidth: number, padding = 0) {
  const column = document.createElement("main");
  column.style.paddingLeft = `${padding}px`;
  column.style.paddingRight = `${padding}px`;
  Object.defineProperty(column, "clientWidth", { value: columnWidth + padding * 2 });
  const slot = document.createElement("div");
  slot.style.setProperty("--center-min-w", "320px");
  column.append(slot);
  return slot;
}

describe("the room a widened pane takes", () => {
  it("is the column less the conversation's floor", () => {
    expect(roomFor(laidOut(1044), "x")).toBe(724);
  });

  it("does not count the column's padding as room", () => {
    expect(roomFor(laidOut(1044, 12), "x")).toBe(724);
  });

  it("is nothing for a pane that is not laid out, or is sized by height", () => {
    expect(roomFor(null, "x")).toBeNull();
    expect(roomFor(document.createElement("div"), "x")).toBeNull();
    expect(roomFor(laidOut(1044), "y")).toBeNull();
  });
});

describe("widening a pane", () => {
  let root: Root;
  let host: HTMLElement;
  let pane: Resizable;
  let slot: HTMLDivElement;

  /** The hook in a column 1044px wide, its slot drawn at whatever it asks for. */
  function Harness() {
    pane = useResizableWidth({ storageKey: "test.fill", initial: 560, min: 380, max: 4096, grow: -1 });
    return (
      <main
        ref={(el) => {
          if (el) Object.defineProperty(el, "clientWidth", { value: 1044, configurable: true });
        }}
      >
        <div
          ref={(el) => {
            if (!el) return;
            slot = el;
            pane.slot!.current = el;
            el.style.setProperty("--center-min-w", "320px");
            el.getBoundingClientRect = () => ({ width: pane.size, height: 600 }) as DOMRect;
          }}
        />
      </main>
    );
  }

  beforeEach(() => {
    localStorage.clear();
    host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
    act(() => root.render(<Harness />));
  });

  afterEach(() => {
    act(() => root.unmount());
    host.remove();
  });

  it("takes all the room the conversation leaves, then goes back", () => {
    expect(slot).toBeTruthy();
    act(() => pane.fill());
    expect(pane.size).toBe(724);
    expect(pane.filled).toBe(true);
    expect(pane.easing).toBe(true);

    act(() => pane.fill());
    expect(pane.size).toBe(560);
    expect(pane.filled).toBe(false);
  });

  it("forgets the width to go back to once the edge is moved by hand", () => {
    act(() => pane.fill());
    act(() => pane.nudge(8)); // Right-anchored: a positive step narrows it.
    expect(pane.filled).toBe(false);
    // From 716 it is no longer at the edge of its room, so the next press widens.
    act(() => pane.fill());
    expect(pane.size).toBe(724);
  });

  it("goes back to where it started when it is already as wide as it goes", () => {
    // Left wide by an earlier window: nothing remembered, so not a no-op.
    act(() => pane.fill());
    act(() => pane.start(0));
    act(() => document.dispatchEvent(new MouseEvent("mouseup")));
    act(() => pane.fill());
    expect(pane.size).toBe(560);
  });
});
