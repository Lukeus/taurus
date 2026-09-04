import { useState } from "react";

import type { AgentProposal } from "../lib/api";

/**
 * The approval gate for a generated sub-agent.
 *
 * The sibling of {@link SkillProposalCard}, and it withholds no more than that
 * one does: an approved agent is a worker that runs on future turns with your
 * tools, so its system prompt is the card's main content rather than something
 * behind a disclosure.
 *
 * The scope line is the part with no equivalent on a skill card. `tools:` is
 * the only field here that decides what the agent can reach, and "inherits
 * yours" is a materially different answer from a named list — so it is stated
 * either way rather than shown only when present.
 */
export function AgentProposalCard({
  proposal,
  onResolve,
}: {
  proposal: AgentProposal;
  onResolve: (approve: boolean, target: "project" | "user") => void;
}) {
  const [target, setTarget] = useState<"project" | "user">("project");
  const [showPrompt, setShowPrompt] = useState(true);

  return (
    <div className="proposal">
      <div className="proposal-head">
        <span className="glyph">◆</span>
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
        <dt>Can use</dt>
        <dd>
          {proposal.tools === null ? (
            <span className="text-dim">every tool you have</span>
          ) : (
            proposal.tools.join(", ")
          )}
        </dd>
        <dt>Stops after</dt>
        <dd className="text-dim">{proposal.max_iterations} round trips</dd>
        {proposal.rationale && (
          <>
            <dt>Why</dt>
            <dd className="text-dim">{proposal.rationale}</dd>
          </>
        )}
      </dl>

      <div className="proposal-disclosures">
        <button className="pill" onClick={() => setShowPrompt(!showPrompt)}>
          {showPrompt ? "▾" : "▸"} system prompt
        </button>
      </div>

      {showPrompt && <pre className="proposal-body">{proposal.prompt}</pre>}

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
          Save agent
        </button>
      </div>
    </div>
  );
}
