import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { SkillSummary } from "../lib/api";
import { plural } from "../lib/format";
import { useStore } from "../state/store";

type Filter = "all" | "project" | "attention";

/**
 * Every procedure the agent can reach for, and every tool server behind them.
 *
 * The filters are the three questions actually asked of this list: what is
 * there, what did this project add, and what is broken. A skill whose scripts
 * will not run is the one that needs finding, so it gets its own filter and
 * says what went wrong rather than only that something did.
 */
export function SkillsDrawer({ onClose }: { onClose: () => void }) {
  const [skills, setSkills] = useState<SkillSummary[] | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [mcpError, setMcpError] = useState<string | null>(null);
  /*
   * One stable slice, narrowed here rather than in the selector.
   *
   * zustand v5 hands the selector straight to `useSyncExternalStore`, which
   * compares what comes back with `Object.is`. A selector that ends in
   * `.filter(…)` or `?? []` allocates a fresh array on every call, so the
   * snapshot never compares equal, and React tears the tree down with "the
   * result of getSnapshot should be cached" — this drawer simply refused to
   * open. `s.status` is one reference that only changes when the status does.
   */
  const status = useStore((s) => s.status);
  // Only what this drawer is actually about. Provider and search failures go
  // to Settings, which is where they can be fixed — an untagged list put them
  // here, under a heading about skills.
  const problems = (status?.problems ?? []).filter(
    (p) => p.source === "skills" || p.source === "mcp",
  );
  const mcpServers = status?.mcp_servers ?? [];

  useEffect(() => {
    api.listSkills().then(setSkills).catch(() => setSkills([]));
  }, []);

  const { all, project, attention, shown } = partition(skills ?? [], filter);

  return (
    <div className="scrim" onClick={onClose}>
      <aside className="drawer" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-head">
          <h2>Skills</h2>
          <button
            onClick={async () => {
              await api.reloadConfig();
              setSkills(await api.listSkills());
            }}
          >
            Rescan
          </button>
          <button className="drawer-close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>

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

        {skills !== null && shown.length === 0 && (
          <p className="drawer-empty">
            {filter === "all"
              ? "No skills yet. Taurus will offer to write one when it works out a procedure worth keeping."
              : "Nothing here."}
          </p>
        )}

        <ul className="card-list">
          {shown.map((skill) => (
            <li key={`${skill.tier}:${skill.name}`} className="card">
              <div className="card-body">
                <div className="card-row">
                  <span className="card-title">{skill.name}</span>
                  <span className={`tag ${skill.tier === "project" ? "project" : ""}`}>
                    {TIER_LABEL[skill.tier]}
                  </span>
                  {/* Only for skills Taurus did not install. Where a borrowed
                      skill came from is the first thing you want to know about
                      it; where Taurus's own skills live is not news. */}
                  {originLabel(skill.origin) && (
                    <span className="tag">{originLabel(skill.origin)}</span>
                  )}
                  {skill.degraded && <span className="tag warn">degraded</span>}
                </div>
                <span className="card-sub">{skill.when_to_use}</span>
                {/* The skill's own claim about what it needs to run. Taurus
                    cannot check it, and says so by placing it here rather than
                    beside `degraded`, which is a fact Taurus established. */}
                {skill.compatibility && (
                  <span className="card-files">needs {skill.compatibility}</span>
                )}
                {/* Wrong but survivable — a name in the wrong case, a colon
                    the YAML never quoted. The skill works, so it belongs on
                    its own row rather than under "Could not load". */}
                {skill.warnings.map((warning) => (
                  <span key={warning} className="card-files warn">
                    {warning}
                  </span>
                ))}
                {skill.degraded ? (
                  <span className="card-files warn">
                    {skill.degraded} — Taurus will follow the written steps.
                  </span>
                ) : (
                  skill.scripts.length > 0 && (
                    <span className="card-files">
                      {plural(skill.scripts.length, "script")} ·{" "}
                      {[...new Set(skill.scripts.map((s) => s.interpreter))].join(", ")}
                    </span>
                  )
                )}
                {/* What the skill says it reaches for. Advisory — it is not a
                    grant, and every call still meets the permission gate — but
                    it is the one thing you would want to read before running
                    something a model wrote. */}
                {skill.allowed_tools.length > 0 && (
                  <span className="card-files">
                    uses {skill.allowed_tools.join(", ")}
                  </span>
                )}
              </div>
            </li>
          ))}
        </ul>

        <section className="section">
          <div className="section-head">
            <span className="micro">MCP servers</span>
            {/* A route to the file, not a form. The format is the one Claude
                Desktop uses and entries get pasted between the two, so a
                reimplementation here would be a schema Taurus does not own and
                would have to chase. */}
            <button
              className="link"
              onClick={async () => {
                try {
                  await api.openMcpConfig("global");
                } catch (e) {
                  setMcpError(String(e));
                }
              }}
            >
              Edit mcp.json
            </button>
          </div>
          {mcpError && <p className="settings-problem">{mcpError}</p>}
          {mcpServers.length === 0 && (
            <p className="drawer-empty">
              No servers configured. Add one to mcp.json, then Rescan.
            </p>
          )}
          {mcpServers.length > 0 &&
            mcpServers.map((server) => (
              <div key={server.name} className="section-row">
                <span className={`dot ${server.connected ? "ok" : "error"}`} />
                <span className="name">{server.name}</span>
                <span className={`value${server.connected ? "" : " error"}`}>
                  {server.connected
                    ? plural(server.tool_count, "tool")
                    : (server.error ?? "failed")}
                </span>
              </div>
            ))}
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
 * The three questions actually asked of the skill list: what is there, what
 * did this project add, and what needs looking at.
 *
 * Every count comes out of one pass so the pills and the list below them can
 * never disagree — a filter reading "Needs attention 2" over a list of three
 * is the kind of thing nobody reports and everybody distrusts.
 */
export function partition(skills: SkillSummary[], filter: Filter) {
  const project = skills.filter((s) => s.tier === "project");
  // Both kinds of "something is off with this one": scripts that will not run
  // here, and a file that loaded only because Taurus was lenient about it.
  // Separating them would mean two filters answering the same question.
  const attention = skills.filter((s) => s.degraded || s.warnings.length > 0);
  return {
    all: skills,
    project,
    attention,
    shown:
      filter === "project" ? project : filter === "attention" ? attention : skills,
  };
}

const TIER_LABEL: Record<SkillSummary["tier"], string> = {
  user: "all projects",
  project: "project",
};

/**
 * Where a skill was installed, for skills Taurus did not install itself.
 *
 * The directory name rather than a friendlier word, because that is what you
 * will type to go find it. `taurus` returns null on purpose: it is the default
 * location, and a badge saying so on every row is one nobody reads.
 */
export function originLabel(origin: SkillSummary["origin"]): string | null {
  switch (origin) {
    case "agents":
      return ".agents";
    case "claude":
      return ".claude";
    case "taurus":
      return null;
  }
}
