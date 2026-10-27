// The Tools tab: a switch for every tool, and what leaving each one on costs.

import { useEffect, useState } from "react";

import * as api from "../../lib/api";
import type { ToolSwitch } from "../../lib/api";
import { short } from "../../lib/format";
import { useStore } from "../../state/store";
import { Problem, Problems } from "../Problem";

/** Past this many tools the list gets a filter box. */
const FILTER_FROM = 12;

/**
 * Which tools the model is offered.
 *
 * Every tool's description and schema goes out with every request, called or
 * not, and with a few MCP servers attached that overhead can outweigh the whole
 * system prompt. `disabled_tools` was the lever, and reaching it meant knowing
 * the exact names and editing settings.json by hand.
 *
 * Each switch writes the whole list and reloads, so what's shown is read back
 * from the host rather than assumed. When the open project sets its own list,
 * that list replaces the global one here, and writing the global file from
 * switches showing the project's would copy the project's choice everywhere —
 * so the switches are shown but not offered.
 */
export function ToolsTab() {
  const status = useStore((s) => s.status);
  const [switches, setSwitches] = useState<ToolSwitch[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [filter, setFilter] = useState("");

  // Leaves the error alone when it succeeds: it runs after a failed switch
  // too, to put back what the host has, and clearing the error there hid it
  // the moment it was shown. A switch clears it as it starts instead.
  const load = () =>
    api
      .toolSwitches()
      .then(setSwitches)
      .catch((e) => setError(`Couldn't list the tools: ${e}`));

  // Again when the registry moves under it: a server connecting adds tools,
  // and a hand-edited settings file changes which are off.
  const registered = status?.tool_names.join("\n");
  const off = status?.settings.disabled_tools.join("\n");
  useEffect(() => {
    void load();
  }, [registered, off]);

  const fromProject = status?.settings.from_project.includes("disabled_tools") ?? false;
  const problems = (status?.problems ?? []).filter((p) => p.source === "tools");

  const toggle = (name: string, on: boolean) => {
    if (!switches) return;
    const disabled = switches
      .filter((s) => (s.name === name ? !on : !s.enabled))
      .map((s) => s.name);
    setBusy(true);
    setError(null);
    setSwitches(switches.map((s) => (s.name === name ? { ...s, enabled: on } : s)));
    api
      .setDisabledTools(disabled)
      .catch((e) => setError(`${name} stayed ${on ? "off" : "on"}: ${e}`))
      .then(load)
      .finally(() => setBusy(false));
  };

  const needle = filter.trim().toLowerCase();
  const shown = (switches ?? []).filter((s) => s.name.toLowerCase().includes(needle));
  const groups = groupByServer(shown);
  const enabled = (switches ?? []).filter((s) => s.enabled);
  const fixed = enabled.reduce((n, s) => n + (s.tokens ?? 0), 0);

  return (
    <>
      <p className="drawer-intro">
        Every tool's description goes out with every request, whether it's
        called or not. Switch off what you never use and stop paying for it. A
        tool that's off can't be reached by skills or sub-agents either.
      </p>

      {problems.length > 0 && <Problems problems={problems.map((p) => p.message)} />}

      {fromProject && (
        <p className="hint">
          This project's <code>.taurus/settings.json</code> chooses which tools
          are off, so its list is the one shown. Change it in that file; the
          switches here set your own list, which other workspaces use.
        </p>
      )}

      {switches === null ? (
        !error && <p className="drawer-loading">Reading…</p>
      ) : (
        <>
          <p className="tools-summary m-0 text-12-5 text-dim">
            {enabled.length} of {switches.length} on, about {short(fixed)}{" "}
            tokens on every request.
          </p>

          {switches.length > FILTER_FROM && (
            <input
              type="search"
              aria-label="Filter tools"
              placeholder="Filter tools"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          )}

          {groups.length === 0 && <p className="drawer-empty">No tool matches “{filter}”.</p>}

          {groups.map(([server, tools]) => (
            <section key={server ?? ""} className="section tools-group">
              <span className="micro">{server ? `MCP · ${server}` : "Taurus"}</span>
              <ul className="m-0 flex list-none flex-col p-0">
                {tools.map((tool) => (
                  <li key={tool.name}>
                    <label className="tool-switch flex items-center gap-2 py-1 text-13">
                      <input
                        type="checkbox"
                        checked={tool.enabled}
                        disabled={busy || fromProject}
                        onChange={(e) => toggle(tool.name, e.target.checked)}
                      />
                      <span
                        className="min-w-0 flex-1 font-mono text-12-5 wrap-anywhere"
                        data-tip={server ? tool.name : undefined}
                      >
                        {server ? tool.name.slice(`mcp__${server}__`.length) : tool.name}
                      </span>
                      <span className="text-12 text-faint tabular-nums">{cost(tool)}</span>
                    </label>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </>
      )}

      {error && <Problem>{error}</Problem>}
    </>
  );
}

/** What a row says it costs. */
function cost(tool: ToolSwitch): string {
  if (!tool.enabled) return "off";
  if (tool.tokens === undefined) return "per turn";
  return `${short(tool.tokens)} tokens`;
}

/** Taurus's own first, then each server's, in the order the host sorted them. */
export function groupByServer(tools: ToolSwitch[]): [string | null, ToolSwitch[]][] {
  const groups = new Map<string | null, ToolSwitch[]>();
  for (const tool of tools) {
    const key = tool.server ?? null;
    groups.set(key, [...(groups.get(key) ?? []), tool]);
  }
  return [...groups.entries()].sort(([a], [b]) =>
    a === null ? -1 : b === null ? 1 : a.localeCompare(b),
  );
}
