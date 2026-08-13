import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { AgentSummary, AgentTier, Scope } from "../lib/api";
import { useStore } from "../state/store";

type Filter = "all" | "project" | "attention";

/**
 * Every sub-agent this workspace can delegate to, and what each one is scoped
 * to reach.
 *
 * Authoring is the filesystem, so this drawer's job is to show what was found,
 * what shadowed what, and what is broken — and to give someone who has never
 * seen the format a way in. `listAgents` rescans on the way, because a drawer
 * that renders a catalog assembled at startup would be showing the state of a
 * directory the user has since edited.
 */
export function AgentsDrawer({ onClose }: { onClose: () => void }) {
  const [agents, setAgents] = useState<AgentSummary[] | null>(null);
  const [cost, setCost] = useState<number | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [scope, setScope] = useState<Scope>("workspace");
  const [error, setError] = useState<string | null>(null);

  /*
   * One stable slice, narrowed here rather than in the selector — see the same
   * note in SkillsDrawer. A selector ending in `.filter(…)` allocates a fresh
   * array per call, zustand v5 compares snapshots with `Object.is`, and the
   * drawer simply refuses to open.
   */
  const status = useStore((s) => s.status);
  const refreshStatus = useStore((s) => s.refresh);
  const problems = (status?.problems ?? []).filter((p) => p.source === "agents");

  const refresh = async () => {
    const [found, roster] = await Promise.all([
      api.listAgents(),
      api.agentRosterCost(),
    ]);
    setAgents(found);
    setCost(roster);
    // The rescan replaced the host's agent problems, so the list the drawer
    // renders below has to come from after it, not before.
    await refreshStatus();
  };

  useEffect(() => {
    refresh().catch(() => setAgents([]));
    // Once, on open. The rescan is the point of mounting.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const create = async () => {
    setError(null);
    try {
      await api.createAgent(scope, name.trim());
      setCreating(false);
      setName("");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const { all, project, attention, shown } = partition(agents ?? [], filter);

  return (
    <div className="scrim" onClick={onClose}>
      <aside className="drawer" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-head">
          <h2>Agents</h2>
          <button onClick={() => setCreating((open) => !open)}>New agent…</button>
          <button className="drawer-close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>

        {creating && (
          <section className="section">
            {/* A file, not a form: this writes a commented template and opens
                it in whatever edits markdown here. Disk stays the source of
                truth — this only means nobody has to already know the
                frontmatter to write their first agent. */}
            <div className="section-head">
              <span className="micro">New agent</span>
            </div>
            <input
              autoFocus
              value={name}
              placeholder="code-reviewer"
              aria-label="Agent name"
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && create()}
            />
            <div className="pill-row">
              <button
                className={`pill${scope === "workspace" ? " on" : ""}`}
                onClick={() => setScope("workspace")}
              >
                This project
              </button>
              <button
                className={`pill${scope === "global" ? " on" : ""}`}
                onClick={() => setScope("global")}
              >
                All projects
              </button>
            </div>
            <button className="primary" disabled={!name.trim()} onClick={create}>
              Create and open
            </button>
            {error && <p className="settings-problem">{error}</p>}
          </section>
        )}

        <div className="pill-row">
          <button
            className={`pill${filter === "all" ? " on" : ""}`}
            onClick={() => setFilter("all")}
          >
            All {all.length}
          </button>
          <button
            className={`pill${filter === "project" ? " on" : ""}`}
            onClick={() => setFilter("project")}
          >
            This project {project.length}
          </button>
          <button
            className={`pill${filter === "attention" ? " on" : ""}`}
            onClick={() => setFilter("attention")}
            disabled={attention.length === 0}
          >
            Needs attention {attention.length}
          </button>
        </div>

        {agents !== null && shown.length === 0 && (
          <p className="drawer-empty">Nothing here.</p>
        )}

        <ul className="card-list">
          {shown.map((agent) => (
            <li key={agent.name} className="card">
              <div className="card-body">
                <div className="card-row">
                  <span className="card-title">{agent.name}</span>
                  <span
                    className={`tag ${agent.tier === "project" ? "project" : ""}`}
                  >
                    {TIER_LABEL[agent.tier]}
                  </span>
                  {agent.degraded && <span className="tag warn">degraded</span>}
                </div>
                <span className="card-sub">{agent.description}</span>
                {/* Enforced, unlike a skill's tool list: this is the set the
                    child is actually offered. */}
                <span className="card-files">
                  {agent.tools
                    ? `can use ${agent.tools.join(", ")}`
                    : "inherits every tool the main agent has"}
                </span>
                {agent.model && (
                  <span className="card-files">
                    runs on {agent.model}
                    {agent.provider ? ` · ${agent.provider}` : ""}
                  </span>
                )}
                {/* The fact a person opens this drawer to check. */}
                {agent.shadows && (
                  <span className="card-files">
                    replaces the {TIER_LABEL[agent.shadows]} agent of the same
                    name
                  </span>
                )}
                {agent.degraded && (
                  <span className="card-files warn">{agent.degraded}</span>
                )}
                {agent.path && <span className="card-files">{agent.path}</span>}
              </div>
            </li>
          ))}
        </ul>

        <section className="section">
          <div className="section-head">
            <span className="micro">Where agents live</span>
          </div>
          <p className="drawer-empty">
            Any <code>.md</code> file in <code>~/.taurus/agents</code> or{" "}
            <code>&lt;workspace&gt;/.taurus/agents</code>. The file name is the
            agent's name, and the body below the frontmatter is its system
            prompt. A project agent replaces a personal one; either replaces a
            built-in.
          </p>
          {cost !== null && (
            <div className="section-row">
              <span className="name">Roster cost</span>
              <span className="value">
                {cost.toLocaleString()} characters of every request
              </span>
            </div>
          )}
        </section>

        {problems.length > 0 && (
          <section className="section">
            <span className="micro">Could not load</span>
            {problems.map((problem) => (
              <p key={problem.message} className="settings-problem">
                {problem.message}
              </p>
            ))}
          </section>
        )}
      </aside>
    </div>
  );
}

/**
 * The three questions actually asked of the roster: what is there, what did
 * this project add, and what is broken.
 *
 * Every count comes out of one pass, so the pills and the list below them can
 * never disagree.
 */
export function partition(agents: AgentSummary[], filter: Filter) {
  const project = agents.filter((a) => a.tier === "project");
  const attention = agents.filter((a) => a.degraded);
  return {
    all: agents,
    project,
    attention,
    shown:
      filter === "project" ? project : filter === "attention" ? attention : agents,
  };
}

const TIER_LABEL: Record<AgentTier, string> = {
  builtin: "built-in",
  user: "all projects",
  project: "project",
};
