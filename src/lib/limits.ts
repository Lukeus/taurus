/**
 * Numbers the Rust side enforces, mirrored for the UI.
 *
 * Not generated: `ts-rs` exports types, not constants, and a Tauri round trip
 * for three integers that cannot change at runtime would be machinery in place
 * of a number. Mirrored by hand instead — but mirrored *once*, here, because
 * the same ceiling stated in four components is a ceiling that will eventually
 * be stated four different ways.
 *
 * These bound what a control offers. They are never the guarantee: the host
 * clamps or rejects whatever arrives, since `settings.json` and an agent's
 * frontmatter are both edited by hand and never pass through this file at all.
 */

/**
 * Longest an agent description may be.
 *
 * Matches `DESCRIPTION_LIMIT` in `taurus-agents`. It is paid on every request —
 * the roster sits in the spawn tool's description — so it is held to the same
 * 200 characters a skill's `when_to_use` is.
 */
export const DESCRIPTION_LIMIT = 200;

/**
 * Most iterations either kind of turn may take.
 *
 * Matches `MAX_ITERATIONS_LIMIT` in `taurus-agents`, and governs both places
 * iterations are counted: a parent turn's `max_iterations` in `settings.json`
 * and a sub-agent's in its frontmatter.
 */
export const MAX_ITERATIONS_LIMIT = 100;

/** What a parent turn gets when nobody has said otherwise. */
export const DEFAULT_MAX_ITERATIONS = 25;

/** What a newly drafted sub-agent gets. Matches `default_max_iterations`. */
export const DEFAULT_AGENT_ITERATIONS = 20;

/**
 * Pulls a typed number into range, keeping `fallback` for anything unreadable.
 *
 * Shared because both fields that take one of these want the same behaviour:
 * an empty box is on the way to typing a number, not a request for a turn that
 * cannot take a step.
 */
export function clampIterations(text: string, fallback: number): number {
  const parsed = Number.parseInt(text, 10);
  if (Number.isNaN(parsed)) return fallback;
  return Math.min(Math.max(parsed, 1), MAX_ITERATIONS_LIMIT);
}
