import { useEffect, useLayoutEffect, useRef, useState } from "react";

/*
 * The app's tooltip, as one layer and an attribute.
 *
 * What it replaces is `title`, which the whole app used and which is worse than
 * it looks: the browser decides when it appears (about a second, unconfigurable
 * and different per platform), what it looks like (an OS chrome tooltip, in
 * neither of this app's two themes), and where — a Tauri window on macOS draws
 * it in the system's grey no matter what the stylesheet says. It also cannot be
 * dismissed with a key, and it is the one piece of UI in the window that gives
 * away that this is a webview.
 *
 * The alternative most codebases reach for is a `<Tooltip>` wrapper around each
 * trigger, and that is the thing this deliberately is not. Half the controls
 * here sit in flex rows where an extra element changes the layout — a rail row
 * is a grid of button, label and trash can — so wrapping forty-nine of them
 * would mean forty-nine small layout regressions to find. An attribute changes
 * nothing about the box it is on:
 *
 *   <button data-tip="Delete this conversation">
 *
 * One listener on the document finds the nearest `[data-tip]` above whatever
 * the pointer entered, so nesting works the way CSS does: the trash can's own
 * tip wins over the row's, because it is closer.
 *
 * Disabled controls work too, which took the one piece of cleverness in here —
 * see `trigger`. They are the sites that matter most: four of the app's tips
 * are a sentence explaining why the button under them cannot be pressed.
 */

/** Which edge of the trigger the tip prefers, before it is flipped to fit. */
type Side = "top" | "bottom" | "left" | "right";

/**
 * How long a pointer has to rest before a tip opens.
 *
 * Long enough that crossing a row of controls on the way somewhere else opens
 * nothing, which is the failure mode of a fast tooltip: the pointer travels
 * over four toolbar buttons and each one flashes a box.
 */
const DELAY = 450;

/**
 * How long after one closes that the next opens with no delay at all.
 *
 * Reading a row of icons is one task, not five, and paying the delay once per
 * icon makes a rail feel stuck. Once the first tip has been earned the rest of
 * the row is free, until the pointer settles somewhere else for a moment.
 */
const WARM = 400;

/** The gap between the trigger and the tip, and the tip and the window edge. */
const GAP = 8;
const MARGIN = 8;

/** The id the tip carries, so a trigger can point `aria-describedby` at it. */
const TIP_ID = "app-tip";

interface Open {
  el: HTMLElement;
  text: string;
  side: Side;
}

/**
 * The one tooltip in the window.
 *
 * Mounted once, near the root. There is deliberately no way to have two open:
 * a second tip is always the first one failing to close, and rendering both is
 * how that bug ships.
 */
export function TipLayer() {
  const [open, setOpen] = useState<Open | null>(null);
  const [at, setAt] = useState<{ top: number; left: number; side: Side } | null>(
    null,
  );
  const tip = useRef<HTMLDivElement | null>(null);
  // The pending open, and when the last tip closed — the two halves of the
  // delay. Refs rather than state: neither is rendered, and a timer that
  // re-rendered the whole layer on every pointer move would cost more than the
  // tooltip is worth.
  const timer = useRef<number | undefined>(undefined);
  const closedAt = useRef(0);

  useEffect(() => {
    const cancel = () => {
      window.clearTimeout(timer.current);
      timer.current = undefined;
    };

    const hide = () => {
      cancel();
      setOpen((current) => {
        if (current) closedAt.current = Date.now();
        return null;
      });
      setAt(null);
    };

    /** Opens for `el`, after the delay unless the row is still warm. */
    const show = (el: HTMLElement, immediate: boolean) => {
      const text = el.dataset.tip?.trim();
      if (!text) return;
      const side = sideOf(el.dataset.tipSide);
      cancel();
      const wait = immediate || Date.now() - closedAt.current < WARM ? 0 : DELAY;
      if (wait === 0) {
        setOpen({ el, text, side });
        return;
      }
      timer.current = window.setTimeout(() => setOpen({ el, text, side }), wait);
    };

    const from = (target: EventTarget | null) =>
      target instanceof Element
        ? (target.closest("[data-tip]") as HTMLElement | null)
        : null;

    /**
     * The tipped element under the pointer.
     *
     * Hit-tested rather than read off the event, because of disabled controls.
     * A disabled button dispatches no pointer events at all — the browser
     * retargets them to its nearest enabled ancestor — so reading `e.target`
     * finds whatever is *around* the control and never the control itself.
     * That is precisely backwards: a disabled button is where a tip earns its
     * keep, since "why can I not press this" is the question being asked, and
     * four of the app's are exactly that sentence.
     *
     * `elementFromPoint` is not suppressed the same way and returns the button,
     * measured in this engine rather than assumed. For an enabled control it
     * returns the same element the event did, so there is one path and not two.
     */
    const trigger = (e: PointerEvent, target: EventTarget | null) => {
      // A pointer event with no position — synthesised, or from a device that
      // does not have one — has nothing to hit-test with.
      const hit =
        e.clientX || e.clientY
          ? from(document.elementFromPoint(e.clientX, e.clientY))
          : null;
      return hit ?? from(target);
    };

    const over = (e: PointerEvent) => {
      const el = trigger(e, e.target);
      // A pointer that is dragging is doing something, not reading. This is
      // what keeps a tip off the screen while a pane is being resized.
      if (!el || e.buttons !== 0) {
        hide();
        return;
      }
      show(el, false);
    };

    const out = (e: PointerEvent) => {
      // `pointerout` also fires moving *between* two children of the same
      // trigger, which would close and reopen the tip on every internal edge.
      const to = from(e.relatedTarget);
      if (to !== from(e.target)) hide();
    };

    // Keyboard focus is a deliberate act rather than a pointer passing over, so
    // it opens at once: somebody tabbing to a control has already decided to
    // look at it, and making them wait 450ms is making them wait for nothing.
    const focus = (e: FocusEvent) => {
      const el = from(e.target);
      if (el) show(el, true);
      else hide();
    };

    const key = (e: KeyboardEvent) => {
      // Dismissible without moving the pointer, which is the requirement `title`
      // has never met. Does not stop the event: Escape here must still close
      // whatever dialog is behind the tip.
      if (e.key === "Escape") hide();
    };

    /*
     * Scrolling moves the trigger, so the tip follows it — and lets go once the
     * trigger has left the window.
     *
     * Closing on any scroll at all was the first version and it was wrong in a
     * way only a screenshot caught: focusing a control scrolls it into view, so
     * the scroll a keyboard user's own Tab produces arrived a frame after the
     * tip opened and shut it again. Every tip reached by keyboard flashed once
     * and vanished. Following is also just better with a wheel — a tip that
     * survives a two-line scroll is one you can still be reading.
     */
    const follow = () =>
      setOpen((current) => {
        if (!current) return null;
        const box = current.el.getBoundingClientRect();
        if (
          box.bottom < 0 ||
          box.right < 0 ||
          box.top > window.innerHeight ||
          box.left > window.innerWidth
        ) {
          closedAt.current = Date.now();
          return null;
        }
        // A fresh object, so the placing effect re-runs against wherever the
        // trigger has got to. Same three fields; only the identity changes.
        return { ...current };
      });

    document.addEventListener("pointerover", over);
    document.addEventListener("pointerout", out);
    document.addEventListener("focusin", focus);
    document.addEventListener("focusout", hide);
    document.addEventListener("keydown", key);
    // Capture, because most of these scroll a pane rather than the window and
    // do not bubble.
    window.addEventListener("scroll", follow, true);
    window.addEventListener("resize", hide);
    window.addEventListener("blur", hide);
    return () => {
      cancel();
      document.removeEventListener("pointerover", over);
      document.removeEventListener("pointerout", out);
      document.removeEventListener("focusin", focus);
      document.removeEventListener("focusout", hide);
      document.removeEventListener("keydown", key);
      window.removeEventListener("scroll", follow, true);
      window.removeEventListener("resize", hide);
      window.removeEventListener("blur", hide);
    };
  }, []);

  /*
   * Placed after it has been measured, in a layout effect so the browser never
   * paints the frame where it is the right size in the wrong place.
   *
   * `at` starting null is what keeps that honest: with no position the tip is
   * rendered hidden, so the measuring pass cannot be seen.
   */
  useLayoutEffect(() => {
    if (!open || !tip.current) {
      setAt(null);
      return;
    }
    setAt(place(open.el.getBoundingClientRect(), tip.current.getBoundingClientRect(), open.side));
  }, [open]);

  /* The described-by wiring, undone on the way out. A stale `aria-describedby`
     pointing at an element that is no longer in the document is worse than
     none: a screen reader announces nothing and gives no sign why. */
  useEffect(() => {
    const el = open?.el;
    if (!el) return;
    el.setAttribute("aria-describedby", TIP_ID);
    return () => el.removeAttribute("aria-describedby");
  }, [open]);

  if (!open) return null;
  return (
    <div
      id={TIP_ID}
      ref={tip}
      className={`tip${at ? ` ${at.side}` : ""}`}
      role="tooltip"
      style={
        at
          ? { top: at.top, left: at.left }
          : // The measuring frame: laid out, so it has a size, but not painted
            // and not in anybody's way.
            { top: 0, left: 0, visibility: "hidden" }
      }
    >
      {open.text}
    </div>
  );
}

/**
 * Where the tip goes, given the two boxes and the side it asked for.
 *
 * Flips to the opposite side when the preferred one does not fit, rather than
 * shrinking or scrolling: a tip is one short line, and a tip that has to be
 * scrolled has already failed. Then clamps along the other axis, which is what
 * keeps the tip on a control at the very edge of the window from hanging off it.
 *
 * Exported for the test, which is the only way to check the flip without a
 * browser — jsdom measures every box as zero and would report that everything
 * fits everywhere.
 */
export function place(
  anchor: { top: number; left: number; width: number; height: number },
  tip: { width: number; height: number },
  side: Side,
  view: { width: number; height: number } = {
    width: window.innerWidth,
    height: window.innerHeight,
  },
): { top: number; left: number; side: Side } {
  const flipped = fits(anchor, tip, side, view) ? side : opposite(side);
  const vertical = flipped === "top" || flipped === "bottom";

  const top =
    flipped === "top"
      ? anchor.top - tip.height - GAP
      : flipped === "bottom"
        ? anchor.top + anchor.height + GAP
        : anchor.top + anchor.height / 2 - tip.height / 2;
  const left =
    flipped === "left"
      ? anchor.left - tip.width - GAP
      : flipped === "right"
        ? anchor.left + anchor.width + GAP
        : anchor.left + anchor.width / 2 - tip.width / 2;

  return {
    side: flipped,
    // Only the axis the side did not decide is clamped. Clamping the other one
    // would slide the tip over the control it belongs to.
    top: vertical ? top : clamp(top, MARGIN, view.height - tip.height - MARGIN),
    left: vertical ? clamp(left, MARGIN, view.width - tip.width - MARGIN) : left,
  };
}

function fits(
  anchor: { top: number; left: number; width: number; height: number },
  tip: { width: number; height: number },
  side: Side,
  view: { width: number; height: number },
): boolean {
  if (side === "top") return anchor.top - tip.height - GAP >= MARGIN;
  if (side === "bottom")
    return anchor.top + anchor.height + tip.height + GAP <= view.height - MARGIN;
  if (side === "left") return anchor.left - tip.width - GAP >= MARGIN;
  return anchor.left + anchor.width + tip.width + GAP <= view.width - MARGIN;
}

const OPPOSITE = { top: "bottom", bottom: "top", left: "right", right: "left" } as const;
const opposite = (side: Side): Side => OPPOSITE[side];

function sideOf(value: string | undefined): Side {
  return value === "bottom" || value === "left" || value === "right" ? value : "top";
}

function clamp(value: number, min: number, max: number): number {
  // `max` is below `min` when the tip is wider than the window it is in, and
  // then the near edge is the one to keep: a tip clipped on the right is still
  // readable from its start.
  return Math.max(min, Math.min(value, Math.max(min, max)));
}
