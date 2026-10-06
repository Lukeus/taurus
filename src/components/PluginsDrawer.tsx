import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import * as api from "../lib/api";
import type { PluginSummary } from "../lib/api";
import { useArmed } from "../lib/armed";
import { Drawer } from "./Drawer";
import { Problem } from "./Problem";

/**
 * Plugins: skills, sub-agents, MCP servers and hooks under one name.
 *
 * The layout is Claude Code's, so a plugin written for it installs here as it
 * is. What it brings goes through the same loaders as everything else, named
 * under the plugin (`ops:deploy`), and shows up in Skills, Agents and MCP with
 * where it came from. What Taurus can't run is listed on its card rather than
 * left out, because a plugin that quietly does half of what its README says is
 * worse than one that says which half. See `taurus_host::plugins`.
 */
export function PluginsDrawer({
  busy,
  trusted,
  onClose,
}: {
  /** A turn is running. Installing, removing or switching reloads everything a
   *  turn reads from, so it waits. */
  busy: boolean;
  /** Whether this project's config is read. A project plugin waits for it. */
  trusted: boolean;
  onClose: () => void;
}) {
  const [plugins, setPlugins] = useState<PluginSummary[] | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  // One change at a time: each reloads the host, and two racing would each
  // read a half-written plugins folder.
  const [working, setWorking] = useState<string | null>(null);
  const { armed, arm, disarm } = useArmed<string>();

  useEffect(() => {
    api
      .listPlugins()
      .then(setPlugins)
      .catch((e) => {
        setPlugins([]);
        setFailed(String(e));
      });
  }, []);

  const change = async (label: string, run: () => Promise<PluginSummary[]>) => {
    if (working) return;
    setFailed(null);
    setWorking(label);
    try {
      setPlugins(await run());
    } catch (e) {
      setFailed(String(e));
    } finally {
      setWorking(null);
      disarm();
    }
  };

  const idle = !busy && working === null;
  const why = busy ? "Wait for the current turn to finish" : undefined;

  return (
    <Drawer title="Plugins" onClose={onClose}>
      <p className="drawer-intro">
        A plugin bundles skills, sub-agents, MCP servers and hooks under one
        name, laid out the way Claude Code lays them out. Its parts are named
        under it, so <code>ops:deploy</code> is plugin <code>ops</code>'s
        deploy skill and can't replace one of yours.
      </p>

      <AddPlugin
        disabled={!idle}
        why={why}
        adding={working === "add"}
        onAdd={(from, project, gitRef) =>
          change("add", () => api.addPlugin(from, project, gitRef))
        }
      />

      {failed && <Problem>{failed}</Problem>}

      {plugins === null ? (
        <p className="drawer-loading">Reading…</p>
      ) : plugins.length === 0 ? (
        <p className="drawer-empty">
          No plugins installed. Add one from a folder or a git URL above; it
          goes in <code>~/.taurus/plugins/</code>, or this project's{" "}
          <code>.taurus/plugins/</code>.
        </p>
      ) : (
        <ul className="card-list">
          {plugins.map((plugin) => {
            const project = plugin.scope === "workspace";
            const key = `${plugin.scope}/${plugin.name}`;
            return (
              <li key={key} className="card plugin-card">
                <div className="card-body">
                  <div className="card-row">
                    <span className="card-title">{plugin.name}</span>
                    {plugin.version && (
                      <span className="card-files">{plugin.version}</span>
                    )}
                    <span className="tag">{project ? "project" : "yours"}</span>
                    <State plugin={plugin} trusted={trusted} />
                  </div>
                  {plugin.description && (
                    <span className="card-sub">{plugin.description}</span>
                  )}
                  <Parts plugin={plugin} />
                  {plugin.problems.map((problem) => (
                    <Problem key={problem}>{problem}</Problem>
                  ))}
                  {plugin.unsupported.map((part) => (
                    <p key={part.part} className="card-files">
                      <span className="tag warn">not run here</span> {part.part} —{" "}
                      {part.reason}
                    </p>
                  ))}
                  {plugin.warnings.map((warning) => (
                    <p key={warning} className="card-files">
                      <span className="tag">note</span> {warning}
                    </p>
                  ))}
                  {plugin.source && (
                    <p className="card-files">
                      from {plugin.source.from}
                      {plugin.source.commit && ` at ${plugin.source.commit.slice(0, 12)}`}
                    </p>
                  )}
                  <div className="card-row">
                    <button
                      className="quiet"
                      disabled={!idle}
                      data-tip={
                        why ??
                        (project
                          ? "Switches it in this project's settings"
                          : "Switches it in your settings")
                      }
                      onClick={() =>
                        change(key, () =>
                          api.setPluginEnabled(plugin.name, project, !plugin.enabled),
                        )
                      }
                    >
                      {plugin.enabled ? "Switch off" : "Switch on"}
                    </button>
                    {plugin.source && (
                      <button
                        className="quiet"
                        disabled={!idle}
                        data-tip={why ?? `Fetch it again from ${plugin.source.from}`}
                        onClick={() =>
                          change(key, () => api.updatePlugin(plugin.name, project))
                        }
                      >
                        {working === key ? "Updating…" : "Update"}
                      </button>
                    )}
                    <div className="spacer" />
                    <button
                      className={armed === key ? "danger" : "quiet"}
                      disabled={!idle}
                      data-tip={why ?? `Deletes ${plugin.root}`}
                      onClick={() =>
                        armed === key
                          ? change(key, () => api.removePlugin(plugin.name, project))
                          : arm(key)
                      }
                    >
                      {armed === key ? "Remove it" : "Remove"}
                    </button>
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Drawer>
  );
}

/** Whether it loads, and if not, why — in the one word a list can carry. */
function State({ plugin, trusted }: { plugin: PluginSummary; trusted: boolean }) {
  if (plugin.problems.length > 0) return <span className="tag warn">doesn't load</span>;
  if (plugin.shadowed)
    return (
      <span className="tag" tabIndex={0} data-tip="A plugin of the same name in this project replaces it">
        replaced
      </span>
    );
  if (plugin.scope === "workspace" && !trusted)
    return (
      <span className="tag warn" tabIndex={0} data-tip="Nothing in it runs until this project is trusted">
        waiting for trust
      </span>
    );
  return <span className="tag">{plugin.enabled ? "on" : "off"}</span>;
}

/** What it brings, by kind. */
function Parts({ plugin }: { plugin: PluginSummary }) {
  const parts = [
    ["skills", plugin.skills],
    ["agents", plugin.agents],
    ["MCP servers", plugin.mcp_servers],
    ["hooks", plugin.hooks],
  ] as const;
  const shown = parts.filter(([, names]) => names.length > 0);
  if (shown.length === 0)
    return <p className="card-files">Nothing in it that Taurus can run.</p>;
  return (
    <>
      {shown.map(([label, names]) => (
        <p key={label} className="card-files">
          <b>{label}</b> {names.join(" · ")}
        </p>
      ))}
    </>
  );
}

/** A folder or a git URL, into yours or this project's. */
function AddPlugin({
  disabled,
  why,
  adding,
  onAdd,
}: {
  disabled: boolean;
  why?: string;
  adding: boolean;
  onAdd: (from: string, project: boolean, gitRef?: string) => Promise<void>;
}) {
  const [from, setFrom] = useState("");
  const [gitRef, setGitRef] = useState("");
  const [project, setProject] = useState(false);
  // A `…/tree/<ref>/<path>` page names its own ref, so it gets no ref box.
  const tree = /^https?:\/\/.+\/tree\/[^/]+\/./.test(from.trim());
  const git = !tree && /^(https?:\/\/|ssh:\/\/|git:\/\/|git@)|\.git$/.test(from.trim());

  const pick = async () => {
    const chosen = await open({ directory: true, title: "Choose a plugin folder" });
    if (typeof chosen === "string") setFrom(chosen);
  };

  return (
    <section className="section plugin-add">
      <label className="micro" htmlFor="plugin-from">
        Add a plugin
      </label>
      <div className="card-row">
        <input
          id="plugin-from"
          value={from}
          onChange={(e) => setFrom(e.target.value)}
          placeholder="A folder, a git URL, or a repository folder's page"
        />
        <button className="quiet" onClick={pick} disabled={disabled}>
          Choose folder…
        </button>
      </div>
      {git && (
        <input
          value={gitRef}
          onChange={(e) => setGitRef(e.target.value)}
          placeholder="Branch or tag (the default branch if empty)"
          aria-label="Branch or tag"
        />
      )}
      <div className="card-row">
        <label className="card-files">
          <input
            type="checkbox"
            checked={project}
            onChange={(e) => setProject(e.target.checked)}
          />{" "}
          Into this project's <code>.taurus/plugins/</code>
        </label>
        <div className="spacer" />
        <button
          disabled={disabled || from.trim() === ""}
          data-tip={why}
          onClick={async () => {
            await onAdd(from.trim(), project, git ? gitRef.trim() : undefined);
            setFrom("");
            setGitRef("");
          }}
        >
          {adding ? (git ? "Cloning…" : "Adding…") : "Add"}
        </button>
      </div>
    </section>
  );
}
