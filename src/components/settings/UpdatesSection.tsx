import { useState } from "react";

import * as api from "../../lib/api";
import { megabytes } from "../../state/voice";
import { Problem } from "../Problem";

type Phase =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "checked"; result: api.UpdateCheck }
  | { kind: "installing"; result: api.UpdateCheck; progress: api.UpdateDownload | null };

/**
 * The version running, and a way to move to a newer one.
 *
 * On the About tab, beside the files, because it's about the app rather than
 * any one setting. Nothing here reaches the network until the
 * button is pressed.
 */
export function UpdatesSection() {
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [error, setError] = useState<string | null>(null);

  const check = () => {
    setError(null);
    setPhase({ kind: "checking" });
    api
      .checkForUpdate()
      .then((result) => setPhase({ kind: "checked", result }))
      .catch((e) => {
        setError(String(e));
        setPhase({ kind: "idle" });
      });
  };

  const install = (result: api.UpdateCheck) => {
    setError(null);
    setPhase({ kind: "installing", result, progress: null });
    // Resolves only on failure; success restarts the app out from under it.
    api
      .installUpdate((progress) => setPhase({ kind: "installing", result, progress }))
      .catch((e) => {
        setError(String(e));
        setPhase({ kind: "checked", result });
      });
  };

  const result = phase.kind === "checked" || phase.kind === "installing" ? phase.result : null;
  const available = result?.available;

  return (
    <section className="section">
      <span className="micro">Updates</span>
      <div className="card-row">
        <span className="text-12-5 text-dim">
          {result ? (
            <>
              Taurus <span className="font-mono">{result.current}</span>
              {available
                ? ` · ${available.version} is available${available.date ? `, released ${available.date}` : ""}`
                : " · up to date"}
            </>
          ) : (
            "Updates are signed, and each one is verified before it installs."
          )}
        </span>
        <div className="spacer" />
        {available && !result?.cannot_install ? (
          <button
            className="primary"
            disabled={phase.kind === "installing"}
            onClick={() => result && install(result)}
          >
            {phase.kind === "installing" ? "Installing…" : `Install ${available.version} and restart`}
          </button>
        ) : (
          <button disabled={phase.kind === "checking"} onClick={check}>
            {phase.kind === "checking" ? "Checking…" : "Check for updates"}
          </button>
        )}
      </div>

      {phase.kind === "installing" && (
        <>
          <div className="plan-bar w-full" aria-hidden="true">
            <span
              className="plan-bar-fill"
              style={{
                width: phase.progress?.total
                  ? `${Math.round((phase.progress.done / phase.progress.total) * 100)}%`
                  : "0%",
              }}
            />
          </div>
          {phase.progress && (
            <p className="hint m-0">
              {megabytes(phase.progress.done)}
              {phase.progress.total ? ` of ${megabytes(phase.progress.total)}` : ""}
            </p>
          )}
        </>
      )}

      {available && !result?.cannot_install && phase.kind !== "installing" && (
        <p className="hint">
          Restarting ends anything still running: a turn in progress, the
          dock's shells and the agent's background commands.
        </p>
      )}
      {available && result?.cannot_install && (
        <p className="hint">{result.cannot_install}</p>
      )}
      {available?.notes && (
        <details>
          <summary className="hint">What's new in {available.version}</summary>
          <p className="hint whitespace-pre-wrap">{available.notes}</p>
        </details>
      )}

      {error && <Problem>{error}</Problem>}
    </section>
  );
}
