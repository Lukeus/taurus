import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { AgentSummary, AgentTier } from "../lib/api";
import {
  clampIterations,
  DEFAULT_MAX_ITERATIONS,
  MAX_ITERATIONS_LIMIT,
} from "../lib/limits";
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

        {/* The two limits are separate numbers that read as one concept, and
            keeping them two screens apart is how someone raises an agent's
            ceiling and wonders why the turn that spawned it still stops. */}
        <p className="drawer-intro hint">
          Each agent's limit is its own. The conversation that delegates gets{" "}
          {status?.settings.max_iterations ?? DEFAULT_MAX_ITERATIONS} steps of
          its own — Settings › Behavior.
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
            <AgentCard key={agent.name} agent={agent} onChanged={refresh} />
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
function AgentCard({
  agent,
  onChanged,
}: {
  agent: AgentSummary;
  onChanged: () => Promise<void>;
}) {
  const tools = agent.tools ? chips(agent.tools) : null;
  // Everything that is true of every agent, on one line, because separately
  // they are four lines of grey that push the next card off the screen. The
  // iteration limit used to sit here too and now has its own control — a
  // number you can change does not belong in a row of facts you cannot.
  const meta = [
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
        <IterationField agent={agent} onChanged={onChanged} />
        <span className="card-files">{meta.join(" · ")}</span>
      </div>
    </li>
  );
}

/**
 * One agent's iteration limit, editable where it is read.
 *
 * Retuning a delegate is the one thing about an agent someone changes without
 * wanting to rewrite it, and sending them to the editor to change a number
 * meant loading a system prompt to save it back unchanged. The write goes
 * through a command that edits the file in place, so `model:` and anything else
 * hand-written survives — the editor's save path rebuilds a file from a
 * proposal, which deliberately does not carry those.
 *
 * A built-in has no file. Changing one writes a copy you own that shadows it,
 * which the hint says before the click rather than leaving someone to find an
 * agents directory they did not know they had.
 */
function IterationField({
  agent,
  onChanged,
}: {
  agent: AgentSummary;
  onChanged: () => Promise<void>;
}) {
  const [draft, setDraft] = useState(String(agent.max_iterations));
  const [error, setError] = useState<string | null>(null);
  const builtin = agent.path === null;

  // A rescan is the authority — it is what a save is followed by, and what
  // another pane's edit arrives through.
  useEffect(() => setDraft(String(agent.max_iterations)), [agent.max_iterations]);

  const commit = async () => {
    const next = clampIterations(draft, agent.max_iterations);
    setDraft(String(next));
    if (next === agent.max_iterations) return;
    try {
      setError(null);
      await api.setAgentIterations(agent.name, next);
      await onChanged();
    } catch (e) {
      // Put the number back: a field showing a limit the file does not have is
      // worse than one that never moved.
      setDraft(String(agent.max_iterations));
      setError(String(e));
    }
  };

  return (
    <div className="agent-iterations">
      <label>
        <span className="micro">Max iterations</span>
        <input
          className="mono"
          inputMode="numeric"
          aria-label={`Max iterations for ${agent.name}`}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
          }}
        />
      </label>
      <span className="hint">
        {builtin
          ? `1–${MAX_ITERATIONS_LIMIT} · changing this saves a copy you own that shadows the built-in`
          : `1–${MAX_ITERATIONS_LIMIT}`}
      </span>
      {error && <span className="card-files warn">{error}</span>}
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
