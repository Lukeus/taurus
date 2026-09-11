import { useCallback, useRef } from "react";

/**
 * One function identity for the life of the component, always calling the
 * newest one it was given.
 *
 * The identity is what the memos compare; the freshness is what keeps a
 * callback from closing over a stale render. Without the second half this would
 * be a cache that answers questions with last week's answer.
 *
 * Shared because two places need it for the same reason: the transcript, which
 * hands its callbacks to every memoized turn, and `App`, which hands the rail
 * handlers it rebuilds on every render.
 */
export function useStable<A extends unknown[]>(
  fn: (...args: A) => void,
): (...args: A) => void;
export function useStable<A extends unknown[]>(
  fn: ((...args: A) => void) | undefined,
): ((...args: A) => void) | undefined;
export function useStable<A extends unknown[]>(
  fn: ((...args: A) => void) | undefined,
): ((...args: A) => void) | undefined {
  const held = useRef(fn);
  held.current = fn;

  const stable = useCallback((...args: A) => held.current?.(...args), []);

  // Absence is meaningful further down — a row offers to open a delegate's
  // conversation only where there is somewhere to open one — so an absent
  // callback has to stay absent rather than becoming a function that does
  // nothing.
  return fn === undefined ? undefined : stable;
}
