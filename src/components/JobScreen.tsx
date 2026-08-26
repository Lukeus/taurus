import { useLayoutEffect, useRef } from "react";

import type { BackgroundJob } from "../lib/api";
import { mark, title, tone } from "../lib/jobs";

/**
 * The background commands, in the dock.
 *
 * `run_command` with `background: true` hands the model a number and goes on
 * printing into a buffer nobody is watching. The card for the call that started
 * it is closed by the time anything arrives, so a build the model can read was
 * a build the user could not see — which is the wrong way round for the one
 * process on the machine neither of them started deliberately.
 *
 * So each one gets a tab beside the shell. Read-only, because that is what it
 * is: there is no pty behind a background command and nothing to type into. See
 * `crates/taurus-tools/src/jobs.rs` for the buffer both readers share and the
 * cursors that keep them from emptying it for each other.
 *
 * Deliberately free of the emulator. `TerminalDock` carries the largest import
 * in the frontend and this is a block of text — keeping the two apart is what
 * lets this be mounted in a test and drawn in a screenshot.
 */
export function DockTabs({
  jobs,
  watching,
  onWatch,
}: {
  jobs: BackgroundJob[];
  /** The tab on screen. `null` is the shell, which is always the first one. */
  watching: number | null;
  onWatch: (id: number | null) => void;
}) {
  return (
    <div className="dock-tabs" role="tablist" aria-label="Terminal tabs">
      <button
        className={`dock-tab${watching === null ? " on" : ""}`}
        role="tab"
        aria-selected={watching === null}
        onClick={() => onWatch(null)}
      >
        Terminal
      </button>
      {jobs.map((job) => (
        <button
          key={job.id}
          className={`dock-tab ${tone(job)}${watching === job.id ? " on" : ""}`}
          role="tab"
          aria-selected={watching === job.id}
          onClick={() => onWatch(job.id)}
          /* The label is clipped to fit a strip; the tip is the command as it
             was actually run, which is the thing worth being able to check. */
          data-tip={job.command}
          title={job.command}
        >
          <span className="dock-tab-num">#{job.id}</span>
          {mark(job) && <span className="dock-tab-mark">{mark(job)}</span>}
          {title(job.command)}
        </button>
      ))}
    </div>
  );
}

/**
 * One background command's output.
 *
 * Text rather than an emulator, and that follows from what is upstream: a
 * background command runs with pipes and no pseudo-terminal, so nothing
 * addresses a screen by coordinate and there is nothing for an emulator to do
 * that a scrolled block of text does not. Escape sequences from a command that
 * colours anyway are taken off in the host — see `Jobs::read`.
 */
export function JobScreen({
  job,
  text,
  problem,
  onStop,
}: {
  job: BackgroundJob;
  /** Everything the pane has collected, gaps marked. See `extend` in `lib/jobs`. */
  text: string;
  problem: string | null;
  onStop: () => void;
}) {
  const view = useRef<HTMLPreElement>(null);
  /**
   * Whether to follow the output down.
   *
   * A build tails itself, which is what anybody wants until they scroll up to
   * read something — and a pane that yanks you back to the bottom every time a
   * line arrives is one you cannot read at all. So following is a state that
   * scrolling away turns off and scrolling back turns on, the way every log
   * viewer behaves.
   */
  const following = useRef(true);

  useLayoutEffect(() => {
    const element = view.current;
    if (!element || !following.current) return;
    element.scrollTop = element.scrollHeight;
  }, [text]);

  return (
    <div className="job-pane">
      <div className="job-bar">
        <span className={`job-status ${tone(job)}`}>{job.status}</span>
        <div className="spacer" />
        {job.running && (
          <button className="pill" onClick={onStop}>
            Stop
          </button>
        )}
      </div>
      {problem && <p className="dock-problem">{problem}</p>}
      <pre
        className="job-out"
        ref={view}
        tabIndex={0}
        aria-label={`Output of ${job.command}`}
        onScroll={() => {
          const element = view.current;
          if (!element) return;
          // A line of slack, so that a pane sitting at the bottom is not
          // knocked out of following by a rounding error in the scroll height.
          following.current =
            element.scrollHeight - element.scrollTop - element.clientHeight < 24;
        }}
      >
        {text ||
          (job.running
            ? "Nothing printed yet."
            : "This command printed nothing.")}
      </pre>
    </div>
  );
}
