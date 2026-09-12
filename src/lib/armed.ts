import { useCallback, useEffect, useState } from "react";

/** How long a delete stays armed with nobody pressing anything. */
export const ARMED_FOR_MS = 5_000;

/**
 * A destructive control that takes two presses: the first arms it, the second
 * acts.
 *
 * Written out by hand three times — the rail's conversations, a note, an MCP
 * server — each with its own idea of when a question stops being asked. So one
 * left armed stayed armed, and a stray click a minute later answered a question
 * nobody remembered asking. Here it lets go on its own after a few seconds and
 * on Escape, the same everywhere.
 *
 * `T` is what is armed: an id where one control serves a list, so arming a
 * second row disarms the first, or `true` where there is only one.
 */
export function useArmed<T = true>(): {
  armed: T | null;
  arm: (what: T) => void;
  disarm: () => void;
} {
  const [armed, setArmed] = useState<T | null>(null);
  const disarm = useCallback(() => setArmed(null), []);
  const arm = useCallback((what: T) => setArmed(() => what), []);

  useEffect(() => {
    if (armed === null) return;
    const lapse = setTimeout(disarm, ARMED_FOR_MS);
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") disarm();
    };
    window.addEventListener("keydown", key);
    return () => {
      clearTimeout(lapse);
      window.removeEventListener("keydown", key);
    };
  }, [armed, disarm]);

  return { armed, arm, disarm };
}
