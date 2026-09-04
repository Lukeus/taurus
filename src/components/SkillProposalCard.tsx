import { useState } from "react";

import type { SkillProposal } from "../lib/api";

/**
 * The approval gate for a generated skill.
 *
 * Everything the model wrote is visible before it becomes something the agent
 * will follow later — the trigger line, the procedure, and any script source in
 * full. An approved skill is future instructions, so nothing is elided: the
 * procedure opens with the card, and only the scripts wait to be asked for.
 */
export function SkillProposalCard({
  proposal,
  onResolve,
}: {
  proposal: SkillProposal;
  onResolve: (approve: boolean, target: "project" | "user") => void;
}) {
  const [target, setTarget] = useState<"project" | "user">("project");
  const [showBody, setShowBody] = useState(true);
  const [openScript, setOpenScript] = useState<string | null>(null);

  return (
    <div className="proposal">
      <div className="proposal-head">
        <span className="glyph">✦</span>
        <div>
          <h3>
            {proposal.name}
            {proposal.replaces_existing && (
              <span className="tag warn">replaces existing</span>
            )}
          </h3>
          <p className="proposal-desc">{proposal.description}</p>
        </div>
      </div>

      <dl className="proposal-meta">
        <dt>Fires when</dt>
        <dd>{proposal.when_to_use}</dd>
        {proposal.rationale && (
          <>
            <dt>Why</dt>
            <dd className="text-dim">{proposal.rationale}</dd>
          </>
        )}
      </dl>

      <div className="proposal-disclosures">
        <button className="pill" onClick={() => setShowBody(!showBody)}>
          {showBody ? "▾" : "▸"} procedure
        </button>
        {proposal.scripts.map((script) => (
          <button
            key={script.path}
            className="pill"
            onClick={() =>
              setOpenScript(openScript === script.path ? null : script.path)
            }
          >
            {openScript === script.path ? "▾" : "▸"} {script.path} ·{" "}
            {script.interpreter}
          </button>
        ))}
      </div>

      {showBody && <pre className="proposal-body">{proposal.body}</pre>}
      {proposal.scripts
        .filter((script) => script.path === openScript)
        .map((script) => (
          <pre key={script.path} className="proposal-body">
            {script.content}
          </pre>
        ))}

      <div className="proposal-actions">
        <span className="micro">Save to</span>
        <button
          className={`pill${target === "project" ? " on" : ""}`}
          onClick={() => setTarget("project")}
        >
          this project
        </button>
        <button
          className={`pill${target === "user" ? " on" : ""}`}
          onClick={() => setTarget("user")}
        >
          all projects
        </button>
        <div className="spacer" />
        <button className="quiet" onClick={() => onResolve(false, target)}>
          Discard
        </button>
        <button className="primary" onClick={() => onResolve(true, target)}>
          Save skill
        </button>
      </div>
    </div>
  );
}
