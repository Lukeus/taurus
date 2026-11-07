import { useEffect, useState } from "react";

import * as api from "../../lib/api";
import { basename, when } from "../../lib/format";
import { Problem } from "../Problem";

const STATE_WORD: Record<string, string> = {
  kept: "waiting to be applied",
  running: "still working",
  applied: "applied",
  discarded: "discarded",
  empty: "changed nothing",
};

/**
 * Work delegates kept in copies of the workspace, still waiting.
 *
 * Here so it can't pile up out of sight: every run that holds a commit nobody
 * has applied or discarded, or left a folder that wouldn't go, is listed with
 * whose it was and when. Apply is on the delegation's card, where its files
 * are; discarding is the one thing worth doing from a list.
 */
export function DelegateRunsSection() {
  const [runs, setRuns] = useState<api.RunSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    api
      .delegateRuns()
      .then(setRuns)
      .catch((e) => setError(String(e)));
  };
  useEffect(load, []);

  const discard = (run: api.RunSummary) => {
    if (!run.session) return;
    setError(null);
    api
      .discardDelegate(run.session, run.run)
      .then(load)
      .catch((e) => setError(String(e)));
  };

  if (!runs || (runs.length === 0 && !error)) return null;
  return (
    <section className="section delegate-runs">
      <span className="micro">Kept by delegates</span>
      <p className="hint m-0">
        Changes delegates made in their own copy of a workspace. Nothing here is
        in a workspace until it's applied from the delegation's card.
      </p>
      <ul className="delegate-runs-list">
        {runs.map((run) => (
          <li key={run.run} className="card-row">
            <span className="text-12-5">
              {run.agent} in <span className="font-mono">{basename(run.workspace)}</span>
              <span className="text-dim">
                {" "}
                · {when(run.created)} · {STATE_WORD[run.state] ?? run.state}
              </span>
              {run.left_behind && <span className="hint block">{run.left_behind}</span>}
            </span>
            <div className="spacer" />
            {run.state !== "running" && run.session && (
              <button onClick={() => discard(run)}>Discard</button>
            )}
          </li>
        ))}
      </ul>
      {error && <Problem>{error}</Problem>}
    </section>
  );
}
