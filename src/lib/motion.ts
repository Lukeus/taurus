import { useEffect, useLayoutEffect, useState, type RefObject } from "react";

/**
 * Whether this machine has asked for less movement.
 *
 * Almost all of Taurus's motion is CSS, and CSS answers this question itself in
 * one `@media` block at the bottom of the stylesheet. This exists for the one
 * animation that is not CSS — the waveform, which is a `requestAnimationFrame`
 * loop writing transforms — where honouring the preference means *not starting
 * the loop*, rather than starting it and then hiding what it does.
 *
 * Subscribed rather than read once. The preference is a system setting and it
 * changes while apps are running: on macOS, Reduce Motion is a checkbox in
 * Accessibility, and somebody who ticks it mid-session means it now.
 */
export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(prefersReducedMotion);
  useEffect(() => {
    const query = window.matchMedia?.(REDUCED);
    if (!query) return;
    const answer = () => setReduced(query.matches);
    query.addEventListener("change", answer);
    return () => query.removeEventListener("change", answer);
  }, []);
  return reduced;
}

const REDUCED = "(prefers-reduced-motion: reduce)";

/**
 * The same question, answered once and without a subscription.
 *
 * Guarded because `matchMedia` is missing in jsdom and in any environment that
 * renders this on a server. Absent means "no preference expressed", which is
 * the same answer a browser gives when nobody has expressed one.
 */
export function prefersReducedMotion(): boolean {
  return window.matchMedia?.(REDUCED).matches ?? false;
}

/**
 * Moves one marker to whichever tab is selected, so it glides there instead
 * of each tab drawing its own and the line jumping.
 *
 * Writes the selected tab's box to the row as `--glide-x`, `--glide-w` and
 * `--glide-top`, for a `.glide` element inside it to sit on. The row says
 * `data-glide` once it has measured, which is when a row's CSS hides each tab's
 * own marker. Until then, or with no script, the tabs draw their own as before.
 *
 * `layout` is anything that can move the selected tab without resizing it or
 * the row: which tab it is, and the tabs and counts drawn before it. Closing a
 * tab to its left is the case a size watcher alone would miss.
 *
 * The first position is set without moving: a marker that slid in from the
 * row's left edge on every mount would be motion about nothing. Measured again
 * when the row or the tab changes size, which fonts loading and a tab's name
 * arriving both do.
 *
 * A transform moving, not a pseudo-element travelling: see the first rule in
 * the motion section of `styles.css`.
 */
export function useGlide(
  row: RefObject<HTMLElement | null>,
  layout: string,
  selector = ".on",
): void {
  useLayoutEffect(() => {
    const el = row.current;
    if (!el) return;
    const place = () => {
      const tab = el.querySelector<HTMLElement>(selector);
      if (!tab) {
        el.removeAttribute("data-glide");
        return;
      }
      el.style.setProperty("--glide-x", `${tab.offsetLeft}px`);
      el.style.setProperty("--glide-w", `${tab.offsetWidth}px`);
      el.style.setProperty("--glide-top", `${tab.offsetTop}px`);
      if (!el.hasAttribute("data-glide")) {
        el.setAttribute("data-glide", "set");
        // From the next frame on, a change of tab moves it.
        requestAnimationFrame(() => {
          if (el.getAttribute("data-glide") === "set") el.setAttribute("data-glide", "on");
        });
      }
    };
    place();
    if (typeof ResizeObserver === "undefined") return;
    const watch = new ResizeObserver(place);
    watch.observe(el);
    const tab = el.querySelector<HTMLElement>(selector);
    if (tab) watch.observe(tab);
    return () => watch.disconnect();
  }, [row, layout, selector]);
}
