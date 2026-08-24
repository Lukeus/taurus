import { useCallback, useState } from "react";

/*
 * Which of the rail's sections are folded away.
 *
 * Stored as the set of *collapsed* names rather than of open ones, so a section
 * added later starts open without anything having to migrate the stored value.
 * That matters more than it sounds: the alternative — remembering what is open
 * — means every new section is invisible to everybody who has used the app
 * before, and it is invisible in a way nobody reports, because a section they
 * have never seen is not a section they can notice is missing.
 */
const KEY = "taurus.railSections";

export interface Sections {
  collapsed: (name: string) => boolean;
  toggle: (name: string) => void;
}

/**
 * The fold state, remembered across launches.
 *
 * A rail that reopens every section on every start is not offering to fold
 * them; it is offering to fold them until you close the window. Somebody who
 * keeps Tools shut has said something about how they work, and the next launch
 * should still know it.
 */
export function useSections(): Sections {
  const [names, setNames] = useState<Set<string>>(read);

  const toggle = useCallback((name: string) => {
    setNames((current) => {
      const next = new Set(current);
      if (!next.delete(name)) next.add(name);
      write(next);
      return next;
    });
  }, []);

  const collapsed = useCallback((name: string) => names.has(name), [names]);
  return { collapsed, toggle };
}

function read(): Set<string> {
  try {
    const stored = localStorage.getItem(KEY);
    if (!stored) return new Set();
    const parsed: unknown = JSON.parse(stored);
    // Anything else is a value this app did not write, or wrote in a version
    // that meant something different by it. Both are answered by opening
    // everything, which is the state the app ships in.
    return Array.isArray(parsed)
      ? new Set(parsed.filter((n): n is string => typeof n === "string"))
      : new Set();
  } catch {
    return new Set();
  }
}

function write(names: Set<string>): void {
  try {
    localStorage.setItem(KEY, JSON.stringify([...names]));
  } catch {
    // A webview with storage turned off still folds. It just forgets.
  }
}
