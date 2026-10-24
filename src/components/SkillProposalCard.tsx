import { useState } from "react";

import type { SkillProposal } from "../lib/api";

/**
 * The approval gate for a generated skill.
 *
 * Everything the model wrote is visible before it becomes something the agent
 * will follow later — the trigger line, the procedure, and every script,
 * reference and asset in full. An approved skill is future instructions, so nothing is elided: the
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
  // Scripts and the references and assets beside them, opened one at a time
  // the same way: every file a skill carries is read before it's saved.
  const bundled = [
    ...proposal.scripts.map((script) => ({
      path: script.path,
      label: `${script.path} · ${script.interpreter}`,
      content: script.content,
    })),
    ...proposal.files.map((file) => ({
      path: file.path,
      label: file.path,
      content: file.content,
    })),
  ];

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
        {bundled.map((file) => (
          <button
            key={file.path}
            className="pill"
            onClick={() =>
              setOpenScript(openScript === file.path ? null : file.path)
            }
          >
            {openScript === file.path ? "▾" : "▸"} {file.label}
          </button>
        ))}
      </div>

      {showBody && <pre className="proposal-body">{proposal.body}</pre>}
      {bundled
        .filter((file) => file.path === openScript)
        .map((file) => (
          <pre key={file.path} className="proposal-body">
            {file.content}
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
