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
  const problems = useStore((s) => s.status?.skill_problems ?? []);
  const mcpServers = useStore((s) => s.status?.mcp_servers ?? []);

  useEffect(() => {
    api.listSkills().then(setSkills).catch(() => setSkills([]));
  }, []);

  const all = skills ?? [];
  const project = all.filter((s) => s.tier === "project");
  const attention = all.filter((s) => s.degraded);
  const shown =
    filter === "project" ? project : filter === "attention" ? attention : all;

  return (
    <div className="scrim" onClick={onClose}>
      <aside className="drawer" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-head">
          <h2>Skills</h2>
          <button
            onClick={async () => {
              await api.reloadSkills();
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
                  {skill.degraded && <span className="tag warn">degraded</span>}
                </div>
                <span className="card-sub">{skill.when_to_use}</span>
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
              </div>
            </li>
          ))}
        </ul>

        {mcpServers.length > 0 && (
          <section className="section">
            <span className="micro">MCP servers</span>
            {mcpServers.map((server) => (
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
        )}

        {problems.length > 0 && (
          <section className="section">
            <span className="micro">Could not load</span>
            {problems.map((problem) => (
              <p key={problem} className="settings-problem">
                {problem}
              </p>
            ))}
          </section>
        )}
      </aside>
    </div>
  );
}

const TIER_LABEL: Record<SkillSummary["tier"], string> = {
  builtin: "built in",
  user: "all projects",
  project: "project",
};
