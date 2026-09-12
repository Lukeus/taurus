import { type DependencyList, useCallback, useEffect, useRef, useState } from "react";

/**
 * What a panel read when it opened, and why it could not, if it could not.
 *
 * Nine panels wrote this out by hand, with three different answers to a failed
 * read: say so, show an empty list, or show nothing and say nothing. An empty
 * list is the worst of the three — it reads as "there are none", which is a
 * claim, and the one thing known is that the read did not happen. So a failure
 * here is always an `error` a panel can show, and `data` stays `null` rather
 * than pretending.
 *
 * Only the latest read lands. A reload started while an earlier one is in
 * flight, or a panel closed before its answer arrives, discards the stale
 * answer instead of letting it overwrite a newer one — the guard only one of
 * the nine had.
 *
 * `set` is for the actions that answer with the new state themselves, like a
 * forget that returns what is left, so the panel shows it without a second
 * read that could race the first.
 */
export function useLoad<T>(
  load: () => Promise<T>,
  deps: DependencyList,
): {
  data: T | null;
  error: string | null;
  reload: () => Promise<void>;
  set: (value: T) => void;
} {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** Bumped per read; an answer carrying an older number is dropped. */
  const latest = useRef(0);
  const read = useRef(load);
  read.current = load;

  const reload = useCallback(async () => {
    const mine = ++latest.current;
    try {
      const value = await read.current();
      if (mine !== latest.current) return;
      setData(value);
      setError(null);
    } catch (e) {
      if (mine !== latest.current) return;
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
    // A read that lands after the panel has gone has nowhere to go.
    return () => {
      latest.current++;
    };
    // The caller's own dependencies decide when to read again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  const set = useCallback((value: T) => {
    latest.current++;
    setData(value);
    setError(null);
  }, []);

  return { data, error, reload, set };
}
