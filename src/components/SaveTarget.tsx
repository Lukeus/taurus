import { useEffect, useState } from "react";

import * as api from "../lib/api";

/**
 * Where an editor saves: this project, all projects, or a plugin you made.
 *
 * `plugin:<name>` rather than a third enum member, because the backend takes
 * a plugin as its own argument beside the two-way target, and one string is
 * what the pills toggle between.
 */
export type SaveTo = "project" | "user" | `plugin:${string}`;

/** The two-way target and the plugin, as the save commands take them. */
export function split(to: SaveTo): { target: "project" | "user"; plugin?: string } {
  return to.startsWith("plugin:")
    ? { target: "user", plugin: to.slice("plugin:".length) }
    : { target: to as "project" | "user" };
}

/** Words for the tag in an editor's head. */
export function label(to: SaveTo): string {
  return to === "project" ? "project" : to === "user" ? "all projects" : `plugin ${split(to).plugin}`;
}

/**
 * The plugins a skill or agent can be saved into: ones you made, which
 * nothing updates. A plugin fetched from somewhere is left out, because its
 * folder is replaced the next time it's updated. One that doesn't load is
 * left out too: what was saved into it wouldn't load either.
 */
export function usePluginTargets(offer: boolean): string[] {
  const [names, setNames] = useState<string[]>([]);
  useEffect(() => {
    if (!offer) return;
    api
      .listPlugins()
      .then((plugins) =>
        setNames(
          plugins
            .filter((p) => !p.source && p.problems.length === 0 && !p.shadowed)
            .map((p) => p.name),
        ),
      )
      // No plugins to offer is the same answer as a listing that failed: the
      // two folders of your own are still there.
      .catch(() => setNames([]));
  }, [offer]);
  return names;
}

/** The pills themselves, for an editor's footer. */
export function SaveTargetPills({
  value,
  onChange,
  plugins,
}: {
  value: SaveTo;
  onChange: (to: SaveTo) => void;
  plugins: string[];
}) {
  const options: SaveTo[] = ["project", "user", ...plugins.map((p) => `plugin:${p}` as SaveTo)];
  return (
    <>
      <span className="micro">Save to</span>
      {options.map((to) => (
        <button
          key={to}
          className={`pill${value === to ? " on" : ""}`}
          onClick={() => onChange(to)}
        >
          {to === "project" ? "this project" : label(to)}
        </button>
      ))}
    </>
  );
}
