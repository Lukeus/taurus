import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "../lib/api";
import type { Page, PageKind, PageRef, Scope } from "../lib/api";
import { reconcile, SAVE_AFTER_MS } from "../lib/document";
import { basename } from "../lib/format";

/**
 * The notebook, as one screen's worth of state.
 *
 * # Why this lives above the pane
 *
 * The Data pane is unmounted on every switch back to the conversation, on
 * purpose: it holds a pending query, and a token held down there would re-run on
 * the way back. A notes pane holds something else — a half-typed paragraph, a
 * drawing half made — and throwing that away on a glance at the transcript would
 * be the worst bug this feature could have.
 *
 * So the state is a hook called once in `App`, which never unmounts, and
 * `NotesPane` is a view over it. That is the canvas's arrangement for the same
 * reason: `App` holds `doc`, `typed`, `saveState` and `conflict` so that closing
 * the editor is not the same as discarding what is in it.
 *
 * # Why it is a hook and not part of the store
 *
 * Everything in `store.ts` is about a conversation — turns, tools, permissions,
 * checkpoints. A notebook outlives every conversation in the workspace and takes
 * no part in one. Putting it there would make the session store the place where
 * unrelated state goes.
 *
 * # Notes and sketches are one thing here
 *
 * A sketch's file is Excalidraw's JSON rather than Markdown, and that is the
 * whole of the difference this file sees. The save loop, the compare-and-swap,
 * the conflict and both reconcile paths are the same code for either — `typed`
 * is whatever text is in the editor, and for a sketch it is the scene.
 */

/** Everything needed to name one note or sketch. `Page` and `PageRef` satisfy it. */
export type Which = { scope: Scope; kind: PageKind; name: string };

/** How a note is shown: the Markdown, or what it renders as. */
export type NoteMode = "write" | "read";

export type SaveState = "idle" | "typing" | "saving" | "failed";

/** How a flush went. `conflict` is one already open, which no save can settle. */
export type Flushed = "clean" | "written" | "stale" | "failed" | "conflict";

/**
 * A version of somebody's, typed into a file they have since left and not yet
 * written — see `hold`. `base` is the file as it was when that typing began,
 * and `conflict` whether it was left because the file changed underneath it
 * rather than because a save failed.
 */
type Kept = { base: Page; draft: string; conflict: boolean };

/**
 * What a kept version is filed under. A project note's includes its folder, so
 * a version typed in one repository is offered back only in that repository —
 * and is there again when it is reopened.
 */
function keyOf(which: Which, workspace: string | null): string {
  const folder = which.scope === "workspace" ? (workspace ?? "") : "";
  return `${which.scope}\u0000${folder}\u0000${which.kind}\u0000${which.name}`;
}

export type Notebook = ReturnType<typeof useNotebook>;

export function useNotebook({
  /**
   * Increments whenever a turn wrote files, with the paths it touched.
   *
   * The project notebook is in the workspace, so `write_note` and `write_file`
   * can both reach a note in it. Without this the editor would go on showing
   * text the file no longer has, until the next save was refused for a conflict
   * the reader had no way to see coming.
   */
  wrote,
  /**
   * Whether a turn is running.
   *
   * Watched for the moment it stops. A turn can create a note the list has
   * never heard of, and it can change a *global* note — which is outside the
   * workspace, so `wrote` never names it. The end of a turn is the one moment
   * both are known to have finished.
   */
  busy,
  /**
   * The folder that is open, or `null`.
   *
   * Watched for a switch. A project note belongs to the folder it was read
   * from, and the host's notebook moves with the workspace the moment
   * `set_workspace` returns — see the effect that watches this.
   */
  workspace,
  onError,
}: {
  wrote: { at: number; paths: string[] } | null;
  busy: boolean;
  workspace: string | null;
  onError: (message: string) => void;
}) {
  const [pages, setPages] = useState<PageRef[] | null>(null);
  /** Which note or sketch is open, or `null` for the list with nothing chosen. */
  const [open, setOpen] = useState<Which | null>(null);
  /** The file as last read from or written to disk. */
  const [page, setPage] = useState<Page | null>(null);
  /**
   * What is in the editor, which is not always what is on disk. Held beside
   * `page` because the difference between the two *is* the unsaved state.
   */
  const [typed, setTyped] = useState("");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  /** The other version, when a save was refused because somebody got there
   *  first. Both are kept; neither is chosen here. */
  const [conflict, setConflict] = useState<Page | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [mode, setMode] = useState<NoteMode>("write");
  /**
   * Moves whenever the editor's text is replaced from disk rather than typed.
   *
   * A prose editor can simply be handed new text. Excalidraw cannot: it reads
   * its scene once and then owns it, with the undo history and the tool in
   * hand. So it is told to read again by this moving — on opening, on taking
   * the other version, and when a turn rewrote a sketch nobody was drawing in —
   * and never by a save, which changes the file and must not reset the canvas
   * under somebody's pen.
   */
  const [generation, setGeneration] = useState(0);
  /** Versions kept for files that were left unwritten, by `keyOf`. */
  const [kept, setKept] = useState<Record<string, Kept>>({});

  /*
   * The same values, for code that runs after the render it was made in — a
   * save answered after a switch, a flush run from a click. Each needs what is
   * open now rather than what was open when it was created. Assigned during
   * render, the way `SketchEditor` holds its `onChange`.
   */
  const live = useRef({ open, page, typed, conflict, kept, workspace });
  live.current = { open, page, typed, conflict, kept, workspace };
  /**
   * Which opening of a file the editor is on. It moves the moment the editor
   * leaves one — see `go` — and a save's answer is applied only if it has not
   * moved since the save began. Moved on leaving rather than on the next read
   * arriving, because a save's answer can land between the two: applied there,
   * a refusal became a conflict over the *next* file, whose **Keep mine** wrote
   * the next file's text into the one that had been left.
   */
  const session = useRef(0);
  /** The autosave waiting out its debounce, so `flush` can run it now instead. */
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** The save in flight, so a flush asking for the same one waits for it rather
   *  than sending it again against a fingerprint it is about to change. */
  const saving = useRef<{ page: Page; text: string; done: Promise<Flushed> } | null>(null);
  /** Set by the sketch editor while it is mounted — see its `drain`. */
  const drain = useRef<(() => void) | null>(null);
  // Held rather than depended on. `App` passes a new function on every render,
  // and an effect keyed on it would restart the autosave with each one.
  const report = useRef(onError);
  report.current = onError;

  /** Moves the editor to another file, or to none. The one place `open` is
   *  set, so the one place a session ends. */
  const go = useCallback((next: Which | null) => {
    session.current += 1;
    setOpen(next && { scope: next.scope, kind: next.kind, name: next.name });
  }, []);

  /**
   * Keeps what was typed into a file that is being left and cannot be written:
   * a conflict still waiting for a choice, or a save that came back refused or
   * failed after the editor had moved on.
   *
   * Leaving is not a decision about whose version survives, so it must not
   * make one. The version is kept against the file until it is opened again —
   * which reads it afresh and asks, or simply saves it if nothing else has
   * written the file since — or until the file is deleted. The list marks it
   * meanwhile. In memory only: see `docs/known-gaps.md`.
   */
  const hold = useCallback((base: Page, draft: string, conflict: boolean) => {
    const key = keyOf(base, live.current.workspace);
    setKept((all) => ({ ...all, [key]: { base, draft, conflict } }));
  }, []);

  const release = useCallback((which: Which) => {
    const key = keyOf(which, live.current.workspace);
    setKept((all) => {
      if (!(key in all)) return all;
      const rest = { ...all };
      delete rest[key];
      return rest;
    });
  }, []);

  const refresh = useCallback(async () => {
    try {
      setPages(await api.listPages());
    } catch (e) {
      setPages([]);
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /*
   * Reads whatever is open.
   *
   * Keyed on the three names rather than on the object, so re-choosing the one
   * that is already open does not throw away what has been typed into it.
   */
  const scope = open?.scope ?? null;
  const kind = open?.kind ?? null;
  const name = open?.name ?? null;
  useEffect(() => {
    if (scope === null || kind === null || name === null) {
      setPage(null);
      setTyped("");
      setConflict(null);
      return;
    }
    let current = true;
    setError(null);
    setConflict(null);
    api
      .readPage(scope, kind, name)
      .then((read) => {
        if (!current) return;
        const held = live.current.kept[keyOf(read, live.current.workspace)];
        if (!held) {
          setPage(read);
          setTyped(read.text);
        } else {
          // The writer's own version was kept when this was left. Put back,
          // and asked about against the file as it is now, not as it was then.
          release(read);
          if (read.text === held.draft) {
            // Whoever wrote it wrote the same thing.
            setPage(read);
            setTyped(read.text);
          } else if (read.fingerprint === held.base.fingerprint) {
            // Nothing has written it since the typing began: the kept version
            // simply saves.
            setPage(read);
            setTyped(held.draft);
          } else {
            setPage(held.base);
            setTyped(held.draft);
            setConflict(read);
          }
        }
        setSaveState("idle");
        setGeneration((g) => g + 1);
      })
      .catch((e) => {
        if (!current) return;
        setPage(null);
        setError(String(e));
      });
    return () => {
      current = false;
    };
  }, [scope, kind, name, release]);

  /*
   * Writes what has been typed, once typing has stopped.
   *
   * The canvas's bargain, and the same debounce: autosaved rather than left to
   * ⌘S, because a note nobody remembered to save is a note the next
   * conversation cannot be asked about. Nothing runs while a conflict is open —
   * a save is exactly what is being decided about.
   */
  useEffect(() => {
    if (!page || conflict || typed === page.text) return;
    // Being left: its last words went with the flush that left it, and a second
    // save against the same stamp would be refused by the first.
    if (!same(open, page)) return;
    setSaveState("typing");
    const pending = setTimeout(() => {
      timer.current = null;
      void write(page, typed);
    }, SAVE_AFTER_MS);
    timer.current = pending;
    return () => {
      clearTimeout(pending);
      if (timer.current === pending) timer.current = null;
    };
    // `write` is stable, so this restarts on typing and nothing else.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [typed, page, conflict, open]);

  /**
   * Writes `text` over the file `target` was read as.
   *
   * The answer is applied only if the same opening of the same file is still
   * in the editor — see `session`. A save started on one note and answered
   * after another had opened is about a file no longer on screen: applied, it
   * made the old note current again under the new one's text, and the next
   * autosave wrote that text into the old file. The outcome is returned either
   * way, for the one caller that has to know — a rename.
   */
  const write = useCallback(
    (target: Page, text: string): Promise<Flushed> => {
      const underway = saving.current;
      if (underway && underway.page === target && underway.text === text) return underway.done;
      const mine = session.current;
      const here = () => session.current === mine;
      setSaveState("saving");
      const done = api
        .savePage(target.scope, target.kind, target.name, text, target.fingerprint)
        .then((result): Flushed => {
          if (result.type === "stale") {
            if (here()) {
              // Not an error. Somebody wrote the file after this editor read
              // it, so both versions exist and which one survives is not a
              // decision this code gets to make.
              setConflict(result.current);
              setSaveState("typing");
            } else {
              // The file has been left, so there is no editor to ask in. Kept
              // for when it is opened again, and said, since the only sign on
              // screen is a word in a row of the list.
              hold(target, text, true);
              report.current(
                `"${target.name}" changed on disk while you were working on it, so your version was kept rather than saved. Open it again to choose between them.`,
              );
            }
            return "stale";
          }
          if (here()) {
            setPage(result.page);
            setSaveState("idle");
          }
          // The list carries a size and a timestamp, and this changed both.
          void refresh();
          return "written";
        })
        .catch((e): Flushed => {
          if (here()) {
            setSaveState("failed");
            report.current(String(e));
          } else {
            hold(target, text, false);
            report.current(
              `${String(e)} Your version of "${target.name}" is kept — open it again to save it.`,
            );
          }
          return "failed";
        })
        .finally(() => {
          if (saving.current?.done === done) saving.current = null;
        });
      saving.current = { page: target, text, done };
      return done;
    },
    [refresh, hold],
  );

  /**
   * Writes now whatever is waiting to be written.
   *
   * Run before anything that takes the editor off the file it is on: choosing
   * another, making one, renaming this one, switching workspace. The debounce
   * is right while somebody is typing and wrong the moment they leave —
   * cancelled by the switch, it took the last sentence with it. A sketch's last
   * stroke is asked for first, because it waits on a debounce of its own.
   *
   * `leaving` when the editor is about to move off the file: a conflict still
   * waiting for a choice is then kept rather than dropped — see `hold`. A
   * rename passes nothing, because it refuses instead and stays.
   */
  const flush = useCallback(async (leaving = false): Promise<Flushed> => {
    drain.current?.();
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    const { open, page, typed, conflict } = live.current;
    // Nothing open, or already being left — which was flushed when it was.
    if (!page || !same(open, page)) return "clean";
    if (conflict) {
      if (leaving) hold(page, typed, true);
      return "conflict";
    }
    if (typed === page.text) return "clean";
    const outcome = await write(page, typed);
    // Refused or failed on the way out. `write` keeps the text when its answer
    // arrives after the editor has moved on; this answer arrived before, while
    // the file was still on screen, and the caller is about to move it — to
    // another note, or to another folder, where this one's editor is closed.
    // Nothing would be left holding the text, so it is kept the same way.
    if (leaving && (outcome === "stale" || outcome === "failed")) {
      hold(page, typed, outcome === "stale");
    }
    return outcome;
  }, [write, hold]);

  /** Opens a file, having written what was typed into the one being left. */
  const choose = useCallback(
    (which: Which) => {
      // Re-choosing the file that is open is nothing, rather than a re-read
      // that would throw away what has been typed into it.
      if (same(live.current.open, which)) return;
      void flush(true);
      go(which);
    },
    [flush, go],
  );

  /**
   * What was typed into the editor of `which`.
   *
   * Named, because an editor can speak after its file has been left: the
   * sketch editor hands over a stroke still waiting to be serialised when it
   * unmounts, and by then the pane may be showing another file. Taken, it would
   * become that file's text — and be saved over it.
   */
  const edit = useCallback((text: string, which: Which) => {
    if (!same(which, live.current.page)) return;
    live.current.typed = text;
    setTyped(text);
  }, []);

  /**
   * Takes a version of the open file that arrived from somewhere else.
   *
   * `reconcile`'s rule, which is never to silently lose typing: a clean editor
   * takes the new version, and a dirty one keeps both and asks. Shared by the
   * two routes a change arrives by below.
   */
  const arrive = useCallback(
    (read: Page, base: Page, draft: string) => {
      const what = reconcile({ base: base.text, draft }, read.text);
      if (what.kind === "same") {
        // Still take the new fingerprint: the bytes match, but the file has
        // been rewritten, and saving against the old stamp would be refused
        // for a conflict that does not exist.
        setPage(read);
        return;
      }
      if (what.kind === "conflict") {
        setConflict(read);
        return;
      }
      setPage(read);
      setTyped(read.text);
      setGeneration((g) => g + 1);
    },
    [],
  );

  /*
   * A turn wrote the project file that is open.
   *
   * Only project files arrive this way — `files_changed` reports
   * workspace-relative paths, and a global note is not in the workspace.
   */
  const wroteAt = wrote?.at;
  useEffect(() => {
    if (!wrote || !page || page.scope !== "workspace") return;
    if (!wrote.paths.includes(pathOf(page.name, page.kind))) return;
    let current = true;
    api
      .readPage(page.scope, page.kind, page.name)
      .then((read) => {
        if (current) arrive(read, page, typed);
      })
      .catch(() => {
        // A file the turn deleted. Left as it was rather than blanked — what is
        // on screen is still what was there, and an empty editor would be a lie
        // about the file.
      });
    return () => {
      current = false;
    };
    // Only the counter, so a re-render mid-turn does not re-read the file.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wroteAt]);

  /*
   * A turn finished.
   *
   * The list is read again, since a turn can make a note nobody has seen yet.
   * And a *global* file that is open is read again, since a turn can change one
   * without `wrote` ever naming it. A project one needs neither: `wrote` already
   * covered it, the moment it happened.
   */
  const wasBusy = useRef(busy);
  useEffect(() => {
    const finished = wasBusy.current && !busy;
    wasBusy.current = busy;
    if (!finished) return;
    void refresh();
    if (!page || page.scope !== "global" || conflict) return;
    let current = true;
    api
      .readPage(page.scope, page.kind, page.name)
      .then((read) => {
        if (current && read.fingerprint !== page.fingerprint) arrive(read, page, typed);
      })
      .catch(() => {});
    return () => {
      current = false;
    };
    // Only the transition matters, and the values it reads are the ones current
    // at that moment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy]);

  /*
   * The workspace switched.
   *
   * The list is read again, because its project half is a different folder's
   * now. And a project file that was open is closed rather than kept: the host
   * already answers for the new folder, so the next autosave would have landed
   * in whatever file there has the same name — or been refused as a conflict
   * whose **Keep mine** overwrote it. What was typed has been written by then,
   * because `App` flushes before it asks for the switch. A global file stays
   * open; it belongs to neither folder.
   */
  const seenWorkspace = useRef(workspace);
  useEffect(() => {
    if (seenWorkspace.current === workspace) return;
    const previous = seenWorkspace.current;
    seenWorkspace.current = workspace;
    void refresh();
    // Versions kept in the folder being left are kept for when it is back, and
    // said, because the rows that would have marked them are gone from the list.
    const left = Object.keys(live.current.kept).filter((key) =>
      key.startsWith(`workspace\u0000${previous ?? ""}\u0000`),
    ).length;
    if (left > 0 && previous) {
      report.current(
        `Your unsaved version of ${left === 1 ? "a project note" : `${left} project notes`} is kept for when ${basename(previous)} is open again, until this window closes.`,
      );
    }
    if (live.current.open?.scope !== "workspace") return;
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    go(null);
  }, [workspace, refresh, go]);

  const create = useCallback(
    async (scope: Scope, kind: PageKind, name: string) => {
      // Refused names come back from the host, which is the only place that can
      // know whether one is taken. Thrown rather than swallowed: the box that
      // asked for the name is where the message belongs.
      const made = await api.createPage(scope, kind, name);
      setPages(await api.listPages());
      void flush(true);
      go(made);
      setMode("write");
      return made;
    },
    [flush, go],
  );

  // Takes only what it addresses a file by, so a `Page` from the editor and a
  // `PageRef` from the list both fit without either being converted.
  const rename = useCallback(
    async (from: Which, to: string) => {
      // Written under the name it was typed under before the file moves, since
      // it is then read afresh under the new one. A save that did not land is a
      // reason not to move it: what was typed would stay behind with the old
      // editor, and nothing would say so.
      if (same(from, live.current.page)) {
        const saved = await flush();
        if (saved === "conflict" || saved === "stale") {
          throw new Error(
            `Choose a version first — this ${from.kind} changed while you were working on it.`,
          );
        }
        if (saved === "failed") {
          throw new Error("It was not renamed, because your latest changes to it could not be saved.");
        }
      }
      const moved = await api.renamePage(from.scope, from.kind, from.name, to);
      setPages(await api.listPages());
      go(moved);
      return moved;
    },
    [flush, go],
  );

  const forget = useCallback(async (target: Which) => {
    try {
      // An autosave still waiting out its debounce is about a file that is on
      // its way out, and has nothing left to write to.
      if (same(live.current.page, target) && timer.current !== null) {
        clearTimeout(timer.current);
        timer.current = null;
      }
      setPages(await api.forgetPage(target.scope, target.kind, target.name));
      // A version kept for a file that is gone has nothing left to be about.
      release(target);
      if (same(live.current.open, target)) go(null);
    } catch (e) {
      report.current(String(e));
    }
  }, [go, release]);

  /** Saves what is in the editor over the other version. */
  const keepMine = useCallback(() => {
    if (!conflict || !page) return;
    // Adopt the *fingerprint* the refusal handed back while keeping the old text
    // as the base. The stamp is what makes the next save succeed where the
    // refused one did not; the stale base is what keeps `typed !== page.text`,
    // so the autosave above still has something to do. Setting the base to what
    // was typed instead makes the buffer look clean and nothing is ever written
    // — which is exactly what this did until a test pressed the button twice.
    setPage({ ...conflict, text: page.text });
    setConflict(null);
    setSaveState("typing");
  }, [conflict, page]);

  /** Throws away what is in the editor and takes the other version. */
  const takeTheirs = useCallback(() => {
    if (!conflict) return;
    setPage(conflict);
    setTyped(conflict.text);
    setConflict(null);
    setSaveState("idle");
    setGeneration((g) => g + 1);
  }, [conflict]);

  /**
   * What the pane is showing, for `OnScreen`.
   *
   * A note only. The model reads a note with `read_note`, and what it could be
   * told about a sketch on screen it cannot act on — there is no tool that
   * reads one, because there is no picture of one to read. A sketch reaches the
   * model when a note embeds it, as the words written on it.
   *
   * Deliberately not the text, either way: a note is a file the model can read,
   * and sending a whole one with every message from a pane somebody leaves open
   * would be the most expensive habit in the app.
   */
  /** Whether a file has a version of somebody's kept for it — see `hold`. */
  const keptAs = useCallback(
    (which: Which): "conflict" | "unsaved" | null => {
      const held = kept[keyOf(which, workspace)];
      if (!held) return null;
      return held.conflict ? "conflict" : "unsaved";
    },
    [kept, workspace],
  );

  const onScreen = useMemo(
    () =>
      page && page.kind === "note"
        ? { scope: page.scope, name: page.name, unsaved: typed !== page.text }
        : null,
    [page, typed],
  );

  return {
    pages,
    open,
    page,
    typed,
    saveState,
    conflict,
    error,
    mode,
    generation,
    onScreen,
    refresh,
    choose,
    edit,
    flush,
    drain,
    keptAs,
    setMode,
    create,
    rename,
    forget,
    keepMine,
    takeTheirs,
  };
}

/** Whether two names point at the same file. */
export function same(a: Which | null, b: Which | null): boolean {
  return !!a && !!b && a.scope === b.scope && a.kind === b.kind && a.name === b.name;
}

/**
 * Where a project file sits, workspace-relative.
 *
 * Written here rather than sent from the host because it is only ever compared
 * against `files_changed`, which speaks in these paths. Forward slashes on every
 * platform, the way every path the user sees is written.
 */
export function pathOf(name: string, kind: PageKind): string {
  return `.taurus/notes/${name}.${kind === "note" ? "md" : "excalidraw"}`;
}

/**
 * The line that puts a sketch into a note.
 *
 * An ordinary Markdown image, so the note stays Markdown any other viewer can
 * show. The angle brackets are what a name with a space in it needs, and they
 * are harmless around a name without one — so they are always there, and the
 * line is the same shape whatever the sketch is called.
 */
export function embedFor(name: string): string {
  return `![${name}](<${name}.excalidraw>)`;
}
