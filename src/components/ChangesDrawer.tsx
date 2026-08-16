import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { Checkpoint, Commit, RepoStatus, Restored, TurnChange } from "../lib/api";
import { plural, when } from "../lib/format";
import { DiffView } from "./DiffView";
import { CHANGES_WIDTH, ResizeHandle, useResizableWidth } from "./ResizeHandle";

/**
 * Files this conversation changed, the way back, and the way forward.
 *
 * A rewind overwrites the workspace and cannot itself be undone, so it is
 * never one click: choosing a turn asks the backend what reverting would do
 * and shows that list, and only the second press writes. The same two steps
 * the CLI takes, for the same reason — a checkpoint is only worth having if
 * the user can see what they are about to trade for it.
 *
 * The other direction is newer. A checkpoint lives in the config home, is keyed
 * by session id, and disappears with the conversation; the turn that got it
 * right deserves better than that, and git is where "better" already lives. So
 * each turn can be read as a diff and committed on its own, with the same rule
 * in both directions: what the checkpoint log recorded is what is offered, and
 * nothing else in the tree is touched.
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
  // the paths it lists are as long as the tree they came out of. Diffs made
  // that more true rather than less.
  const pane = useResizableWidth({
    storageKey: "taurus.changesWidth",
    grow: -1,
    ...CHANGES_WIDTH,
  });
  const [turns, setTurns] = useState<Checkpoint[] | null>(null);
  const [repo, setRepo] = useState<RepoStatus | null>(null);
  const [plan, setPlan] = useState<{ turn: number; outcomes: Restored[] } | null>(null);
  const [done, setDone] = useState<Restored[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // One turn open at a time. Expanding every diff in a long conversation is
  // both a wall of text and a request per turn to build it.
  const [open, setOpen] = useState<number | null>(null);

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
    // Read on open rather than held: someone switches branches in a terminal
    // beside this window, and a stale answer would be wrong exactly at the
    // moment before they commit.
    api.repoStatus().then(setRepo).catch(() => setRepo(null));
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

          {repo?.repository && (
            <p className="drawer-intro">
              <span className="tag">
                {repo.branch ?? `detached at ${repo.head ?? "HEAD"}`}
              </span>{" "}
              {repo.branch
                ? "A turn can be committed to this branch on its own."
                : "There is no branch checked out, so a commit here would be hard to find again."}
            </p>
          )}

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
                          aria-expanded={open === turn.turn}
                          onClick={() =>
                            setOpen(open === turn.turn ? null : turn.turn)
                          }
                        >
                          {open === turn.turn ? "Hide changes" : "View changes"}
                        </button>
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

                  {open === turn.turn && (
                    <TurnDetail
                      sessionId={sessionId}
                      turn={turn}
                      repo={repo}
                      busy={busy}
                      onCommitted={refresh}
                    />
                  )}

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

/**
 * One turn opened up: what it changed, and the offer to keep it.
 *
 * Its own component so that expanding a turn fetches one turn's diffs. A long
 * conversation holds a lot of them, and the drawer shows one at a time.
 */
export function TurnDetail({
  sessionId,
  turn,
  repo,
  busy,
  onCommitted,
}: {
  sessionId: string;
  turn: Checkpoint;
  repo: RepoStatus | null;
  busy: boolean;
  onCommitted: () => void;
}) {
  const [changes, setChanges] = useState<TurnChange[] | null>(null);
  // Seeded from what was asked, because the prompt and the commit message
  // agree often enough to be a useful start — and editable, because they
  // agree rarely enough that committing one unread is a bad habit to build.
  const [message, setMessage] = useState(turn.prompt);
  const [committing, setCommitting] = useState(false);
  const [committed, setCommitted] = useState<Commit | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .turnChanges(sessionId, turn.turn)
      .then(setChanges)
      .catch((e) => {
        setError(String(e));
        setChanges([]);
      });
  }, [sessionId, turn.turn]);

  const commit = async () => {
    setError(null);
    setCommitting(true);
    try {
      setCommitted(await api.commitTurn(sessionId, turn.turn, message));
      onCommitted();
    } catch (e) {
      setError(String(e));
    } finally {
      setCommitting(false);
    }
  };

  return (
    <div className="turn-detail">
      {changes === null && <p className="card-files">Reading the diff…</p>}

      {changes?.map((change, i) =>
        change.kind === "diff" ? (
          <DiffView key={i} diff={change.diff} />
        ) : (
          // The same files a rewind reports as skipped. Leaving them out
          // would make the turn look smaller than it was.
          <p key={i} className="card-files">
            <span className="tag warn">not shown</span> {change.path} —{" "}
            {change.reason}
          </p>
        ),
      )}

      {changes?.length === 0 && (
        <p className="card-files">
          This turn's pre-images are recorded, but none of them could be read
          back as text.
        </p>
      )}

      {error && <p className="settings-problem">{error}</p>}

      {committed && (
        <section className="section">
          <p className="card-files">
            <span className="tag">{committed.sha}</span> {committed.subject} —{" "}
            {plural(committed.files.length, "file")}
          </p>
          {/* A commit that quietly covered three of a turn's four files is
              the failure this surface exists to prevent. */}
          {committed.skipped.map((skipped) => (
            <p key={skipped.path} className="card-files">
              <span className="tag warn">not committed</span> {skipped.path} —{" "}
              {skipped.reason}
            </p>
          ))}
        </section>
      )}

      {repo?.repository && !committed && (
        <div className="commit-turn">
          <label className="micro" htmlFor={`commit-${turn.turn}`}>
            Commit message
          </label>
          <input
            id={`commit-${turn.turn}`}
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            placeholder="What this turn changed"
          />
          <div className="actions">
            <button
              // Committing under a running turn would capture a tree that is
              // still being written. The backend refuses too.
              disabled={busy || committing || message.trim() === ""}
              title={
                busy
                  ? "Wait for the current turn to finish"
                  : `Commit this turn's ${plural(turn.files.length, "file")} to ${
                      repo.branch ?? "the current commit"
                    }`
              }
              onClick={commit}
            >
              {committing ? "Committing…" : "Commit this turn"}
            </button>
          </div>
          <p className="drawer-foot">
            Only this turn's files, and only these. Anything you have staged
            stays staged.
          </p>
        </div>
      )}

      {repo && !repo.repository && (
        <p className="drawer-foot">
          {repo.unavailable ??
            "This workspace is not a git repository, so a turn can be undone but not kept."}
        </p>
      )}
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
