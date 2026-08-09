import { useState } from "react";

import type { SkillProposal } from "../lib/api";

/**
 * The approval gate for a generated skill.
 *
 * Everything the model wrote is visible before it becomes something the agent
 * will follow later — the trigger line, the procedure, and any script source in
 * full. An approved skill is future instructions, so nothing is elided.
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
            New skill: <code>{proposal.name}</code>
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
            <dd>{proposal.rationale}</dd>
          </>
        )}
      </dl>

      <button className="disclosure" onClick={() => setShowBody(!showBody)}>
        {showBody ? "▾" : "▸"} procedure
      </button>
      {showBody && <pre className="proposal-body">{proposal.body}</pre>}

      {proposal.scripts.length > 0 && (
        <div className="proposal-scripts">
          {proposal.scripts.map((script) => (
            <div key={script.path}>
              <button
                className="disclosure"
                onClick={() =>
                  setOpenScript(openScript === script.path ? null : script.path)
                }
              >
                {openScript === script.path ? "▾" : "▸"} {script.path}{" "}
                <span className="muted">({script.interpreter})</span>
              </button>
              {openScript === script.path && (
                <pre className="proposal-body">{script.content}</pre>
              )}
            </div>
          ))}
        </div>
      )}

      <div className="proposal-actions">
        <label className="save-target">
          Save to
          <select
            value={target}
            onChange={(e) => setTarget(e.target.value as "project" | "user")}
          >
            <option value="project">this project</option>
            <option value="user">all projects</option>
          </select>
        </label>
        <div className="spacer" />
        <button className="primary" onClick={() => onResolve(true, target)}>
          Save skill
        </button>
        <button onClick={() => onResolve(false, target)}>Discard</button>
      </div>
    </div>
  );
}
