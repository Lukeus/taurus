import type { PermissionRequest } from "../lib/api";

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
 */
export function PermissionDialog({
  request,
  onDecide,
}: {
  request: PermissionRequest;
  onDecide: (decision: "allow_once" | "allow_always" | "deny") => void;
}) {
  return (
    <div className="scrim">
      <div className="dialog" role="alertdialog" aria-labelledby="perm-title">
        <div className="dialog-head">
          <span className={`effect ${request.effect}`}>{request.effect}</span>
          <h2 id="perm-title">
            Taurus {EFFECT_LABEL[request.effect] ?? "wants permission"}
          </h2>
        </div>

        <pre className="dialog-detail">{request.preview}</pre>

        <div className="dialog-actions">
          <button className="primary" onClick={() => onDecide("allow_once")} autoFocus>
            Allow once
          </button>
          <button onClick={() => onDecide("allow_always")}>Allow always</button>
          <button className="danger" onClick={() => onDecide("deny")}>
            Deny
          </button>
        </div>

        <p className="dialog-footnote">{request.always_scope}.</p>
      </div>
    </div>
  );
}
