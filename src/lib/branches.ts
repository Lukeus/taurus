import type { ForkedFrom, SessionMeta } from "./api";

/**
 * Where a fork was made, as `ForkedFrom::place` says it: "at turn 3", or for a
 * turn that changed no files, where it falls between the ones that did.
 */
export function forkPlace(from: ForkedFrom): string {
  if (!from.read_only) return `at turn ${from.checkpoint}`;
  return from.checkpoint === 1 ? "before any changes" : `after turn ${from.checkpoint - 1}`;
}

/**
 * The other branches of `id`'s conversation: every conversation in `sessions`
 * that shares a root with it, the one everything was forked from included.
 * The same family `taurus_host::fork` switches and compares within, read from
 * the rail's listing rather than asked for again.
 */
export function branchesOf(sessions: SessionMeta[], id: string): SessionMeta[] {
  const parent = new Map<string, string>();
  for (const session of sessions) {
    if (session.forked_from) parent.set(session.id, session.forked_from.session);
  }
  const root = (start: string) => {
    let at = start;
    // Bounded, as the backend's is, so a hand-edited loop can't hang the pane.
    for (let i = 0; i <= sessions.length && parent.has(at); i++) at = parent.get(at)!;
    return at;
  };
  const mine = root(id);
  return sessions.filter((session) => session.id !== id && root(session.id) === mine);
}

/** How a branch is named in a list of them: where it was forked, or that it's
 *  the one the others came from. */
export function branchLabel(session: SessionMeta): string {
  return session.forked_from ? `fork ${forkPlace(session.forked_from)}` : "the original";
}
