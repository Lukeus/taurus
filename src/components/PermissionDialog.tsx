import type { PermissionDecision, PermissionRequest } from "../lib/api";
import { DiffView } from "./DiffView";
import { Modal } from "./Modal";

const EFFECT_LABEL: Record<string, string> = {
  write: "wants to change a file",
  execute: "wants to run a command",
  network: "wants to reach the network",
  read: "wants to read",
};

/**
 * The permission gate.
 *
 * Deliberately shows the exact call rather than a summary: the user is
 * approving this command line or this file write, not the tool in general.
 * The scope sentence sits above the buttons because it is the thing that
 * distinguishes them — which of these grants is wider, and by how much.
 *
 * The one `Modal` with no `onClose`: this is answered, not dismissed. Escape
 * would have to mean one of the buttons, and none of them is what a person
 * pressing Escape has decided.
 */
export function PermissionDialog({
  request,
  onDecide,
}: {
  request: PermissionRequest;
  onDecide: (decision: PermissionDecision) => void;
}) {
  return (
    <Modal>
      <div className="dialog" role="alertdialog" aria-labelledby="perm-title">
        <div className="dialog-head">
          <span className={`effect ${request.effect}`}>{VERB[request.effect]}</span>
          <h2 id="perm-title">
            Taurus {EFFECT_LABEL[request.effect] ?? "wants permission"}
          </h2>
        </div>

        <pre className="dialog-detail">
          {request.effect === "execute" && <span className="prompt">❯ </span>}
          {request.preview}
        </pre>

        {/* Present only for the two tools that can work out a before and an
            after. A command line and a URL have none, and the line above is
            already the whole of what is being approved for those. */}
        {request.diff && <DiffView diff={request.diff} />}

        {/* The scope sentence describes the standing grant, so it goes away
            with it: in an untrusted workspace there is nowhere to keep one, and
            a footnote about a button that is not there reads as a bug. */}
        {request.offer_always && (
          <p className="dialog-footnote">{request.always_scope}.</p>
        )}

        <div className="dialog-actions">
          {/* Not `autoFocus`. Focus lands on the dialog itself — see `Modal` —
              because a prompt that opens with the affirmative already focused
              turns a stray Enter, from someone typing when it appeared, into a
              grant they never read. Tab reaches it in one press. */}
          <button className="primary" onClick={() => onDecide("allow_once")}>
            Allow once
          </button>
          {/* Absent in a workspace whose own config is not being read: there
              is no workspace layer to write a standing decision into, so the
              engine would honor this once and quietly forget it. Offering a
              permanence that will not happen is worse than not offering it. */}
          {request.offer_always && (
            <button
              onClick={() => onDecide("allow_always")}
              data-tip={request.always_scope}
            >
              {request.always_global_scope ? "Always here" : "Allow always"}
            </button>
          )}
          {/* Absent where the wider grant is not on offer — running commands,
              where "every project you ever open" is the wrong unit. */}
          {request.always_global_scope && (
            <button
              onClick={() => onDecide("allow_always_global")}
              data-tip={request.always_global_scope}
            >
              Always everywhere
            </button>
          )}
          <div className="spacer" />
          <button className="danger" onClick={() => onDecide("deny")}>
            Deny
          </button>
        </div>
      </div>
    </Modal>
  );
}

/** The chip above the call, in the imperative the user is being asked about. */
const VERB: Record<string, string> = {
  read: "read",
  write: "write",
  execute: "run",
  network: "network",
};
