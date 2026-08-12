import { useCallback, useEffect, useRef, useState } from "react";

/** How wide the rail opens on a machine that has never been dragged. */
export const RAIL_WIDTH = { initial: 236, min: 180, max: 360 };

/** The same, for the Changes drawer. */
export const CHANGES_WIDTH = { initial: 440, min: 320, max: 620 };

export interface Resizable {
  width: number;
  dragging: boolean;
  min: number;
  max: number;
  start: (clientX: number) => void;
  nudge: (steps: number) => void;
}

/**
 * A pane the user can size by dragging its edge.
 *
 * The width outlives the window: a rail that snaps back to 236px every launch
 * is a control that only pretends to be one. It is written when the drag
 * settles rather than on every frame, so a resize costs one write and not two
 * hundred.
 *
 * `grow` is which way the drag runs. The rail is pinned to the left of the
 * window and widens as the pointer moves right; a drawer pinned to the right
 * widens as it moves left, and shares every other line of this.
 */
export function useResizableWidth({
  storageKey,
  initial,
  min,
  max,
  grow = 1,
}: {
  storageKey: string;
  initial: number;
  min: number;
  max: number;
  grow?: 1 | -1;
}): Resizable {
  const [width, setWidth] = useState(() =>
    clamp(remembered(storageKey) ?? initial, min, max),
  );
  const [dragging, setDragging] = useState(false);
  // Where the pointer was, and how wide the pane was, when the drag began.
  // Sizing from the delta rather than from the pointer's absolute position is
  // what keeps the edge under the cursor it was grabbed with.
  const from = useRef({ x: 0, width: initial });

  const start = useCallback(
    (clientX: number) => {
      from.current = { x: clientX, width };
      setDragging(true);
    },
    [width],
  );

  const nudge = useCallback(
    (steps: number) => setWidth((w) => clamp(w + steps * grow, min, max)),
    [grow, min, max],
  );

  useEffect(() => {
    if (!dragging) return;
    const move = (e: MouseEvent) =>
      setWidth(sizedBy(from.current, e.clientX, grow, min, max));
    const stop = () => setDragging(false);

    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", stop);
    // The pointer leaves the five pixels it grabbed within about a frame of
    // any real drag. Held on the body, the resize cursor survives that.
    document.body.classList.add("resizing");
    return () => {
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", stop);
      document.body.classList.remove("resizing");
    };
  }, [dragging, grow, min, max]);

  useEffect(() => {
    if (dragging) return;
    try {
      localStorage.setItem(storageKey, String(width));
    } catch {
      // A webview with storage turned off still resizes. It just forgets.
    }
  }, [dragging, width, storageKey]);

  return { width, dragging, min, max, start, nudge };
}

/**
 * The edge itself.
 *
 * A separator in the ARIA sense, which is the one widget role that already
 * means "drag me to resize the thing beside me" — so it takes focus and answers
 * the arrow keys too. A pane that can only be sized with a mouse is a pane half
 * the people who use this app cannot size.
 */
export function ResizeHandle({ pane, label }: { pane: Resizable; label: string }) {
  return (
    <div
      className={`rail-handle${pane.dragging ? " dragging" : ""}`}
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={Math.round(pane.width)}
      aria-valuemin={pane.min}
      aria-valuemax={pane.max}
      tabIndex={0}
      onMouseDown={(e) => {
        // Without this the drag selects the transcript it passes over.
        e.preventDefault();
        pane.start(e.clientX);
      }}
      onKeyDown={(e) => {
        // Arrows move the edge the way the pointer would, whichever side of
        // the pane this handle is on. Shift covers the range in a few presses.
        const step = e.shiftKey ? 32 : 8;
        if (e.key === "ArrowLeft") pane.nudge(-step);
        else if (e.key === "ArrowRight") pane.nudge(step);
        else return;
        e.preventDefault();
      }}
    />
  );
}

/**
 * How wide the pane is once the pointer has reached `clientX`.
 *
 * Measured from where the drag started rather than from the pane's current
 * width, so a pointer that runs past the limit and comes back tracks the edge
 * again instead of having quietly lost every pixel it overshot by.
 */
export function sizedBy(
  from: { x: number; width: number },
  clientX: number,
  grow: 1 | -1,
  min: number,
  max: number,
): number {
  return clamp(from.width + (clientX - from.x) * grow, min, max);
}

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * The width this pane was left at, if there is one to read.
 *
 * Guarded twice over: the tests render these components to a string with no DOM
 * at all, and a stored width can be anything a previous version — or a hand
 * edit — left behind.
 */
function remembered(key: string): number | null {
  if (typeof localStorage === "undefined") return null;
  const stored = Number(localStorage.getItem(key));
  return Number.isFinite(stored) && stored > 0 ? stored : null;
}
