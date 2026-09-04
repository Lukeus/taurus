import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { UsageReport } from "../lib/api";
import { short } from "../lib/format";
import { Drawer } from "./Drawer";
import { Problem } from "./Problem";

/**
 * Where the context window actually went.
 *
 * The meter above the composer answers "how much", which is the question you
 * ask once. This answers "on what", which is the one you can act on: a tool
 * that read whole files when it wanted three lines, a grep run four times with
 * the same pattern, a transcript whose bulk is one build log.
 *
 * Two halves, and they are different kinds of fact. The left-hand one is the
 * conversation, read back out of the transcript. The right-hand one is what
 * goes out again on *every* request whether it is used or not — the system
 * prompt and every advertised tool schema — and it is read off the live host,
 * so it describes the next request rather than any earlier one. That second
 * half is the surprising one: it is why a conversation worth a thousand tokens
 * can bill twenty, and it is the half a transcript can never explain.
 *
 * Both come from `taurus_host::usage`, the same code `taurus usage` prints, so
 * the terminal and the window cannot disagree about what a tool cost.
 */
export function UsagePanel({
  sessionId,
  onClose,
}: {
  /** The open conversation, or null if there is none to ask about. */
  sessionId: string | null;
  onClose: () => void;
}) {
  /**
   * Which account is on screen.
   *
   * Starts on the conversation, because that is the one that just did
   * something. The workspace view is the one you go looking for — it answers
   * "is this always like this", which is a question you ask second.
   */
  const [scope, setScope] = useState<"session" | "workspace">(
    sessionId ? "session" : "workspace",
  );
  const [report, setReport] = useState<UsageReport | null>(null);
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    setReport(null);
    setFailed(null);
    api
      .usageReport(scope === "session" ? sessionId : null)
      .then((next) => current && setReport(next))
      .catch((e) => {
        if (!current) return;
        setFailed(String(e));
      });
    // Cancelled rather than ignored on the way out: switching scope twice
    // quickly must not let the first answer land on top of the second.
    return () => {
      current = false;
    };
  }, [scope, sessionId]);

  const schemas = report?.schemas ?? [];
  const schemaTokens = schemas.reduce((n, s) => n + s.tokens, 0);

  return (
    <Drawer title="Context" onClose={onClose} panel="usage">
      <p className="drawer-intro">
        What has been spent, and on what. Token counts through here are
        estimated at four characters each — the figures the provider itself
        reported are on the <b>Billed</b> row, and they are the only exact
        ones.
      </p>

      <div className="usage-scope" role="tablist">
        <button
          role="tab"
          aria-selected={scope === "session"}
          className={`seg${scope === "session" ? " on" : ""}`}
          disabled={!sessionId}
          data-tip={
            sessionId
              ? undefined
              : "There is no conversation open to account for"
          }
          onClick={() => setScope("session")}
        >
          This conversation
        </button>
        <button
          role="tab"
          aria-selected={scope === "workspace"}
          className={`seg${scope === "workspace" ? " on" : ""}`}
          onClick={() => setScope("workspace")}
        >
          Every conversation here
        </button>
      </div>

      {failed && <Problem>{failed}</Problem>}

      {report === null && !failed ? (
        <p className="drawer-loading">Reading…</p>
      ) : report === null ? null : (
        <>
          {report.sessions === 0 ? (
            /* Not an empty state with nothing in it: the fixed cost below is
               the half that does not come from a transcript, and it is the
               half worth reading *before* running anything. */
            <p className="drawer-empty">
              Nothing has been recorded in this workspace yet, so there is no
              history to account for. What every request will cost before the
              conversation starts is below.
            </p>
          ) : (
            <>
              <dl className="usage-totals">
                <Total
                  label="Turns"
                  value={report.turns.toLocaleString()}
                  note={`${report.messages.toLocaleString()} messages${
                    scope === "workspace"
                      ? ` across ${report.sessions.toLocaleString()} conversations`
                      : ""
                  }`}
                />
                <Total
                  label="Billed"
                  value={`${short(report.reported_in)} in · ${short(report.reported_out)} out`}
                  note={
                    /* Only when there was a cache to read from. A local
                       Ollama has no cache to have missed, and a line reading
                       "0 cached" beside its numbers invites exactly the
                       wrong conclusion. */
                    report.cached_in
                      ? `${Math.round(
                          (report.cached_in / Math.max(report.reported_in, 1)) * 100,
                        )}% of input came from cache`
                      : "counted by the provider, not estimated"
                  }
                />
                <Total
                  label="Transcript"
                  value={`~${short(report.history)}`}
                  note="what the messages hold right now"
                />
              </dl>

              {report.tools.length === 0 ? (
                <p className="drawer-empty">No tool calls recorded.</p>
              ) : (
                <>
                  <h3 className="usage-heading">What the tools cost</h3>
                  <table className="usage-table">
                    <thead>
                      <tr>
                        <th>Tool</th>
                        <th className="num">Calls</th>
                        <th className="num">~Tokens</th>
                        <th className="num">Share</th>
                      </tr>
                    </thead>
                    <tbody>
                      {report.tools.map((tool) => (
                        <tr key={tool.name}>
                          <th scope="row">
                            <span className="usage-name">{tool.name}</span>
                            {tool.failures > 0 && (
                              <span
                                className="usage-failed"
                                data-tip="Calls that came back an error. The tokens were spent either way."
                              >
                                {tool.failures} failed
                              </span>
                            )}
                          </th>
                          <td className="num">{tool.calls.toLocaleString()}</td>
                          <td className="num">{short(tool.tokens)}</td>
                          <td className="num">
                            {/* The bar is the row's own background rather
                                than a cell of its own: a share is a property
                                of the line you are reading, and a separate
                                column of bars is one more thing to line up
                                by eye. */}
                            <span
                              className="usage-share"
                              style={{ ["--share" as string]: `${tool.share}%` }}
                            >
                              {tool.share}%
                            </span>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </>
              )}

              {report.repeats > 0 && (
                /* The most actionable number here, so it gets a sentence
                   rather than a row: it is pure waste, and unlike everything
                   else on this panel it can be fixed by asking differently. */
                <p className="usage-note">
                  <b>{report.repeats.toLocaleString()}</b> of those calls
                  repeated an earlier one exactly — same tool, same input —
                  at about <b>{short(report.repeat_tokens)}</b> tokens.
                </p>
              )}
            </>
          )}

          <h3 className="usage-heading">
            Sent again with every request
            <span className="usage-total">~{short(report.system_prompt + schemaTokens)}</span>
          </h3>
          <p className="usage-explain">
            History is there because a turn needed it. This is not: the system
            prompt and every tool schema go out again on each iteration,
            called or not.
          </p>
          <dl className="usage-fixed">
            <Total
              label="System prompt"
              value={`~${short(report.system_prompt)}`}
              note="instructions, skills, and the standing brief"
            />
            <Total
              label={`${schemas.length} tool schemas`}
              value={`~${short(schemaTokens)}`}
              note="every tool advertised, whether or not it is called"
            />
          </dl>

          {schemas.length > 0 && (
            <>
              <table className="usage-table">
                <thead>
                  <tr>
                    <th>Heaviest schemas</th>
                    <th className="num">~Tokens</th>
                  </tr>
                </thead>
                <tbody>
                  {schemas.slice(0, SCHEMAS_LISTED).map((schema) => (
                    <tr key={schema.name}>
                      <th scope="row">
                        <span className="usage-name">{schema.name}</span>
                      </th>
                      <td className="num">{short(schema.tokens)}</td>
                    </tr>
                  ))}
                  {schemas.length > SCHEMAS_LISTED && (
                    /* Said rather than left implied: a list that stops
                       without saying so reads as the whole list, and the
                       tools it hid are exactly the ones somebody deciding
                       what to turn off would want to know about. */
                    <tr className="usage-rest">
                      <th scope="row">
                        {schemas.length - SCHEMAS_LISTED} more
                      </th>
                      <td className="num">
                        {short(
                          schemas
                            .slice(SCHEMAS_LISTED)
                            .reduce((n, s) => n + s.tokens, 0),
                        )}
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
              <p className="usage-note">
                Turn off what this workspace does not use with{" "}
                <code className="md-inline-code">disabled_tools</code> in
                settings.json.
              </p>
            </>
          )}
        </>
      )}

      <p className="drawer-foot">also `taurus usage`</p>
    </Drawer>
  );
}

/** How many schemas to name before summing up the rest. */
const SCHEMAS_LISTED = 5;

function Total({
  label,
  value,
  note,
}: {
  label: string;
  value: string;
  note: string;
}) {
  return (
    <div className="usage-total-row">
      <dt>{label}</dt>
      <dd>{value}</dd>
      <p className="micro">{note}</p>
    </div>
  );
}
