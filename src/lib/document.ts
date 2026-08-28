/**
 * What the canvas does when a file changes under it.
 *
 * Pure, and separate from the component for the reason `onScreenFor` is: the
 * decisions are the interesting part, they are the part that can be wrong in a
 * way nobody notices, and none of them render anything.
 *
 * There are two writers — the person typing and the model's `write_file` — and
 * neither waits for the other. Everything here is about the moment they meet.
 */

/** A document as the editor holds it: what was read, and what has been typed. */
export interface Buffer {
  /** The text as last read from or written to disk. */
  base: string;
  /** What is in the editor now. Equal to `base` when nothing has been typed. */
  draft: string;
}

export type Reconciliation =
  /** Nothing to do: what arrived is what we already had. */
  | { kind: "same" }
  /** Take the new text. The buffer had no unsaved work to lose. */
  | { kind: "reload" }
  /**
   * Keep both and ask. The person has typed something the new text would
   * destroy, and choosing for them is the one thing that must not happen here.
   */
  | { kind: "conflict" };

/**
 * What to do with a file that has changed on disk while it was open.
 *
 * The rule in one line: **never silently lose typing.** A clean buffer has
 * nothing to lose, so it takes the new version and the reader watches the model
 * edit their document. A dirty one does not, and the answer is a question.
 *
 * `same` is checked first and is not an optimisation. `files_changed` names
 * every path a turn touched, and a turn that rewrote a file to the same bytes —
 * a formatter, an idempotent edit — would otherwise flash the whole document as
 * changed, or worse, raise a conflict over two identical texts.
 */
export function reconcile(buffer: Buffer, incoming: string): Reconciliation {
  if (incoming === buffer.draft) return { kind: "same" };
  if (buffer.draft === buffer.base) return { kind: "reload" };
  return { kind: "conflict" };
}

/** Whether anything has been typed that is not on disk. */
export function dirty(buffer: Buffer): boolean {
  return buffer.draft !== buffer.base;
}

/**
 * The span of lines that differ between two versions, 1-based and inclusive.
 *
 * One region, found by trimming the lines both versions agree on from each end.
 * That is not a diff and does not pretend to be: two edits far apart come back
 * as one region covering everything between them, and an edit that changes
 * nothing but the order of two blocks covers both.
 *
 * The same shape `intraline.ts` takes for the same reason — this exists to
 * *point*, not to explain. What it drives is a scroll and a brief tint, and for
 * both of those a region that is too large is a smaller error than a diff
 * algorithm that is subtly wrong about which line moved.
 *
 * `null` when the two are identical, which the caller reads as "do not flash".
 */
export function changedLines(
  before: string,
  after: string,
): { from: number; to: number } | null {
  if (before === after) return null;
  const was = before.split("\n");
  const now = after.split("\n");

  let head = 0;
  while (head < was.length && head < now.length && was[head] === now[head]) head++;

  // Counted from the end, and stopped before it can cross the head — otherwise
  // a file with repeated lines double-counts the ones in the middle and the
  // region comes out inverted.
  let tail = 0;
  while (
    tail < was.length - head &&
    tail < now.length - head &&
    was[was.length - 1 - tail] === now[now.length - 1 - tail]
  ) {
    tail++;
  }

  const from = head + 1;
  const to = Math.max(from, now.length - tail);
  return { from, to };
}

/**
 * How long the editor waits after the last keystroke before writing.
 *
 * Long enough that a sentence is one save rather than forty, short enough that
 * putting the question to the model is never a save away — because the whole
 * argument for saving at all rather than asking is that what the model reads is
 * what is on the screen. Anything past about a second and that stops being
 * reliably true.
 */
export const SAVE_AFTER_MS = 800;

/**
 * How long a changed passage stays tinted after somebody else writes it.
 *
 * Long enough to catch out of the corner of an eye while reading something
 * else, short enough that it is gone before it becomes part of how the document
 * looks. Cleared on this timer rather than on the animation ending, so nothing
 * depends on an event that does not fire when motion is turned off.
 */
export const FLASH_MS = 1_800;
