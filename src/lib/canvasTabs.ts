/**
 * Which files the canvas has open, and which of them is showing.
 *
 * Pure and separate from the component for the reason `document.ts` is: these
 * are decisions, they can be wrong in ways nobody notices, and none of them
 * render anything.
 */

export type LineSpan = { from: number; to: number };

/** One open file. What it holds lives in its `CanvasSlot`, not here. */
export type CanvasTab = {
  /**
   * The name it was opened under, tidied by `samePath`. It identifies the tab,
   * so it never changes while the tab is open.
   */
  path: string;
  /** What the host calls it, once it has been read. */
  shown: string | null;
  /** An object so being sent to the same lines twice is two events. */
  reveal: LineSpan | null;
  /** When it was last the one showing, for choosing which to close. */
  used: number;
  /**
   * Whether it came back from the last time this folder was open, rather than
   * being opened since. One that can't be read any more closes instead of
   * saying so: nobody asked for it this time, and a file deleted while the app
   * was shut is an ordinary thing.
   */
  restored?: boolean;
};

export type CanvasTabs = { tabs: CanvasTab[]; active: string; clock: number };

/**
 * How many files stay open before the least recently used one closes.
 *
 * Every open file is held in memory and rendered, and the model can open files
 * as well as the person. Past this, a long turn would keep adding to the row
 * until the names were unreadable.
 */
export const MAX_TABS = 12;

/**
 * A path in the one form a tab is known by.
 *
 * `open_file` announces whatever the model wrote, so `./a.md`, `a.md` and the
 * absolute path can all arrive for one file. Each as its own tab would be two
 * buffers autosaving over each other, with each save refused as a change made by
 * somebody else.
 */
export function samePath(path: string, workspace: string | null): string {
  const slashed = path.replace(/\\/g, "/");
  const root = workspace?.replace(/\\/g, "/").replace(/\/+$/, "");
  const inside = root && slashed.startsWith(`${root}/`) ? slashed.slice(root.length + 1) : slashed;
  const rooted = inside.startsWith("/");
  const parts: string[] = [];
  for (const part of inside.split("/")) {
    if (part === "" || part === ".") continue;
    // A `..` with nothing to cancel stays, for the path guard to refuse.
    if (part === ".." && parts.length > 0 && parts[parts.length - 1] !== "..") parts.pop();
    else parts.push(part);
  }
  return `${rooted ? "/" : ""}${parts.join("/")}`;
}

const isTab = (tab: CanvasTab, path: string) => tab.path === path || tab.shown === path;

/**
 * Opens a file, or goes to it if it's already open.
 *
 * `quietly` opens it without changing what's showing, for the model opening a
 * file while somebody is typing in another. `keep` names the files that can't
 * close to make room, because they hold something the disk doesn't. When all of
 * them do, the row runs past the limit rather than lose any of it.
 */
export function openTab(
  state: CanvasTabs | null,
  path: string,
  reveal: LineSpan | null,
  { quietly = false, keep = () => false }: { quietly?: boolean; keep?: (path: string) => boolean } = {},
): CanvasTabs {
  const clock = (state?.clock ?? 0) + 1;
  const open = state?.tabs ?? [];
  const already = open.find((tab) => isTab(tab, path));
  const going = already?.path ?? path;
  const show = !state || !quietly;
  const active = show ? going : state.active;

  const stamped = (tab: CanvasTab): CanvasTab =>
    tab.path === going
      ? { ...tab, reveal: reveal ?? tab.reveal, used: show ? clock : tab.used }
      : tab;

  if (already) return { tabs: open.map(stamped), active, clock };

  const tabs = [...open, { path, shown: null, reveal, used: clock }];
  if (tabs.length > MAX_TABS) {
    const spare = tabs
      .filter((tab) => tab.path !== path && tab.path !== active && !keep(tab.path))
      .sort((a, b) => a.used - b.used)[0];
    if (spare) return { tabs: tabs.filter((tab) => tab !== spare), active, clock };
  }
  return { tabs, active, clock };
}

/** Shows a file that's already open. */
export function showTab(state: CanvasTabs, path: string): CanvasTabs {
  const clock = state.clock + 1;
  return {
    clock,
    active: path,
    tabs: state.tabs.map((tab) => (tab.path === path ? { ...tab, used: clock } : tab)),
  };
}

/**
 * Closes a file. `null` when it was the last one.
 *
 * The next one along shows, or the one before when it was at the end, so closing
 * several in a row walks the row instead of jumping about it.
 */
export function closeTab(state: CanvasTabs, path: string): CanvasTabs | null {
  const at = state.tabs.findIndex((tab) => tab.path === path);
  if (at < 0) return state;
  const tabs = state.tabs.filter((tab) => tab.path !== path);
  if (tabs.length === 0) return null;
  if (state.active !== path) return { ...state, tabs };
  return showTab({ ...state, tabs }, tabs[Math.min(at, tabs.length - 1)].path);
}

/**
 * Records what the host calls a file, once it has read it.
 *
 * Normally what it was opened under. When it isn't, and another tab already has
 * that name, the two are one file by routes `samePath` can't see, like a symlink.
 * This one closes and the other shows. It happens at the read, before there's any
 * typing here to lose.
 */
export function nameTab(state: CanvasTabs, path: string, shown: string): CanvasTabs | null {
  const twin = state.tabs.find((tab) => tab.path !== path && isTab(tab, shown));
  if (twin) {
    const wasShowing = state.active === path;
    const closed = closeTab(state, path);
    return closed && wasShowing ? showTab(closed, twin.path) : closed;
  }
  return {
    ...state,
    // Read, so it's an ordinary tab from here: a restored one that goes
    // missing later says so like any other.
    tabs: state.tabs.map((tab) => (tab.path === path ? { ...tab, shown, restored: undefined } : tab)),
  };
}

/**
 * What each tab says, keyed by its path.
 *
 * The filename, and as much of its folder as it takes to tell it from another of
 * the same name. A repository has a lot of `lib.rs`, and a row saying `lib.rs`
 * three times says nothing.
 */
export function tabLabels(paths: readonly string[]): Map<string, string> {
  const split = paths.map((path) => path.split("/").filter(Boolean));
  const labelAt = (i: number, depth: number) => split[i].slice(-depth).join("/");
  return new Map(
    paths.map((path, i) => {
      let depth = 1;
      while (
        depth < split[i].length &&
        split.some((_, other) => other !== i && labelAt(other, depth) === labelAt(i, depth))
      ) {
        depth += 1;
      }
      return [path, labelAt(i, depth) || path];
    }),
  );
}

/**
 * Which files each folder had open, kept on this machine so they come back
 * when the folder does.
 *
 * Only the paths and which one was showing. What was typed and not yet written
 * isn't kept: autosave has written it by the time anyone could quit, and a
 * draft kept here would be a second copy of the file to reconcile. Guarded the
 * way every remembered thing in the app is — see `sketchView` — because a
 * webview with site data blocked throws on the read as well as the write, and
 * a value an older version left behind can be anything.
 */
const KEY = "taurus.canvasTabs";

/** How many folders' tabs are kept. The ones that go are the least recently
 *  open, which nobody is coming back to tomorrow. */
const KEEP_FOLDERS = 50;

type Kept = Record<string, { paths: string[]; active: string; at: number }>;

/** Keeps what `workspace` has open, or forgets it when nothing is. */
export function rememberTabs(workspace: string, state: CanvasTabs | null): void {
  const all = readKept();
  if (!state || state.tabs.length === 0) {
    if (!(workspace in all)) return;
    delete all[workspace];
  } else {
    all[workspace] = { paths: state.tabs.map((tab) => tab.path), active: state.active, at: Date.now() };
    const folders = Object.keys(all);
    if (folders.length > KEEP_FOLDERS) {
      folders
        .sort((a, b) => all[a].at - all[b].at)
        .slice(0, folders.length - KEEP_FOLDERS)
        .forEach((old) => delete all[old]);
    }
  }
  try {
    localStorage.setItem(KEY, JSON.stringify(all));
  } catch {
    // Storage full or blocked: the tabs just don't come back next time.
  }
}

/**
 * The tabs `workspace` had open when it was last left, marked as restored, or
 * `null` if there are none.
 *
 * In the order they were in. The one showing then shows now; the rest count as
 * used in that order, so the oldest is still the first to close for room.
 */
export function recallTabs(workspace: string): CanvasTabs | null {
  const kept = readKept()[workspace];
  if (!kept) return null;
  const paths = [...new Set(kept.paths)].slice(0, MAX_TABS);
  if (paths.length === 0) return null;
  const tabs = paths.map((path, i) => ({ path, shown: null, reveal: null, used: i + 1, restored: true }));
  const active = paths.includes(kept.active) ? kept.active : paths[paths.length - 1];
  return { tabs, active, clock: paths.length };
}

function readKept(): Kept {
  try {
    if (typeof localStorage === "undefined") return {};
    const parsed: unknown = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: Kept = {};
    // Folder by folder, so one bad value costs its own folder's tabs only.
    for (const [folder, value] of Object.entries(parsed as Record<string, unknown>)) {
      const entry = value as { paths?: unknown; active?: unknown; at?: unknown } | null;
      if (!entry || typeof entry !== "object" || !Array.isArray(entry.paths)) continue;
      const paths = entry.paths.filter((p): p is string => typeof p === "string" && p !== "");
      out[folder] = {
        paths,
        active: typeof entry.active === "string" ? entry.active : "",
        at: Number(entry.at) || 0,
      };
    }
    return out;
  } catch {
    return {};
  }
}
