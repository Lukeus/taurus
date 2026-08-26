/**
 * The pieces of the background-command tabs that are arithmetic rather than
 * DOM.
 *
 * Split out for the reason `terminal.ts` is: what a pane does with the answers
 * it polls for is worth testing on its own, and testing it should not mean
 * mounting the dock.
 */
import type { BackgroundJob, JobOutput } from "./api";

/**
 * How much of one command's output the pane keeps.
 *
 * The same number the host holds, because holding more here would be holding
 * what the host has already forgotten: this is filled by polling that buffer,
 * so a longer scrollback would only ever be as long as the shortest of the
 * two. See `MAX_PENDING_BYTES` in `crates/taurus-tools/src/jobs.rs`.
 *
 * Characters against the host's bytes, which for anything but plain ASCII
 * means this holds slightly more than the host does. That is the harmless
 * direction: it can never hold *less* than what arrived.
 */
export const PANE_LIMIT = 256 * 1024;

/** How many characters of a command line a tab shows. */
const LABEL = 22;

/**
 * A command line as a tab reads it.
 *
 * Whitespace collapsed first, because a command built across several lines
 * arrives with the newlines in it and a tab is one line high. The number is
 * added by the caller rather than here: it is what makes two `cargo build`s
 * tell apart, and it belongs to the job rather than to its text.
 */
export function title(command: string): string {
  const flat = command.trim().replace(/\s+/g, " ");
  if (flat.length <= LABEL) return flat;
  return `${flat.slice(0, LABEL - 1).trimEnd()}…`;
}

/**
 * A byte count as somebody would say it.
 *
 * Only ever used for output that went missing, where the exact number is not
 * the point — how much of it there was, is.
 */
export function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * What the pane holds after one poll.
 *
 * The gap is written into the text rather than reported beside it, because it
 * happened *there*: a note in a corner saying some output is missing leaves
 * the reader to guess which lines do not follow from the ones above them. This
 * is the only thing the pane adds to what the command wrote, and it is marked
 * as an addition by being the one line in brackets.
 */
export function extend(held: string, output: JobOutput): string {
  let next = held;
  if (output.missed > 0) {
    if (next && !next.endsWith("\n")) next += "\n";
    next += `[${size(output.missed)} of output not shown]\n`;
  }
  next += output.text;
  // Trimmed from the front, and to a line: cutting mid-line would put half a
  // line at the top of the pane looking like something the command wrote.
  if (next.length > PANE_LIMIT) {
    const cut = next.length - PANE_LIMIT;
    const line = next.indexOf("\n", cut);
    next = next.slice(line === -1 ? cut : line + 1);
  }
  return next;
}

/**
 * How a finished command ended, in one character.
 *
 * Beside the colour rather than instead of it, for the reason the diff marks
 * its changed side with a `+` as well as a green: a strip where a failed build
 * and a running one differ only in hue says nothing on a projector, in a
 * screenshot, or to a reader who cannot tell the two apart. A running command
 * is the unmarked case — the tab being there at all is the news.
 *
 * It sits beside the number rather than after the command, where it read as
 * punctuation somebody had typed.
 */
export function mark(job: BackgroundJob): string {
  if (job.running) return "";
  // A stop has no verdict in it — the command was ended, not judged — so it
  // gets neither of the two marks that are one.
  if (job.stopped) return "–";
  return job.code === 0 ? "✓" : "✗";
}

/**
 * How a job's state reads on its own row.
 *
 * The sentence itself comes from the host — see `say` in
 * `crates/taurus-tools/src/jobs.rs` — so that the window and `check_command`
 * cannot describe one command two ways. This only picks which of three ways to
 * draw it.
 */
export function tone(job: BackgroundJob): "running" | "failed" | "done" {
  if (job.running) return "running";
  if (job.stopped) return "done";
  return job.code === 0 ? "done" : "failed";
}
