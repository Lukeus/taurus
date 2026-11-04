import { useCallback, useEffect, useRef, useState, type RefObject } from "react";

/** How wide the rail opens on a machine that has never been dragged. */
export const RAIL_WIDTH = { initial: 236, min: 180, max: 360 };

/** The same, for the Changes panel. */
export const CHANGES_WIDTH = { initial: 440, min: 320, max: 620 };

/** The same, for the terminal dock — a height rather than a width. */
export const TERMINAL_HEIGHT = { initial: 300, min: 120, max: 900 };

/**
 * The same, for the canvas.
 *
 * Wider than the drawers because this one is read *and* worked in: 560px is
 * about ninety columns of the editor's font, which is where source stops
 * needing a horizontal scrollbar and prose stops being a column of three words.
 * The floor is the point below which it stops being an editor and becomes a
 * strip.
 *
 * No ceiling of its own. What keeps the transcript readable is the slot's
 * `max-width`, which leaves the conversation `--center-min-w` at any window
 * size; a fixed 1000px on top of that only ever bit on a wide display, where
 * it left half the screen to a transcript a few words wide.
 */
export const CANVAS_WIDTH = { initial: 560, min: 380, max: 4096 };

/** How long a pane takes to ease to a toggled size: `--reveal`, in script. */
const EASE_MS = 320;

/**
 * Which way a pane is sized.
 *
 * `x` is a pane beside another one, sized by dragging its left or right edge;
 * `y` is a pane above or below one, sized by dragging the edge between them.
 * Everything else about the two is identical, which is why they share this
 * file rather than each having their own copy of the drag arithmetic.
 */
export type Axis = "x" | "y";

export interface Resizable {
  /** How big the pane is along its axis: a width for `x`, a height for `y`. */
  size: number;
  dragging: boolean;
  min: number;
  max: number;
  /** Defaults to `x`, which is what every pane here was before docks existed. */
  axis?: Axis;
  /** The pointer's position along the axis — `clientX`, or `clientY` for `y`. */
  start: (client: number) => void;
  nudge: (steps: number) => void;
  /**
   * The box the size is applied to, for a pane the layout can squeeze below
   * it. Attached, a drag or an arrow key starts from the size that is drawn
   * rather than the one that was asked for — see `drawn`.
   */
  slot?: RefObject<HTMLDivElement | null>;
  /**
   * Widens the pane to all the room the centre's floor leaves it, or puts it
   * back to the width it had before. Does nothing for a pane without a `slot`.
   */
  fill: () => void;
  /** Whether `fill` widened it and nothing has moved the edge since. */
  filled: boolean;
  /**
   * True for the moment a `fill` takes to settle, so the slot can ease to the
   * new size. Never during a drag: the edge has to stay under the pointer.
   */
  easing: boolean;
}

/**
 * A pane the user can size by dragging its edge.
 *
 * The size outlives the window: a rail that snaps back to 236px every launch
 * is a control that only pretends to be one. It is written when the drag
 * settles rather than on every frame, so a resize costs one write and not two
 * hundred.
 *
 * `grow` is which way the drag runs. The rail is pinned to the left of the
 * window and widens as the pointer moves right; a drawer pinned to the right
 * widens as it moves left, and a dock pinned to the bottom grows as the pointer
 * moves *up*. All three share every other line of this.
 */
export function useResizable({
  storageKey,
  initial,
  min,
  max,
  grow = 1,
  axis = "x",
}: {
  storageKey: string;
  initial: number;
  min: number;
  max: number;
  grow?: 1 | -1;
  axis?: Axis;
}): Resizable {
  const [size, setSize] = useState(() =>
    clamp(remembered(storageKey) ?? initial, min, max),
  );
  const [dragging, setDragging] = useState(false);
  // The width a `fill` widened from, to go back to. Gone once the edge moves.
  const [back, setBack] = useState<number | null>(null);
  const [easing, setEasing] = useState(false);
  // Where the pointer was, and how big the pane was, when the drag began.
  // Sizing from the delta rather than from the pointer's absolute position is
  // what keeps the edge under the cursor it was grabbed with.
  const from = useRef({ at: 0, size: initial });
  const slot = useRef<HTMLDivElement>(null);

  const start = useCallback(
    (client: number) => {
      from.current = { at: client, size: drawn(size, slot.current, axis) };
      setBack(null);
      setDragging(true);
    },
    [size, axis],
  );

  const nudge = useCallback(
    (steps: number) => {
      setBack(null);
      setSize((s) => clamp(drawn(s, slot.current, axis) + steps * grow, min, max));
    },
    [grow, min, max, axis],
  );

  const fill = useCallback(() => {
    const room = roomFor(slot.current, axis);
    if (room === null) return;
    const shown = drawn(size, slot.current, axis);
    let next: number;
    if (back !== null) {
      next = back;
      setBack(null);
    } else if (shown >= room - 1) {
      // Already as wide as it goes, from a drag or an earlier window: there is
      // nothing remembered to go back to, so back to where it started.
      next = initial;
    } else {
      next = room;
      setBack(shown);
    }
    setEasing(true);
    setSize(clamp(next, min, max));
  }, [size, axis, back, initial, min, max]);

  useEffect(() => {
    if (!easing) return;
    const done = setTimeout(() => setEasing(false), EASE_MS);
    return () => clearTimeout(done);
  }, [easing, size]);

  useEffect(() => {
    if (!dragging) return;
    const move = (e: MouseEvent) =>
      setSize(
        sizedBy(from.current, axis === "y" ? e.clientY : e.clientX, grow, min, max),
      );
    const stop = () => setDragging(false);

    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", stop);
    // The pointer leaves the five pixels it grabbed within about a frame of
    // any real drag. Held on the body, the resize cursor survives that.
    const held = axis === "y" ? "resizing-y" : "resizing";
    document.body.classList.add(held);
    return () => {
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", stop);
      document.body.classList.remove(held);
    };
  }, [dragging, grow, min, max, axis]);

  useEffect(() => {
    if (dragging) return;
    try {
      localStorage.setItem(storageKey, String(size));
    } catch {
      // A webview with storage turned off still resizes. It just forgets.
    }
  }, [dragging, size, storageKey]);

  return {
    size,
    dragging,
    min,
    max,
    axis,
    start,
    nudge,
    slot,
    fill,
    filled: back !== null,
    easing: easing && !dragging,
  };
}

/**
 * How wide a side pane can be drawn: its column, less the conversation's floor.
 *
 * The same arithmetic as `.side-slot`'s `max-width`, done here because a width
 * that is only `calc()` inside CSS has no number to ease towards. Widths only;
 * a dock has no `fill`.
 */
export function roomFor(slot: HTMLElement | null, axis: Axis): number | null {
  const column = slot?.parentElement;
  if (!slot || !column || axis !== "x") return null;
  const box = getComputedStyle(column);
  const inner =
    column.clientWidth - (parseFloat(box.paddingLeft) || 0) - (parseFloat(box.paddingRight) || 0);
  const floor = parseFloat(getComputedStyle(slot).getPropertyValue("--center-min-w")) || 0;
  return inner > 0 ? Math.floor(inner - floor) : null;
}

/**
 * How big the pane is on screen: the size it was given, or less where the
 * layout has squeezed it to keep the conversation its room.
 *
 * Only ever less. A dock dragged to 600px in a window that now leaves it 200
 * would otherwise make the first 400px of a drag, or fifty presses of an arrow,
 * do nothing visible — the edge stuck under a pointer that is plainly moving.
 */
export function drawn(size: number, slot: HTMLElement | null, axis: Axis): number {
  if (!slot) return size;
  const box = slot.getBoundingClientRect();
  const shown = axis === "y" ? box.height : box.width;
  // Unmounted, or not laid out yet: nothing to correct by.
  return shown > 0 ? Math.min(size, Math.round(shown)) : size;
}

/** A pane sized by its width. */
export function useResizableWidth(
  options: Parameters<typeof useResizable>[0],
): Resizable {
  return useResizable({ ...options, axis: "x" });
}

/**
 * A pane sized by its height, pinned to the bottom.
 *
 * The sign is fixed here rather than passed, because a bottom dock only has one
 * edge to grab and it is the top one: the pane grows as the pointer moves up,
 * which is the negative direction. A caller that had to remember that would
 * eventually forget.
 */
export function useResizableHeight(
  options: Omit<Parameters<typeof useResizable>[0], "grow" | "axis">,
): Resizable {
  return useResizable({ ...options, grow: -1, axis: "y" });
}

/**
 * The edge itself.
 *
 * A separator in the ARIA sense, which is the one widget role that already
 * means "drag me to resize the thing beside me" — so it takes focus and answers
 * the arrow keys too. A pane that can only be sized with a mouse is a pane half
 * the people who use this app cannot size.
 *
 * The orientation reported is the separator's own, which is the opposite of the
 * axis it sizes: a rail is made wider by dragging a line that runs vertically.
 */
export function ResizeHandle({ pane, label }: { pane: Resizable; label: string }) {
  const vertical = (pane.axis ?? "x") === "x";
  return (
    <div
      className={`${vertical ? "rail-handle" : "dock-handle"}${
        pane.dragging ? " dragging" : ""
      }`}
      role="separator"
      aria-orientation={vertical ? "vertical" : "horizontal"}
      aria-label={label}
      aria-valuenow={Math.round(pane.size)}
      aria-valuemin={pane.min}
      aria-valuemax={pane.max}
      tabIndex={0}
      onDoubleClick={pane.fill}
      onMouseDown={(e) => {
        // Without this the drag selects the transcript it passes over.
        e.preventDefault();
        pane.start(vertical ? e.clientX : e.clientY);
      }}
      onKeyDown={(e) => {
        // Arrows move the edge the way the pointer would, whichever side of
        // the pane this handle is on. Shift covers the range in a few presses.
        const step = e.shiftKey ? 32 : 8;
        const back = vertical ? "ArrowLeft" : "ArrowUp";
        const forward = vertical ? "ArrowRight" : "ArrowDown";
        if (e.key === back) pane.nudge(-step);
        else if (e.key === forward) pane.nudge(step);
        // Enter is what a window splitter answers to besides the arrows.
        else if (e.key === "Enter") pane.fill();
        else return;
        e.preventDefault();
      }}
    />
  );
}

/**
 * How big the pane is once the pointer has reached `client`.
 *
 * Measured from where the drag started rather than from the pane's current
 * size, so a pointer that runs past the limit and comes back tracks the edge
 * again instead of having quietly lost every pixel it overshot by.
 */
export function sizedBy(
  from: { at: number; size: number },
  client: number,
  grow: 1 | -1,
  min: number,
  max: number,
): number {
  return clamp(from.size + (client - from.at) * grow, min, max);
}

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * The size this pane was left at, if there is one to read.
 *
 * Guarded three times over: the tests render these components to a string with
 * no DOM at all; a webview with site data blocked throws on the read as it does
 * on the write — on merely naming `localStorage`, in some — and this runs in
 * App's first render, where a throw leaves no window at all; and a stored size
 * can be anything a previous version, or a hand edit, left behind.
 */
export function remembered(key: string): number | null {
  try {
    if (typeof localStorage === "undefined") return null;
    const stored = Number(localStorage.getItem(key));
    return Number.isFinite(stored) && stored > 0 ? stored : null;
  } catch {
    return null;
  }
}
