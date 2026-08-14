import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { AgentSummary, AgentTier } from "../lib/api";
import { AgentEditor } from "./AgentEditor";
import { useStore } from "../state/store";

type Filter = "all" | "builtin" | "attention";

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

  const { all, builtin, attention, shown } = partition(agents ?? [], filter);

  return (
    <div className="scrim" onClick={onClose}>
      <aside className="drawer" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-head">
          <h2>Agents</h2>
          {/* The roster is a directory, and a directory changes under a drawer
              that is already open. Mounting rescans; this is how you rescan
              without closing and reopening. */}
          <button onClick={() => refresh().catch(() => {})}>Rescan</button>
          <button className="drawer-close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>

        <p className="drawer-intro">
          Sub-agents Taurus can delegate to. A project file overrides a user
          file of the same name; either overrides a built-in.
        </p>

        <div className="pill-row">
          <button
            className={`pill${filter === "all" ? " on" : ""}`}
            onClick={() => setFilter("all")}
          >
            All {all.length}
          </button>
          <button
            className={`pill${filter === "builtin" ? " on" : ""}`}
            onClick={() => setFilter("builtin")}
          >
            Built-in {builtin.length}
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
            <AgentCard key={agent.name} agent={agent} />
          ))}
        </ul>

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

        <div className="drawer-foot">
          <button className="card-add" onClick={() => setCreating(true)}>
            New agent — .md with frontmatter
          </button>
          {cost !== null && (
            <p className="agent-cost">
              Roster costs {cost.toLocaleString()} characters of every request.
            </p>
          )}
        </div>
      </aside>

      {creating && (
        <AgentEditor
          onClose={() => setCreating(false)}
          onSaved={async () => {
            setCreating(false);
            await refresh();
          }}
        />
      )}
    </div>
  );
}

/**
 * One agent, as the roster shows it.
 *
 * The order is what someone scanning the list actually asks: what is it called
 * and where did it come from, what is it for, what can it reach, and — only if
 * something is wrong — why. The file path comes last because it is what you
 * need once you have decided to go and edit it.
 */
function AgentCard({ agent }: { agent: AgentSummary }) {
  const tools = agent.tools ? chips(agent.tools) : null;
  // Everything that is true of every agent, on one line, because separately
  // they are four lines of grey that push the next card off the screen.
  const meta = [
    `max ${agent.max_iterations} iterations`,
    agent.model && `runs on ${agent.model}`,
    agent.shadows && `replaces the ${TIER_LABEL[agent.shadows]} agent`,
    agent.path,
  ].filter(Boolean);

  return (
    <li className={`card agent${agent.tier === "project" ? " own" : ""}`}>
      <div className="card-body">
        <div className="card-row">
          <span className="card-title">{agent.name}</span>
          <span className={`tag ${agent.tier === "project" ? "project" : ""}`}>
            {TIER_LABEL[agent.tier]}
          </span>
          {agent.degraded && <span className="tag warn">degraded</span>}
        </div>
        <span className="card-sub">{agent.description}</span>

        {/* Enforced, unlike a skill's tool list: this is the set the child is
            actually offered, so it is shown as the scope it is rather than
            buried in a sentence. */}
        {tools ? (
          <div className="tool-chips">
            {tools.shown.map((tool) => (
              <span key={tool} className="tool-chip">
                {tool}
              </span>
            ))}
            {tools.hidden > 0 && (
              <span className="tool-chip">+{tools.hidden} more</span>
            )}
          </div>
        ) : (
          <span className="card-files">inherits the caller's tools</span>
        )}

        {agent.degraded && (
          <span className="card-files warn">{agent.degraded}</span>
        )}
        <span className="card-files">{meta.join(" · ")}</span>
      </div>
    </li>
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
  const builtin = agents.filter((a) => a.tier === "builtin");
  const attention = agents.filter((a) => a.degraded);
  return {
    all: agents,
    builtin,
    attention,
    shown:
      filter === "builtin" ? builtin : filter === "attention" ? attention : agents,
  };
}

/**
 * The tools a card shows, and how many it had to leave out.
 *
 * A scope of eleven tools would push everything below it off the card, and the
 * first few are enough to tell a read-only agent from one that can write. The
 * whole list is in the editor, where it is the thing being edited.
 */
export function chips(tools: string[], limit = CHIP_LIMIT) {
  return { shown: tools.slice(0, limit), hidden: Math.max(0, tools.length - limit) };
}

const CHIP_LIMIT = 3;

const TIER_LABEL: Record<AgentTier, string> = {
  builtin: "built-in",
  user: "all projects",
  project: "project",
};
