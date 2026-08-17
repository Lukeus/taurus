/** Formatting shared by the shell. Nothing here talks to the backend. */

/** Unix seconds as something a person reads at a glance. */
export function when(seconds: number): string {
  const elapsed = Date.now() / 1000 - seconds;
  if (elapsed < 60) return "just now";
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m ago`;
  if (elapsed < 86400) return `${Math.floor(elapsed / 3600)}h ago`;
  if (elapsed < 604800) return `${Math.floor(elapsed / 86400)}d ago`;
  return new Date(seconds * 1000).toLocaleDateString();
}

/** True for a timestamp falling on today's calendar date, not within 24h. */
export function isToday(seconds: number): boolean {
  const then = new Date(seconds * 1000);
  const now = new Date();
  return (
    then.getFullYear() === now.getFullYear() &&
    then.getMonth() === now.getMonth() &&
    then.getDate() === now.getDate()
  );
}

/** A span in milliseconds, at the coarsest unit that still says something. */
export function duration(ms: number): string {
  if (ms < 1000) return `${Math.max(1, Math.round(ms / 100)) / 10}s`;
  if (ms < 60_000) return `${Math.round(ms / 1000)}s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return seconds === 0 ? `${minutes}m` : `${minutes}m ${seconds}s`;
}

/** The last segment of a path, on either platform's separator. */
export function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** The directory a path sits in, shortened with `~` where it is under home. */
export function parentDir(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  parts.pop();
  if (parts.length === 0) return "/";
  const prefix = path.startsWith("/") ? "/" : "";
  const dir = prefix + parts.join("/");
  // The host reports absolute paths; the rail has room for the tail of one.
  const home = dir.match(/^\/(?:Users|home)\/[^/]+(.*)$/);
  return home ? `~${home[1]}` : dir;
}

/**
 * `1 file`, `2 files` — the pluralisation this app keeps needing.
 *
 * `many` is for the nouns an `s` does not fix. There are few enough of those
 * that a rules engine would be more code than the words it replaced, and one
 * `plural(n, "directory", "directories")` at the call site is legible where a
 * lookup table two files away is not.
 */
export function plural(count: number, noun: string, many?: string): string {
  if (count === 1) return `${count} ${noun}`;
  return `${count} ${many ?? `${noun}s`}`;
}
