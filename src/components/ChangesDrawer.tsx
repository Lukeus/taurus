import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { Checkpoint, Restored } from "../lib/api";
import { plural, when } from "../lib/format";
import { CHANGES_WIDTH, ResizeHandle, useResizableWidth } from "./ResizeHandle";

/**
 * Files this conversation changed, and the way back.
 *
 * A rewind overwrites the workspace and cannot itself be undone, so it is
 * never one click: choosing a turn asks the backend what reverting would do
 * and shows that list, and only the second press writes. The same two steps
 * the CLI takes, for the same reason — a checkpoint is only worth having if
 * the user can see what they are about to trade for it.
 */
export function ChangesDrawer({
  sessionId,
  busy,
  onClose,
}: {
  sessionId: string;
  busy: boolean;
  onClose: () => void;
}) {
  // The one drawer worth sizing: a rewind is read before it is pressed, and
  // the paths it lists are as long as the tree they came out of.
  const pane = useResizableWidth({
    storageKey: "taurus.changesWidth",
    grow: -1,
    ...CHANGES_WIDTH,
  });
  const [turns, setTurns] = useState<Checkpoint[] | null>(null);
  const [plan, setPlan] = useState<{ turn: number; outcomes: Restored[] } | null>(null);
  const [done, setDone] = useState<Restored[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = () =>
    api
      .listCheckpoints(sessionId)
      .then(setTurns)
      .catch((e) => {
        setError(String(e));
        setTurns([]);
      });

  useEffect(() => {
    refresh();
    // Reloading on every render would fight the plan/confirm state.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  const preview = async (turn: number) => {
    setError(null);
    setDone(null);
    try {
      setPlan({ turn, outcomes: await api.rewindTo(sessionId, turn, true) });
    } catch (e) {
      setError(String(e));
    }
  };

  const apply = async () => {
    if (!plan) return;
    setError(null);
    try {
      const outcomes = await api.rewindTo(sessionId, plan.turn, false);
      setDone(outcomes);
      setPlan(null);
      // The undone turns are gone from the workspace but still in the log, so
      // the list is re-read rather than assumed.
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="scrim" onClick={onClose}>
      <div className="drawer-dock" onClick={(e) => e.stopPropagation()}>
        <ResizeHandle pane={pane} label="Changes drawer width" />
        <aside className="drawer" style={{ width: pane.width }}>
          <header className="drawer-head">
            <h2>Changes</h2>
            <button onClick={refresh}>Refresh</button>
            <button className="drawer-close" onClick={onClose} aria-label="Close">
              ✕
            </button>
          </header>

          <p className="drawer-intro">
            Every file Taurus changed in this conversation, and the state it was
            in before.
          </p>

          {error && <p className="settings-problem">{error}</p>}

          {done && (
            <section className="section">
              <span className="micro">Restored</span>
              {done.map((outcome) => (
                <p key={outcome.path} className="card-files">
                  <Outcome outcome={outcome} />
                </p>
              ))}
            </section>
          )}

          {turns?.length === 0 && (
            <p className="drawer-empty">
              This conversation has not changed any files. Taurus records what a
              file held before it edits it, so a turn can be undone.
            </p>
          )}

          <ul className="card-list">
            {turns
              ?.slice()
              .reverse()
              .map((turn) => (
                <li key={turn.turn} className="card">
                  <div className="card-body">
                    <div className="card-row">
                      <span className="micro turn-no">Turn {turn.turn}</span>
                      <span className="card-files">{when(turn.at)}</span>
                      <div className="spacer" />
                      <span className="card-files">
                        {plural(turn.files.length, "file")}
                      </span>
                    </div>
                    <span className="card-title">
                      {turn.prompt || "(no prompt recorded)"}
                    </span>
                    <span className="card-files">{turn.files.join(" · ")}</span>
                    {plan?.turn !== turn.turn && (
                      <div className="card-row rewind-open">
                        <button
                          className="quiet"
                          // Rewinding under a running turn would race the tool
                          // calls still writing. The backend refuses too; this
                          // just stops the user reaching for it.
                          disabled={busy}
                          title={
                            busy
                              ? "Wait for the current turn to finish"
                              : "Undo this turn and everything after it"
                          }
                          onClick={() => preview(turn.turn)}
                        >
                          Rewind to before this
                        </button>
                      </div>
                    )}
                  </div>

                  {plan?.turn === turn.turn && (
                    <div className="rewind-plan">
                      <p>
                        Rewinding here restores {plural(plan.outcomes.length, "file")}{" "}
                        to what they held before turn {turn.turn} — including
                        anything you changed by hand since. This cannot be undone.
                      </p>
                      {plan.outcomes.map((outcome) => (
                        <p key={outcome.path} className="card-files">
                          <Outcome outcome={outcome} />
                        </p>
                      ))}
                      <div className="actions">
                        <button className="danger" onClick={apply}>
                          Rewind to before turn {turn.turn}
                        </button>
                        <button onClick={() => setPlan(null)}>Cancel</button>
                      </div>
                    </div>
                  )}
                </li>
              ))}
          </ul>

          <p className="drawer-foot">
            run_command is not covered — a shell command's reach is not knowable
            before it runs.
          </p>
        </aside>
      </div>
    </div>
  );
}

export function Outcome({ outcome }: { outcome: Restored }) {
  switch (outcome.action) {
    case "reverted":
      return (
        <>
          <span className="tag">reverted</span> {outcome.path}
        </>
      );
    case "deleted":
      return (
        <>
          <span className="tag">deleted</span> {outcome.path}
        </>
      );
    case "skipped":
      return (
        <>
          <span className="tag warn">skipped</span> {outcome.path} — {outcome.reason}
        </>
      );
  }
}
