import { useState } from "react";

import { Modal } from "./Modal";

/**
 * A rule as the prompt that produced it would have said it: `run_command:cargo`
 * is "run cargo", `fetch_url:docs.rs` is "fetch_url docs.rs", and a rule with
 * no qualifier is the tool itself.
 */
export function describeRule(rule: string): string {
  const colon = rule.indexOf(":");
  if (colon < 0) return rule;
  const tool = rule.slice(0, colon);
  const qualifier = rule.slice(colon + 1);
  return tool === "run_command" ? `run ${qualifier}` : `${tool} ${qualifier}`;
}

/**
 * The offer made on the way out the door.
 *
 * Setting a conversation to unattended refuses everything a standing grant
 * does not cover, and what it is about to be refused is, almost always, what
 * the person just said "allow once" to. This shows those, picked by default,
 * and one press makes them standing for the workspace — the same write that
 * "Allow always" on each of their prompts would have made.
 *
 * Dismissible, and dismissing it changes nothing: the conversation is already
 * unattended, and declining the offer means it refuses as it would have. The
 * switch never widens anything on its own; this is a separate, deliberate
 * press, which is the whole of why it is a dialog rather than a side effect.
 */
export function LeavingDialog({
  rules,
  onGrant,
  onClose,
}: {
  rules: string[];
  onGrant: (rules: string[]) => void;
  onClose: () => void;
}) {
  const [picked, setPicked] = useState<Set<string>>(() => new Set(rules));
  const toggle = (rule: string) =>
    setPicked((before) => {
      const next = new Set(before);
      if (!next.delete(rule)) next.add(rule);
      return next;
    });
  const count = picked.size;

  return (
    <Modal onClose={onClose}>
      <div className="dialog" role="dialog" aria-labelledby="leaving-title">
        <div className="dialog-head">
          <h2 id="leaving-title">Running unattended</h2>
        </div>
        <p className="dialog-footnote">
          Anything you haven't granted is refused while you're away. You
          allowed these once in this conversation. Grant them for this
          workspace and the run can keep using them.
        </p>
        <div className="tool-chips leaving-rules">
          {rules.map((rule) => (
            <button
              key={rule}
              className={`tool-chip pick${picked.has(rule) ? " on" : ""}`}
              aria-pressed={picked.has(rule)}
              data-tip={rule}
              onClick={() => toggle(rule)}
            >
              {describeRule(rule)}
            </button>
          ))}
        </div>
        <div className="dialog-actions">
          <button
            className="primary"
            disabled={count === 0}
            onClick={() => onGrant([...picked])}
          >
            Grant {count === rules.length ? "all" : count} for this workspace
          </button>
          <div className="spacer" />
          <button onClick={onClose}>Leave as is</button>
        </div>
      </div>
    </Modal>
  );
}
