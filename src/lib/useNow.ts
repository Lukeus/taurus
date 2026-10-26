import { useEffect, useState } from "react";

/**
 * The time, redrawn every `every` milliseconds while `active`.
 *
 * For a clock that counts up beside something running. Stops when there's
 * nothing running, so a finished card doesn't redraw itself every second to
 * say the same thing.
 */
export function useNow(active: boolean, every = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), every);
    return () => clearInterval(timer);
  }, [active, every]);
  return now;
}
