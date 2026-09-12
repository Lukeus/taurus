import type { AllowedRule, Scope } from "../../lib/api";
import { Problem } from "../Problem";

const SCOPE_LABEL: Record<Scope, string> = {
  global: "every workspace",
  workspace: "this workspace",
};

/**
 * The Permissions tab: every approval marked "always", each one revocable.
 *
 * The rules are the drawer's, read once when it opens; `onRevoke` writes the
 * revocation and reads them again, so what is listed is what the next call
 * will be checked against.
 */
export function PermissionsTab({
  rules,
  error,
  onRevoke,
}: {
  rules: AllowedRule[];
  /** Why the last revoke did not take, under the list it was made from. */
  error: string | null;
  onRevoke: (allowed: AllowedRule) => void;
}) {
  return (
    <>
      <p className="drawer-intro">
        Approvals you marked “always”. Revoking one puts the next such
        call back in front of you.
      </p>
      {rules.length === 0 ? (
        <p className="drawer-empty">
          Nothing has been granted permanently yet.
        </p>
      ) : (
        <ul className="card-list">
          {rules.map((allowed) => (
            <li key={`${allowed.scope}:${allowed.rule}`} className="card">
              <div className="card-body">
                <div className="card-row">
                  <span className="card-title font-mono rule-name text-12-5 min-w-0 wrap-anywhere">
                    {allowed.rule}
                  </span>
                  <span
                    className={`tag${allowed.scope === "workspace" ? " project" : ""}`}
                  >
                    {SCOPE_LABEL[allowed.scope]}
                  </span>
                  <div className="spacer" />
                  <button className="danger" onClick={() => onRevoke(allowed)}>
                    Revoke
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}
      {error && <Problem>{error}</Problem>}
    </>
  );
}
