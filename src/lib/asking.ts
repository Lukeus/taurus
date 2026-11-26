import type { Entry } from "../state/store";

/**
 * Whether the turn is parked on questions for the user.
 *
 * Read off the last entry rather than searched for. A parked call is by
 * definition the newest thing in the transcript — the harness runs one call at
 * a time and this one is blocking it. See the comment on its re-export in
 * `App` for why the window needs to know.
 */
export function isAsking(entries: Entry[]): boolean {
  const last = entries[entries.length - 1];
  return (
    last?.kind === "tool" &&
    last.status === "running" &&
    last.view?.type === "questions"
  );
}
