import type { TrustStatus } from "../lib/api";

/**
 * The one place the app says "this project's config is not being read".
 *
 * A banner rather than a modal, and that is the whole design. The decision is
 * not urgent — nothing from this workspace is loaded, so nothing is waiting on
 * an answer — and a modal on open would be a thing to dismiss before getting to
 * work, which is how a security prompt becomes a reflex. This sits above the
 * composer, says exactly what is being left unread, and waits.
 *
 * It appears only when there is something to decide: a workspace with no config
 * of its own never raises it. See `taurus_host::trust`.
 */
export function TrustBanner({
  trust,
  onTrust,
  onDismiss,
}: {
  trust: TrustStatus;
  onTrust: () => void;
  /**
   * Hides the banner for this session without recording anything.
   *
   * Deliberately not persisted. A declined workspace and one nobody has decided
   * about are the same state on disk — see `taurus_host::trust` — so "no for
   * now" stays in the window it was said in, and a `git pull` that adds another
   * server gets to raise the question again.
   */
  onDismiss: () => void;
}) {
  if (!trust.decision_needed) return null;

  const items = trust.pending;
  return (
    <div className="banner trust" role="status">
      <div className="trust-body">
        <strong>This project has configuration Taurus is not reading.</strong>
        <ul className="trust-list">
          {items.skills > 0 && <li>{count(items.skills, "skill")}</li>}
          {items.agents > 0 && <li>{count(items.agents, "sub-agent")}</li>}
          {items.instructions > 0 && (
            <li>{count(items.instructions, "instruction file")}</li>
          )}
          {items.mcp_servers > 0 && (
            <li>
              {count(items.mcp_servers, "MCP server")}
              {/* Named, not counted. A command line is the only part of this
                  list someone can actually judge, and it is also the part that
                  starts a process on their machine. */}
              <ul className="trust-commands">
                {items.mcp_commands.map((command) => (
                  <li key={command}>
                    <code>{command}</code>
                  </li>
                ))}
              </ul>
            </li>
          )}
          {items.permission_rules > 0 && (
            <li>
              {count(items.permission_rules, "standing permission grant")} —
              tools this project would allow without asking
            </li>
          )}
          {items.providers && <li>provider endpoints</li>}
          {items.search && <li>web search settings</li>}
          {items.settings && <li>harness settings</li>}
        </ul>
        <p className="trust-note">
          Your own skills, agents, and settings are unaffected — only this
          folder's are being held back.
        </p>
      </div>
      <div className="spacer" />
      <div className="trust-actions">
        {/* Not the primary button. The safe answer is the one that needs no
            reading, and the other one should not be the easiest thing to
            press. */}
        <button onClick={onTrust}>Read this project's config</button>
        <button className="quiet" onClick={onDismiss}>
          Not now
        </button>
      </div>
    </div>
  );
}

function count(n: number, noun: string) {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}
