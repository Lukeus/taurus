import { useEffect, useState } from "react";

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
