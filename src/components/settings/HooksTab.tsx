// The Hooks tab: every hook from every layer, and a way to switch one off.

import { useEffect, useState } from "react";

import * as api from "../../lib/api";
import type { HookEvent, HookLayer, HookView, Scope } from "../../lib/api";
import { useStore } from "../../state/store";
import { Problem, Problems } from "../Problem";

const WHEN: Record<HookEvent, string> = {
  pre_tool_use: "before a tool call",
  post_tool_use: "after a tool call",
  user_prompt_submit: "when you send a message",
  stop: "when a turn ends",
};

/**
 * The hooks in force, and switches for them.
 *
 * Listing only, plus on and off. A hook is a command line someone wrote, and
 * the file is the better place to write one; what was missing was seeing
 * them all in one place — yours, the project's, every plugin's — and being
 * able to stop one without finding the right file and the right toggle syntax.
 *
 * Switching off asks where when there's a choice. A hook that came from your
 * own file or a plugin can be off in this project only, which writes a toggle
 * into the project's hooks.json, or off everywhere, which writes yours. One
 * the project defines can only be switched off in the project's file.
 * Switching on goes to whichever layer has it off.
 */
export function HooksTab() {
  const status = useStore((s) => s.status);
  const trusted = useStore((s) => s.trust?.trusted ?? false);
  const [hooks, setHooks] = useState<HookView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** The hook whose "where?" choice is showing. */
  const [asking, setAsking] = useState<string | null>(null);

  // Leaves the error alone, for the reason `ToolsTab`'s load does.
  const load = () =>
    api
      .hookList()
      .then(setHooks)
      .catch((e) => setError(`Couldn't list the hooks: ${e}`));

  // Again after any status push: a reload re-reads every hooks.json.
  useEffect(() => {
    void load();
  }, [status]);

  const problems = (status?.problems ?? []).filter((p) => p.source === "hooks");

  const set = (hook: HookView, scope: Scope, enabled: boolean) => {
    setAsking(null);
    setBusy(true);
    setError(null);
    api
      .setHook(hook.hook.name, scope, enabled)
      .catch((e) =>
        setError(`${hook.hook.name} stayed ${enabled ? "off" : "on"}: ${e}`),
      )
      .then(load)
      .finally(() => setBusy(false));
  };

  const switchOff = (hook: HookView) => {
    if (hook.defined_in.kind === "workspace") set(hook, "workspace", false);
    else if (trusted) setAsking(hook.hook.name);
    else set(hook, "global", false);
  };

  return (
    <>
      <p className="drawer-intro">
        Programs Taurus runs at fixed points in a turn, from your{" "}
        <code>hooks.json</code>, this project's, and plugins. A hook can refuse
        a call you allowed but can't allow one you refused. Switching one off
        here edits <code>hooks.json</code>; writing one is done in the file.
      </p>

      {problems.length > 0 && <Problems problems={problems.map((p) => p.message)} />}

      {hooks === null ? (
        !error && <p className="drawer-loading">Reading…</p>
      ) : hooks.length === 0 ? (
        <p className="drawer-empty">
          No hooks. Add one to <code>~/.taurus/hooks.json</code>, or to{" "}
          <code>.taurus/hooks.json</code> in a project you trust.
        </p>
      ) : (
        <ul className="card-list">
          {hooks.map((hook) => (
            <li
              key={hook.hook.name}
              className="card hook-card"
              data-off={hook.enabled ? undefined : true}
            >
              <div className="card-body">
                <div className="card-row">
                  <span
                    className={`card-title font-mono text-12-5 min-w-0 wrap-anywhere${
                      hook.enabled ? "" : " text-faint line-through"
                    }`}
                  >
                    {hook.hook.name}
                  </span>
                  <span className={`tag${hook.defined_in.kind === "workspace" ? " project" : ""}`}>
                    {from(hook.defined_in)}
                  </span>
                  <div className="spacer" />
                  {hook.enabled ? (
                    asking !== hook.hook.name && (
                      <button disabled={busy} onClick={() => switchOff(hook)}>
                        Switch off
                      </button>
                    )
                  ) : (
                    <button
                      disabled={busy}
                      onClick={() => set(hook, scopeOf(hook.decided_by), true)}
                    >
                      Switch on
                    </button>
                  )}
                </div>

                <p className="hint m-0">
                  Runs {WHEN[hook.hook.on]}
                  {hook.hook.matches ? `, for ${hook.hook.matches}` : ""}. Stopped
                  after {Number(hook.hook.timeout_seconds)}s.
                </p>
                <code className="hook-command block font-mono text-12 text-dim wrap-anywhere">
                  {hook.hook.command}
                </code>

                {!hook.enabled && (
                  <p className="hook-off hint m-0">{offBy(hook.decided_by)}</p>
                )}

                {asking === hook.hook.name && (
                  <div className="settings-actions" role="group" aria-label={`Switch off ${hook.hook.name}`}>
                    <button disabled={busy} onClick={() => set(hook, "workspace", false)}>
                      Off in this project
                    </button>
                    <button disabled={busy} onClick={() => set(hook, "global", false)}>
                      Off everywhere
                    </button>
                    <button onClick={() => setAsking(null)}>Cancel</button>
                  </div>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}

      {error && <Problem>{error}</Problem>}
    </>
  );
}

/** Where a hook was defined, as its tag says it. */
function from(layer: HookLayer): string {
  if (layer.kind === "plugin") return `plugin ${layer.name}`;
  return layer.kind === "workspace" ? "this project" : "yours";
}

/** Why a hook that's off is off, saying which file to look in. */
export function offBy(layer: HookLayer): string {
  if (layer.kind === "plugin") return `Off: plugin ${layer.name} ships it switched off.`;
  return layer.kind === "workspace"
    ? "Off in this project, by its .taurus/hooks.json."
    : "Off everywhere, by your ~/.taurus/hooks.json.";
}

/**
 * The layer to switch a hook back on in: the one that has it off. A plugin's
 * own default is switched over from yours, since its manifest isn't a file
 * this app writes.
 */
export function scopeOf(layer: HookLayer): Scope {
  return layer.kind === "workspace" ? "workspace" : "global";
}
