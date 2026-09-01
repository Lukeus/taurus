import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type {
  Checkpoint,
  Commit,
  RepoStatus,
  Restored,
  ReviewReport,
  Rewind,
  TurnChange,
} from "../lib/api";
import { plural, when } from "../lib/format";
import { DiffView } from "./DiffView";
import { Markdown } from "./Markdown";

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
 *
 * Beside the conversation rather than over it, which it was not always. As a
 * modal it covered the one thing a reviewer needs at the same time as the
 * diff: the exchange that produced it. "Why is this line here" is answered by
 * scrolling up in the transcript, and a panel that had to be closed to do that
 * was asking people to hold a diff in their head. The canvas settled the same
 * argument for a file and settled it the same way — see `Canvas` — and this is
 * that decision applied to the other thing worth reading next to a
 * conversation. It shares the canvas's slot, because a window this wide has
 * one column to spare and two would each be too narrow to read.
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
  const [turns, setTurns] = useState<Checkpoint[] | null>(null);
  const [repo, setRepo] = useState<RepoStatus | null>(null);
  const [plan, setPlan] = useState<{ turn: number; rewind: Rewind } | null>(null);
  const [done, setDone] = useState<Rewind | null>(null);
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
      setPlan({ turn, rewind: await api.rewindTo(sessionId, turn, true) });
    } catch (e) {
      setError(String(e));
    }
  };

  const apply = async () => {
    if (!plan) return;
    setError(null);
    try {
      setDone(await api.rewindTo(sessionId, plan.turn, false));
      setPlan(null);
      // The undone turns are gone from the workspace but still in the log, so
      // the list is re-read rather than assumed.
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  // Escape closes it, which `Modal` used to do on this panel's behalf. Not
  // captured, unlike the modal version: this is a pane beside the conversation
  // rather than over it, so anything inside it with its own use for the key —
  // a diff, a commit box — is entitled to answer first.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return (
        <aside className="drawer changes-pane">
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
              {done.restored.map((outcome) => (
                <p key={outcome.path} className="card-files">
                  <Outcome outcome={outcome} />
                </p>
              ))}
              {/* Repeated after the fact as well as before it. These name
                  things that are still true of the workspace now — a commit
                  left pointing at a tree that no longer exists does not stop
                  being a problem because the rewind finished. */}
              {done.warnings.map((warning) => (
                <p key={warning} className="rewind-warning">
                  <span className="tag warn">still to sort out</span> {warning}
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

          {/* Above the turns, because it is the question asked first. What the
              per-turn list below cannot answer is what a file looks like now
              against what it held when the conversation started — a file
              edited in four turns has four diffs down there, and none of them
              is the one somebody reads before committing. See
              `conversation_changes` on the Rust side for why it is not those
              four added up. */}
          {turns !== null && turns.length > 0 && (
            <Everything sessionId={sessionId} turns={turns.length} />
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
                    {/* What a turn's own record now says about itself. Only
                        drawn when there is something to say: a conversation
                        that stayed on one branch and committed nothing is the
                        common case, and a row of empty tags under every turn
                        would make the list harder to read, not fuller. */}
                    {(turn.commit || turn.moved_git) && (
                      <div className="card-row">
                        {turn.commit && (
                          <span className="tag" data-tip="Already in this branch's history">
                            committed {turn.commit}
                          </span>
                        )}
                        {turn.moved_git && (
                          <span className="tag warn" data-tip="A rewind puts the files back and not HEAD">
                            moved git
                          </span>
                        )}
                      </div>
                    )}
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
                          data-tip={
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
                      // A commit is offered on one turn and lands in a history
                      // shared by all of them, so the offer has to be able to
                      // see the others. See `commitCaveats`.
                      turns={turns ?? []}
                      repo={repo}
                      busy={busy}
                      onCommitted={refresh}
                    />
                  )}

                  {plan?.turn === turn.turn && (
                    <div className="rewind-plan">
                      <p>
                        Rewinding here restores{" "}
                        {plural(plan.rewind.restored.length, "file")} to what they
                        held before turn {turn.turn} — including anything you
                        changed by hand since. This cannot be undone.
                      </p>
                      {plan.rewind.restored.map((outcome) => (
                        <p key={outcome.path} className="card-files">
                          <Outcome outcome={outcome} />
                        </p>
                      ))}
                      {/* Between the file list and the button, which is where
                          it is read. These are the parts of the way back a
                          rewind cannot walk: git's own state, a commit already
                          made, a branch that has changed underneath. */}
                      {plan.rewind.warnings.map((warning) => (
                        <p key={warning} className="rewind-warning">
                          <span className="tag warn">not undone</span> {warning}
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
  );
}

/**
 * The whole conversation as one diff per file.
 *
 * Folded shut, and that is the considered position rather than a default: the
 * panel's job on opening is to show *what* changed, which the turn list does
 * in one line each, and unfolding this fetches and renders every diff the
 * conversation produced. It is opened by somebody about to commit or about to
 * hand the work to a reviewer, which is a deliberate act, and it is one click.
 *
 * Fetched when it is opened rather than when the panel is, for the same reason
 * `TurnDetail` is: reading it costs a pass over the checkpoint log and a read
 * of every file it names.
 */
function Everything({ sessionId, turns }: { sessionId: string; turns: number }) {
  const [open, setOpen] = useState(false);
  const [changes, setChanges] = useState<TurnChange[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let live = true;
    api
      .conversationChanges(sessionId)
      .then((all) => live && setChanges(all))
      .catch((e) => {
        if (!live) return;
        setError(String(e));
        setChanges([]);
      });
    return () => {
      live = false;
    };
  }, [open, sessionId]);

  return (
    <section className="section everything">
      <button
        className="quiet everything-head"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <span className="run-chevron">{open ? "⌄" : "›"}</span>
        {open ? "Hide the whole diff" : "View the whole diff"}
        <div className="spacer" />
        <span className="card-files">{plural(turns, "turn")}</span>
      </button>

      {open && changes === null && !error && (
        <p className="card-files">Reading every file it touched…</p>
      )}

      {open &&
        changes?.map((change, i) =>
          change.kind === "diff" ? (
            <DiffView key={i} diff={change.diff} />
          ) : (
            <p key={i} className="card-files">
              <span className="tag warn">not shown</span> {change.path} —{" "}
              {change.reason}
            </p>
          ),
        )}

      {/* A file changed and then changed back is still listed, with nothing in
          it. Saying so beats an empty box, which reads as a diff that failed
          to load. */}
      {open &&
        changes?.every((c) => c.kind === "diff" && c.diff.hunks.length === 0) &&
        changes.length > 0 && (
          <p className="card-files">
            Every file this conversation touched holds what it held before it.
          </p>
        )}

      {error && <p className="settings-problem">{error}</p>}
    </section>
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
  turns,
  repo,
  busy,
  onCommitted,
}: {
  sessionId: string;
  turn: Checkpoint;
  turns: Checkpoint[];
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
  const [reviewing, setReviewing] = useState(false);
  const [review, setReview] = useState<ReviewReport | null>(null);
  const [reviewError, setReviewError] = useState<string | null>(null);

  useEffect(() => {
    api
      .turnChanges(sessionId, turn.turn)
      .then(setChanges)
      .catch((e) => {
        setError(String(e));
        setChanges([]);
      });
  }, [sessionId, turn.turn]);

  /**
   * Hands the diff to an agent that has never seen this conversation.
   *
   * Its own error state rather than the shared one: a review that could not
   * reach the model must not read as a commit that failed, and the two can be
   * in flight at once.
   */
  const runReview = async () => {
    setReviewError(null);
    setReviewing(true);
    try {
      setReview(await api.reviewTurn(sessionId, turn.turn));
    } catch (e) {
      setReviewError(String(e));
    } finally {
      setReviewing(false);
    }
  };

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

      {/* Before the commit box on purpose: reading a turn over is what you do
          before deciding to keep it, and a button under the commit field would
          be offering it after the decision. */}
      {changes !== null && changes.length > 0 && (
        <div className="turn-review">
          <div className="actions">
            <button
              className="quiet"
              disabled={reviewing}
              data-tip="Hands this diff to an agent with none of this conversation's context. Costs a model round trip."
              onClick={runReview}
            >
              {reviewing ? "Reading it over…" : "Review this turn"}
            </button>
          </div>
          {reviewError && <p className="settings-problem">{reviewError}</p>}
          {review && (
            <section className="section">
              {/* Not streaming: a review arrives whole or not at all. */}
              <Markdown text={review.text} streaming={false} />
              <p className="drawer-foot">
                {plural(review.files, "file")} read by <b>{review.model}</b>,
                without the conversation that produced them — so it cannot know
                what was asked for, and may call a deliberate choice a defect.
              </p>
              {/* A review that covered four of six files and did not say so
                  reads as a clean bill of health for all six. */}
              {review.omitted.map((path) => (
                <p key={path} className="card-files">
                  <span className="tag warn">not reviewed</span> {path}
                </p>
              ))}
            </section>
          )}
        </div>
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

      {/* From the log rather than from this component's state, so it
          survives closing the drawer and reopening the conversation. */}
      {turn.commit && !committed && (
        <p className="card-files">
          <span className="tag">{turn.commit}</span> Already committed. Committing
          it again would take whatever these files hold now.
        </p>
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
          {/* Before the button, because after it the commit is made. */}
          {commitCaveats(turns, turn).map((caveat) => (
            <p key={caveat} className="rewind-warning">
              <span className="tag warn">out of order</span> {caveat}
            </p>
          ))}
          <div className="actions">
            <button
              // Committing under a running turn would capture a tree that is
              // still being written. The backend refuses too.
              disabled={busy || committing || message.trim() === ""}
              data-tip={
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

/**
 * What committing `turn` now would carry in with it, or leave stranded.
 *
 * Each commit is offered on its own, which is what makes committing turn 5
 * while turn 4 is still only in the working tree possible — and, until this,
 * silent. Two different things go wrong depending on whether the turns share a
 * file, and they need two different sentences:
 *
 * - **They share one.** `git commit -- <paths>` commits what those paths hold
 *   *now*, so the earlier turn's edits to a shared file go into this commit
 *   wearing this turn's message. That is the sharper of the two, because the
 *   commit is wrong about its own contents rather than merely out of order.
 * - **They do not.** The earlier work stays in the tree, uncommitted, now
 *   sitting underneath a commit it is not in. Nothing is lost; the history just
 *   no longer reads in the order it happened.
 *
 * Computed here rather than in the backend because the drawer already holds
 * every turn and which commit each is in — the listing carries it. Exported for
 * its own test.
 */
export function commitCaveats(turns: Checkpoint[], turn: Checkpoint): string[] {
  const earlier = turns.filter(
    (other) => other.turn < turn.turn && !other.commit && other.files.length > 0,
  );
  if (earlier.length === 0) return [];

  // Not `plural`, which leads with the count: this needs the noun agreeing
  // with a list of turn numbers, not "2 Turns 4, 5".
  const numbers = `${earlier.length === 1 ? "Turn" : "Turns"} ${earlier
    .map((other) => other.turn)
    .join(", ")}`;
  const shared = earlier.flatMap((other) =>
    other.files.filter((file) => turn.files.includes(file)),
  );
  if (shared.length > 0) {
    return [
      `${numbers} also changed ${[...new Set(shared)].join(", ")} and ` +
        `${earlier.length === 1 ? "is" : "are"} not committed. This commit takes ` +
        `what those files hold now, so that work goes in with it.`,
    ];
  }
  return [
    `${numbers} changed files and ${earlier.length === 1 ? "is" : "are"} not ` +
      `committed. Committing this one puts it into history ahead of work it came after.`,
  ];
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
