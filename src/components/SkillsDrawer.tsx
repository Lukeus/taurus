import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { Instructions, SkillSummary } from "../lib/api";
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
  const refreshStatus = useStore((s) => s.refresh);
  // Only what this drawer is actually about. Provider and search failures go
  // to Settings, which is where they can be fixed — an untagged list put them
  // here, under a heading about skills. MCP moved out with its servers: a
  // problem belongs on the screen that can fix it, and that is now the MCP
  // panel.
  const problems = (status?.problems ?? []).filter(
    (p) => p.source === "skills" || p.source === "instructions",
  );

  const [instructions, setInstructions] = useState<Instructions[]>([]);

  useEffect(() => {
    api.listSkills().then(setSkills).catch(() => setSkills([]));
    api
      .listInstructions()
      .then(setInstructions)
      .catch(() => setInstructions([]));
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
              setInstructions(await api.listInstructions().catch(() => []));
              // The rescan replaced the host's catalog and its problems, so
              // the count on the rail and the list in here have to come from
              // after it. Without this a rescan that found a new skill showed
              // it below and left the badge on the old number.
              await refreshStatus();
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
                  {originLabel(skill.origin, skill.tier) && (
                    <span className="tag">
                      {originLabel(skill.origin, skill.tier)}
                    </span>
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

        <InstructionsSection instructions={instructions} />

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
 *
 * Copilot is the one origin that needs the tier as well, because it is the one
 * whose two directories are not named the same: a repository's skills sit in
 * `.github` beside everything else GitHub reads, and a person's in a dotdir of
 * Copilot's own. Returning one name for both would send half the readers to a
 * folder that is not there.
 */
export function originLabel(
  origin: SkillSummary["origin"],
  tier: SkillSummary["tier"],
): string | null {
  switch (origin) {
    case "agents":
      return ".agents";
    case "claude":
      return ".claude";
    case "copilot":
      return tier === "project" ? ".github" : ".copilot";
    case "taurus":
      return null;
  }
}

/**
 * The standing brief, listed below the skills because it is the thing already
 * in the prompt: a skill is loaded when the model reaches for it, a brief
 * applies to every turn whether or not anyone remembers writing it.
 *
 * Paths in full rather than filenames, because which of six possible locations
 * a rule came from is the question being answered — `CLAUDE.md` alone does not
 * distinguish the one in this repo from the one in the home directory.
 */
export function InstructionsSection({
  instructions,
}: {
  instructions: Instructions[];
}) {
  return (
    <section className="section">
      <div className="section-head">
        <span className="micro">Instructions</span>
      </div>
      {instructions.length === 0 && (
        <p className="drawer-empty">
          No AGENTS.md, CLAUDE.md, or TAURUS.md found. Add one to the project
          root or your home directory, then Rescan.
        </p>
      )}
      {instructions.map((entry) => (
        <div key={entry.source.path} className="section-row">
          <span className="name">{entry.source.path}</span>
          <span className="value">
            {entry.source.tier === "project" ? "project" : "personal"}
            {/* A rule about some files rather than all of them. Shown because
                a brief listed without its scope reads as one that always
                applies, which is the opposite of what it says. */}
            {entry.applies_to && ` \u00b7 ${entry.applies_to}`}
            {entry.truncated && " \u00b7 truncated"}
          </span>
        </div>
      ))}
    </section>
  );
}
