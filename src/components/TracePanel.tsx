import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { TraceReport, TraceStep, TurnTrace } from "../lib/api";
import { Drawer } from "./Drawer";
import { Problem } from "./Problem";

/**
 * Where the time went.
 *
 * The context account next door answers "what filled the window", which is a
 * question about tokens. This is the other one — *why did that take ninety
 * seconds* — and it is the one a transcript cannot answer at all, because a
 * transcript records what was said and not how long any of it took.
 *
 * The source is the span ring in this process: the same OpenTelemetry spans
 * that go to a collector when somebody has configured one, kept locally
 * whether or not anybody has. So this describes what *this run of the app* has
 * done and forgets on quit, which is the honest limit of it and is said on the
 * panel rather than left to be discovered. Durable history across machines and
 * across days is what an OTLP endpoint is for, and both can be on at once.
 *
 * The furniture — the number tables, the scope tabs, the totals — is the
 * context panel's, reused rather than renamed. The two are the same gesture at
 * the same depth in the same drawer, and a second set of styles that merely
 * looked similar would be the thing that eventually drifts.
 */
export function TracePanel({
  sessionId,
  onClose,
}: {
  /** The open conversation, or null if there is none to ask about. */
  sessionId: string | null;
  onClose: () => void;
}) {
  const [scope, setScope] = useState<"session" | "window">(
    sessionId ? "session" : "window",
  );
  const [report, setReport] = useState<TraceReport | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  /**
   * Which turn's waterfall is open, by sequence number.
   *
   * One at a time, and none to begin with. Every turn expanded at once is a
   * page of bars with no way to compare two of them, and the list itself
   * already carries the shape of each turn in its own row.
   */
  const [open, setOpen] = useState<number | null>(null);
  /** Bumped to ask again. The ring fills as the app is used. */
  const [asked, setAsked] = useState(0);

  useEffect(() => {
    let current = true;
    setReport(null);
    setFailed(null);
    api
      .traceReport(scope === "session" ? sessionId : null)
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
  }, [scope, sessionId, asked]);

  const clear = async () => {
    await api.clearTraces();
    setOpen(null);
    setAsked((n) => n + 1);
  };

  return (
    <Drawer title="Traces" onClose={onClose} panel="traces">
      <p className="drawer-intro">
        What each turn spent its wall time on, from the spans the harness
        emits. Kept in memory for this run of the app only — set an OTLP
        endpoint in Settings to send the same spans somewhere that keeps them.
      </p>

      <div className="usage-scope" role="tablist">
        <button
          role="tab"
          aria-selected={scope === "session"}
          className={`seg${scope === "session" ? " on" : ""}`}
          disabled={!sessionId}
          data-tip={
            sessionId ? undefined : "There is no conversation open to time"
          }
          onClick={() => setScope("session")}
        >
          This conversation
        </button>
        <button
          role="tab"
          aria-selected={scope === "window"}
          className={`seg${scope === "window" ? " on" : ""}`}
          onClick={() => setScope("window")}
        >
          Everything since launch
        </button>
      </div>

      {failed && <Problem>{failed}</Problem>}

      {report === null && !failed ? (
        <p className="drawer-loading">Reading…</p>
      ) : report === null ? null : report.spans === 0 ? (
        /* Not a failure and not an error: a window that has not run a turn
           yet has nothing to time, and saying what would fill this is more
           use than an empty table. */
        <p className="drawer-empty">
          Nothing has been timed yet. Every turn this window runs is recorded
          here as it finishes — including the ones that fail, which are
          usually the interesting ones.
        </p>
      ) : (
        <>
          <dl className="usage-totals">
            <Total
              label="Turns"
              value={report.turns.toLocaleString()}
              note={
                report.dropped > 0
                  ? `and ${report.dropped.toLocaleString()} older spans already forgotten`
                  : since(report.since)
              }
            />
            <Total
              label="Median turn"
              value={ms(report.median_turn_ms)}
              note="the middle one, not the average"
            />
            <Total
              label="Slowest turn"
              value={ms(report.slowest_turn_ms)}
              note="a real turn, and usually the one being looked for"
            />
            <Total
              label="Failures"
              value={report.failures.toLocaleString()}
              note="spans of any kind that ended in an error"
            />
          </dl>

          <h3 className="usage-heading">
            Where the time went
            <span className="usage-total">{ms(report.total_ms)}</span>
          </h3>
          <p className="usage-explain">
            Time inside a model call, against everything else — tools doing
            their own work, the harness between steps, and waiting. Tools are
            not summed against this: a <code className="md-inline-code">spawn</code>{" "}
            holds a whole sub-agent turn, so adding it to what ran inside it
            would count the same seconds twice.
          </p>
          <Split model={report.model_ms} other={report.other_ms} />

          {report.recent.length > 0 && (
            <>
              <h3 className="usage-heading">Recent turns</h3>
              <ul className="trace-turns">
                {report.recent.map((turn) => (
                  <Turn
                    key={turn.seq}
                    turn={turn}
                    open={open === turn.seq}
                    onToggle={() =>
                      setOpen(open === turn.seq ? null : turn.seq)
                    }
                  />
                ))}
              </ul>
            </>
          )}

          {report.models.length > 0 && (
            <>
              <h3 className="usage-heading">Models</h3>
              <table className="usage-table">
                <thead>
                  <tr>
                    <th>Model</th>
                    <th className="num">Calls</th>
                    <th className="num">Median</th>
                    <th className="num">Slowest</th>
                    <th className="num">Out/s</th>
                  </tr>
                </thead>
                <tbody>
                  {report.models.map((model) => (
                    <tr key={`${model.provider}/${model.name}`}>
                      <th scope="row">
                        <span className="usage-name">{model.name}</span>
                        {model.failures > 0 && (
                          <span
                            className="usage-failed"
                            data-tip="Requests that came back an error. A retried request is counted twice, because it was two round trips."
                          >
                            {model.failures} failed
                          </span>
                        )}
                        {model.cached_tokens !== null && (
                          /* Only when a backend reported a cache. A local
                             model has none to have missed, and a 0% beside
                             its name invites the wrong conclusion. */
                          <span className="trace-aside">
                            {Math.round(
                              (model.cached_tokens /
                                Math.max(model.input_tokens, 1)) *
                                100,
                            )}
                            % cached
                          </span>
                        )}
                      </th>
                      <td className="num">{model.calls.toLocaleString()}</td>
                      <td className="num">{ms(model.median_ms)}</td>
                      <td className="num">{ms(model.slowest_ms)}</td>
                      <td className="num">
                        {model.output_per_second === null
                          ? "—"
                          : model.output_per_second.toLocaleString()}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </>
          )}

          {report.tools.length > 0 && (
            <>
              <h3 className="usage-heading">Tools</h3>
              <table className="usage-table">
                <thead>
                  <tr>
                    <th>Tool</th>
                    <th className="num">Calls</th>
                    <th className="num">Median</th>
                    <th className="num">Slowest</th>
                    <th className="num">Share</th>
                  </tr>
                </thead>
                <tbody>
                  {report.tools.map((tool) => (
                    <tr key={tool.name}>
                      <th scope="row">
                        <span className="usage-name">{tool.name}</span>
                        {tool.nested && (
                          /* Said rather than left to be inferred from a row
                             that dwarfs the others: this one contains a
                             delegate's whole turn. */
                          <span
                            className="trace-aside"
                            data-tip="This tool ran a sub-agent, so its time includes the delegate's model calls and tools"
                          >
                            includes a delegate
                          </span>
                        )}
                        {tool.failures > 0 && (
                          <span className="usage-failed">
                            {tool.failures} failed
                          </span>
                        )}
                      </th>
                      <td className="num">{tool.calls.toLocaleString()}</td>
                      <td className="num">{ms(tool.median_ms)}</td>
                      <td className="num">{ms(tool.slowest_ms)}</td>
                      <td className="num">
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
              <p className="usage-note">
                Share is of tool time, not of the turn — see above.
              </p>
            </>
          )}

          <div className="trace-foot-actions">
            <button
              className="quiet"
              onClick={() => void clear()}
              data-tip="Forget everything recorded so far, so the next reading is of the next thing you do"
            >
              Clear
            </button>
            <button className="quiet" onClick={() => setAsked((n) => n + 1)}>
              Refresh
            </button>
          </div>
        </>
      )}

      <p className="drawer-foot">
        the same spans an OTLP collector receives
      </p>
    </Drawer>
  );
}

/** How much of a turn's wall time was the model, drawn as one bar. */
function Split({ model, other }: { model: number; other: number }) {
  const total = model + other;
  const share = total > 0 ? Math.round((model / total) * 100) : 0;
  return (
    <div className="trace-split">
      <div
        className="trace-split-bar"
        style={{ ["--model" as string]: `${share}%` }}
        role="img"
        aria-label={`${share}% of the time was inside a model call`}
      />
      <div className="trace-split-key">
        <span>
          <i className="trace-key chat" /> Model {share}% · {ms(model)}
        </span>
        <span>
          <i className="trace-key other" /> Everything else {100 - share}% ·{" "}
          {ms(other)}
        </span>
      </div>
    </div>
  );
}

/** One turn in the list, and its waterfall when it is open. */
function Turn({
  turn,
  open,
  onToggle,
}: {
  turn: TurnTrace;
  open: boolean;
  onToggle: () => void;
}) {
  const share =
    turn.duration_ms > 0
      ? Math.round((turn.model_ms / turn.duration_ms) * 100)
      : 0;
  return (
    <li className={`trace-turn-row${open ? " open" : ""}`}>
      <button className="trace-turn" aria-expanded={open} onClick={onToggle}>
        <span className="trace-when">{clock(turn.started)}</span>
        <span className="trace-turn-name">
          {turn.model || "unknown"}
          {turn.error && (
            <span className="usage-failed">{turn.error}</span>
          )}
        </span>
        <span
          className="trace-split-bar mini"
          style={{ ["--model" as string]: `${share}%` }}
        />
        <span className="trace-turn-ms">{ms(turn.duration_ms)}</span>
      </button>

      {open &&
        (turn.steps.length === 0 ? (
          /* A turn that answered without calling anything. Worth saying:
             an empty waterfall under an expanded row otherwise reads as a
             panel that failed to draw. */
          <p className="trace-empty">
            One model call and no tools — nothing to break down.
          </p>
        ) : (
          <ol className="trace-flow">
            {turn.steps.map((step, i) => (
              <Step key={i} step={step} total={turn.duration_ms} />
            ))}
          </ol>
        ))}
    </li>
  );
}

/** One bar on the waterfall, placed by when it started. */
function Step({ step, total }: { step: TraceStep; total: number }) {
  const span = Math.max(total, 1);
  const offset = Math.min((step.offset_ms / span) * 100, 100);
  // A floor, so a call that took four milliseconds inside a ninety-second turn
  // is still something you can see and point at rather than a hairline.
  const width = Math.max(
    Math.min((step.duration_ms / span) * 100, 100 - offset),
    0.8,
  );
  return (
    <li
      className="trace-step"
      style={{ ["--depth" as string]: String(Math.min(step.depth, 4)) }}
    >
      <span className="trace-step-name" title={step.name}>
        {step.name}
      </span>
      <span className="trace-track">
        <span
          className={`trace-bar ${step.kind}${step.error ? " failed" : ""}`}
          style={{
            ["--offset" as string]: `${offset}%`,
            ["--width" as string]: `${width}%`,
          }}
          data-tip={
            step.error
              ? `${step.name} — ${step.error}`
              : `${step.name} · ${ms(step.duration_ms)}`
          }
        />
      </span>
      <span className="trace-step-ms">{ms(step.duration_ms)}</span>
    </li>
  );
}

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

/**
 * `7400` → `7.4s`.
 *
 * Three bands, because the difference that matters changes with the scale.
 * Under a second, milliseconds are what a tool call is compared in; past it,
 * a decimal second is how long a turn *felt*; past a minute, nobody is
 * counting seconds any more.
 */
export function ms(value: number): string {
  if (value < 1_000) return `${Math.round(value)}ms`;
  if (value < 60_000) return `${(value / 1_000).toFixed(1)}s`;
  const minutes = Math.floor(value / 60_000);
  const seconds = Math.round((value % 60_000) / 1_000);
  return `${minutes}m ${seconds}s`;
}

/** Unix milliseconds → the time of day, which is how a turn is remembered. */
function clock(at: number): string {
  return new Date(at).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** What the window covers, when nothing has been forgotten. */
function since(at: number | null): string {
  return at === null ? "nothing recorded" : `since ${clock(at)}`;
}
