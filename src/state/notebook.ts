import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "../lib/api";
import type { Page, PageKind, PageRef, Scope } from "../lib/api";
import { reconcile, SAVE_AFTER_MS } from "../lib/document";

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
  onError,
}: {
  wrote: { at: number; paths: string[] } | null;
  busy: boolean;
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
      return;
    }
    let current = true;
    setError(null);
    setConflict(null);
    api
      .readPage(scope, kind, name)
      .then((read) => {
        if (!current) return;
        setPage(read);
        setTyped(read.text);
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
  }, [scope, kind, name]);

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
    setSaveState("typing");
    const timer = setTimeout(() => {
      setSaveState("saving");
      api
        .savePage(page.scope, page.kind, page.name, typed, page.fingerprint)
        .then((result) => {
          if (result.type === "stale") {
            // Not an error. Somebody wrote the file after this editor read it,
            // so both versions exist and which one survives is not a decision
            // this code gets to make.
            setConflict(result.current);
            setSaveState("typing");
            return;
          }
          setPage(result.page);
          setSaveState("idle");
          // The list carries a size and a timestamp, and this changed both.
          void refresh();
        })
        .catch((e) => {
          setSaveState("failed");
          onError(String(e));
        });
    }, SAVE_AFTER_MS);
    return () => clearTimeout(timer);
    // `refresh` and `onError` are stable; depending on them would restart the
    // timer on every render and a fast typist would never reach the end of one.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [typed, page, conflict]);

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

  const create = useCallback(async (scope: Scope, kind: PageKind, name: string) => {
    // Refused names come back from the host, which is the only place that can
    // know whether one is taken. Thrown rather than swallowed: the box that
    // asked for the name is where the message belongs.
    const made = await api.createPage(scope, kind, name);
    setPages(await api.listPages());
    setOpen({ scope: made.scope, kind: made.kind, name: made.name });
    setMode("write");
    return made;
  }, []);

  // Takes only what it addresses a file by, so a `Page` from the editor and a
  // `PageRef` from the list both fit without either being converted.
  const rename = useCallback(async (from: Which, to: string) => {
    const moved = await api.renamePage(from.scope, from.kind, from.name, to);
    setPages(await api.listPages());
    setOpen({ scope: moved.scope, kind: moved.kind, name: moved.name });
    return moved;
  }, []);

  const forget = useCallback(
    async (target: Which) => {
      try {
        setPages(await api.forgetPage(target.scope, target.kind, target.name));
        if (same(open, target)) setOpen(null);
      } catch (e) {
        onError(String(e));
      }
    },
    [open, onError],
  );

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
    choose: setOpen,
    edit: setTyped,
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
