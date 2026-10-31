import { useEffect, useRef, useState, type MutableRefObject } from "react";

import * as api from "../lib/api";
import type { Document, Selection } from "../lib/api";
import { changedLines, FLASH_MS, reconcile, SAVE_AFTER_MS } from "../lib/document";
import { useStore } from "../state/store";
import { Canvas } from "./Canvas";
import type { SaveState } from "../state/notebook";
import { ResizeHandle, type Resizable } from "./ResizeHandle";

/**
 * What was typed into a file that was left and couldn't be written.
 *
 * The notebook's rule, and for the same reason: leaving isn't a decision about
 * whose version survives. Outside the component because closing the canvas
 * unmounts it, and in memory only, so it lasts as long as the window.
 */
type Kept = { base: Document; draft: string };
const kept = new Map<string, Kept>();
const keyOf = (workspace: string | null, path: string) => `${workspace ?? ""}\u0000${path}`;

/** How many files have a version kept for them in `workspace`. */
export function keptIn(workspace: string | null): number {
  const prefix = keyOf(workspace, "");
  return [...kept.keys()].filter((key) => key.startsWith(prefix)).length;
}

type Written = "written" | "stale" | "failed";
const nameOf = (path: string) => path.split("/").pop() ?? path;

/**
 * The file open beside the conversation, and everything true of it that is not
 * yet true on disk.
 *
 * Its own component rather than state in `App` because of how often it
 * changes: on every keystroke. Held in `App`, each one redrew the rail, the
 * composer and the transcript pane to put one character in a column none of
 * them are in. Here a keystroke draws this and the editor, and `App` hears only
 * the one thing it needs — whether there is unsaved typing — and only when
 * that flips.
 *
 * Mounted for as long as a file is open, including while Changes has the
 * column. It draws nothing then and keeps the document, so the file comes back
 * with its typing. The editor is what mounts and unmounts: laid out to nothing
 * it would not be hidden, it would be broken.
 */
export function CanvasSlot({
  path,
  reveal,
  workspace,
  showing,
  pane,
  onUnsaved,
  onSelect,
  onAsk,
  onClose,
  flushRef,
}: {
  path: string;
  reveal: { from: number; to: number } | null;
  /** Re-read on a new folder, where the same path is a different file. */
  workspace: string | null;
  /** Whether the column is this one's. Changes takes it while it is open. */
  showing: boolean;
  pane: Resizable;
  /** Told when the editor starts or stops holding what the disk does not.
   *  Called on the edge, so it must be stable — a state setter is. */
  onUnsaved: (unsaved: boolean) => void;
  onSelect: (selection: Selection | null) => void;
  onAsk: (text: string) => void;
  onClose: () => void;
  /**
   * Set to what writes the waiting typing now, for `App` to run before it
   * switches workspace. By the time this component hears of a switch, the host
   * already answers for the new folder, so a write then would land there.
   */
  flushRef?: MutableRefObject<(() => Promise<void>) | null>;
}) {
  const noteError = useStore((s) => s.noteError);
  const wrote = useStore((s) => s.wrote);

  const [doc, setDoc] = useState<Document | null>(null);
  const [docError, setDocError] = useState<string | null>(null);
  /**
   * What is in the editor, which is not always what is on disk.
   *
   * Held beside `doc` rather than inside it, because they are two different
   * facts and the difference between them *is* the unsaved state. `doc` is what
   * was last read or written; this is what has been typed since.
   *
   * Not called `draft`: the composer already has one of those, and two in one
   * window is the sort of shadowing that compiles and then goes wrong
   * somewhere else.
   */
  const [typed, setTyped] = useState("");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  /**
   * Why the last save failed, for the header word that says it did. The
   * banner it also goes to can be dismissed; the word stays until the next
   * keystroke tries again, so it carries its own reason.
   */
  const [saveError, setSaveError] = useState<string | null>(null);
  /**
   * The running turn deleted or moved the open file. What's on screen is
   * what it last held, and the header says the file itself is gone.
   */
  const [gone, setGone] = useState(false);
  /** The other version, when a save was refused because somebody got there
   *  first. Both are kept; neither is chosen here. */
  const [conflict, setConflict] = useState<Document | null>(null);
  /** Lines somebody else just changed, to tint for a moment. */
  const [flash, setFlash] = useState<{ from: number; to: number } | null>(null);

  // Nearly always false — the canvas saves a moment after typing stops. What
  // it catches is the conflict, where the screen and the disk hold different
  // things until somebody decides. See `onScreenFor`.
  const unsaved = !!doc && typed !== doc.text;
  useEffect(() => onUnsaved(unsaved), [unsaved, onUnsaved]);

  // The same values, for code that runs after the render it was made in: a save
  // answered after a switch, a cleanup on the way out.
  const live = useRef({ doc, typed, conflict });
  live.current = { doc, typed, conflict };
  const workspaceNow = useRef(workspace);
  workspaceNow.current = workspace;
  /**
   * Which opening of a file the editor is on. It moves the moment the editor
   * leaves one, and an answer is applied only if it hasn't moved since it was
   * asked. Applied anyway, a save answered after another file opened made the old
   * file current again under the new one's text, and the next autosave wrote that
   * text into the old file.
   */
  const session = useRef(0);
  /** Where a version of the file now open would be kept. */
  const where = useRef("");
  /** The autosave waiting out its debounce, so leaving can run it now instead. */
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** The save in flight, so another for that file waits rather than racing it. */
  const saving = useRef<{ path: string; text: string; done: Promise<Written> } | null>(null);
  /** What the last save wrote, with the fingerprint it came back with. */
  const landed = useRef<Document | null>(null);

  /**
   * Writes `text` over the file `target` was read as.
   *
   * `stamp` is the opening it was asked in, taken once: a save queued behind
   * another runs later, and must answer to the opening that asked for it.
   */
  const write = (
    target: Document,
    text: string,
    stamp = { session: session.current, key: where.current },
  ): Promise<Written> => {
    const here = () => session.current === stamp.session;
    const underway = saving.current;
    if (underway?.path === target.path && underway.text === text) return underway.done;
    // Another save of this file is still out. Sent now, this one would carry the
    // fingerprint from before it, and the host would refuse it as a change on
    // disk, which it is, by this editor's own hand. So it goes once the first
    // has answered, with the fingerprint that one brought back.
    if (underway?.path === target.path) {
      const after: Promise<Written> = underway.done.then((first) => {
        if (saving.current?.done === after) saving.current = null;
        if (first === "written") {
          return write(landed.current?.path === target.path ? landed.current : target, text, stamp);
        }
        // The first one's text was kept if the file has been left, and this is
        // newer. It has already said why.
        if (!here()) kept.set(stamp.key, { base: target, draft: text });
        return first;
      });
      saving.current = { path: target.path, text, done: after };
      return after;
    }
    setSaveState("saving");
    const done = api
      .saveDocument(target.path, text, target.fingerprint)
      .then((result): Written => {
        if (result.type === "stale") {
          if (here()) {
            // Not an error. Somebody wrote the file after this editor read it,
            // so both versions exist and which one survives is not a decision
            // this code gets to make.
            setConflict(result.current);
            setSaveState("typing");
          } else {
            kept.set(stamp.key, { base: target, draft: text });
            noteError(
              `"${nameOf(target.path)}" changed on disk while you were working on it, so your version was kept rather than saved. Open it again to choose between them.`,
            );
          }
          return "stale";
        }
        landed.current = result.document;
        if (here()) {
          setDoc(result.document);
          setSaveState("idle");
        }
        return "written";
      })
      .catch((e): Written => {
        if (here()) {
          setSaveState("failed");
          setSaveError(String(e));
          noteError(String(e));
        } else {
          kept.set(stamp.key, { base: target, draft: text });
          noteError(`${String(e)} Your version of "${nameOf(target.path)}" is kept. Open it again to save it.`);
        }
        return "failed";
      })
      .finally(() => {
        if (saving.current?.done === done) saving.current = null;
      });
    saving.current = { path: target.path, text, done };
    return done;
  };

  /**
   * The editor is coming off this opening: another file, another folder, or
   * closed. The debounce is right while somebody is typing and wrong now, because
   * cancelled by the move it would take the last sentence with it.
   *
   * What can't be written now is kept: a conflict still waiting for a choice, or
   * anything at all once the folder has moved, since the host already answers
   * for the new one and the same path there is a different file. `App` writes
   * what it can before it asks for that switch, and says what was kept after it.
   */
  const leave = (from: string | null) => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
    const { doc, typed, conflict } = live.current;
    if (doc && typed !== doc.text) {
      const moved = workspaceNow.current !== from;
      if (conflict || moved) {
        kept.set(where.current, { base: doc, draft: typed });
        if (!moved) {
          noteError(
            `"${nameOf(doc.path)}" is showing two versions, so yours was kept rather than saved. Open it again to choose between them.`,
          );
        }
      } else {
        // Before the session moves, so this save knows it has been left.
        void write(doc, typed);
      }
    }
    session.current += 1;
  };

  /*
   * Reads whatever the canvas is open on.
   *
   * Re-runs on the path rather than on the reveal, so being sent to new lines
   * in a file that is already open scrolls it instead of fetching it again.
   */
  useEffect(() => {
    let current = true;
    setDoc(null);
    setDocError(null);
    setConflict(null);
    setFlash(null);
    setSaveState("idle");
    setSaveError(null);
    setGone(false);
    const key = keyOf(workspace, path);
    where.current = key;
    api
      .openDocument(path)
      .then((read) => {
        if (!current) return;
        const held = kept.get(key);
        kept.delete(key);
        if (!held || read.text === held.draft) {
          setDoc(read);
          // The buffer starts as what was read. Everything after this point is
          // the difference between the two.
          setTyped(read.text);
        } else if (read.fingerprint === held.base.fingerprint) {
          // Nothing has written it since the typing began: the kept version
          // simply saves.
          setDoc(read);
          setTyped(held.draft);
        } else {
          // Asked about against the file as it is now, not as it was then.
          setDoc(held.base);
          setTyped(held.draft);
          setConflict(read);
        }
      })
      .catch((e) => current && setDocError(String(e)));
    // A file opened, then closed, then opened again before the first read
    // landed would otherwise show the first file's contents under the second
    // one's name.
    return () => {
      current = false;
      leave(workspace);
    };
    // `leave` reads everything through refs, so naming it restarts nothing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, workspace]);

  useEffect(() => {
    if (!flushRef) return;
    flushRef.current = async () => {
      if (timer.current !== null) clearTimeout(timer.current);
      timer.current = null;
      const { doc, typed, conflict } = live.current;
      if (doc && !conflict && typed !== doc.text) await write(doc, typed);
    };
    return () => {
      flushRef.current = null;
    };
    // Reads everything through refs, like `leave`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [flushRef]);

  /*
   * Writes what has been typed, once typing has stopped.
   *
   * Debounced rather than saved per keystroke, and autosaved rather than left
   * to ⌘S, because the whole argument for the canvas is that the model is
   * looking at the same file you are. An unsaved buffer breaks that silently:
   * you ask about the paragraph on screen and it reads the one on disk.
   *
   * Nothing runs while a conflict is open — a save is exactly what is being
   * decided about, and repeating the refusal every 800ms would bury the
   * question under its own answer.
   */
  useEffect(() => {
    if (!doc || conflict || typed === doc.text) return;
    setSaveState("typing");
    setSaveError(null);
    const pending = setTimeout(() => {
      timer.current = null;
      void write(doc, typed);
    }, SAVE_AFTER_MS);
    timer.current = pending;
    return () => {
      clearTimeout(pending);
      if (timer.current === pending) timer.current = null;
    };
    // `write` works through refs and a store action that never changes, so this
    // restarts on typing and nothing else.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [typed, doc, conflict]);

  /*
   * What to do when the running turn writes the file that is open.
   *
   * The rule is `reconcile`'s: never silently lose typing. A clean buffer takes
   * the new version and flashes what moved, which is the model visibly editing
   * the document. A dirty one keeps both and asks.
   */
  useEffect(() => {
    if (!wrote || !doc) return;
    if (!wrote.paths.includes(doc.path)) return;
    let current = true;
    // Not cancelled by the path changing, since this hangs off the counter, so
    // the opening it was asked in is checked as well. And what's compared is the
    // buffer now: typing may have gone on while the read was out.
    const opening = session.current;
    const still = () => current && session.current === opening;
    api
      .openDocument(doc.path)
      .then((read) => {
        if (!still()) return;
        const { doc, typed } = live.current;
        if (!doc) return;
        setGone(false);
        const what = reconcile({ base: doc.text, draft: typed }, read.text);
        if (what.kind === "same") {
          // Still take the new fingerprint: the bytes match, but the file has
          // been rewritten, and saving against the old stamp would be refused
          // for a conflict that does not exist.
          setDoc(read);
          return;
        }
        if (what.kind === "conflict") {
          setConflict(read);
          return;
        }
        setFlash(changedLines(doc.text, read.text));
        setDoc(read);
        setTyped(read.text);
      })
      .catch(() => {
        // A file the turn deleted or moved. Left as it was rather than blanked
        // — what is on screen is still what was there, and saying so with an
        // empty editor would be a lie about the file. The header says the
        // file is gone, which before this only a failed save found out.
        if (still()) setGone(true);
      });
    return () => {
      current = false;
    };
    // Only the counter. Depending on the document or the typing would re-read
    // the file on every keystroke while the same write was still the newest one.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wrote?.at]);

  /* The tint is a moment, not a state. Cleared on a timer rather than by the
     animation ending, so nothing depends on an event that does not fire when
     motion is turned off. */
  useEffect(() => {
    if (!flash) return;
    const timer = setTimeout(() => setFlash(null), FLASH_MS);
    return () => clearTimeout(timer);
  }, [flash]);

  /*
   * Escape closes it, the way Escape closes Changes, which takes turns with
   * this in the same column.
   *
   * Only an Escape aimed here: focus in the canvas, or on nothing. Escape in
   * the composer already means something there (stop reading, drop a
   * recording), and closing the file as well would be one key doing two
   * things. In the editor the first press leaves the field and the next one
   * closes, so a press meant as "stop editing" doesn't throw the file away.
   * Only while the column is this one's, so it never closes a file hidden
   * under Changes.
   */
  useEffect(() => {
    if (!showing) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const target = e.target as HTMLElement | null;
      const inCanvas = !!target?.closest?.(".canvas");
      const nowhere = !target || target === document.body || target === document.documentElement;
      if (!inCanvas && !nowhere) return;
      if (inCanvas && target?.closest?.("input, textarea, select, [contenteditable='true']")) {
        target.blur();
        return;
      }
      onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [showing, onClose]);

  if (!showing) return null;

  return (
    <>
      <ResizeHandle pane={pane} label="Editor width" />
      <div className="side-slot" ref={pane.slot} style={{ width: pane.size }}>
        <Canvas
          path={path}
          reveal={reveal}
          document={doc}
          draft={typed}
          state={saveState}
          stateWhy={saveError}
          gone={gone}
          flash={flash}
          conflict={conflict}
          error={docError}
          onEdit={setTyped}
          onKeepMine={() => {
            // Adopt the *fingerprint* the refusal handed back while keeping
            // the old text as the base. The stamp is what makes the next save
            // succeed where the refused one did not; the stale base is what
            // keeps `draft !== doc.text`, so the autosave above still has
            // something to do.
            if (conflict && doc) setDoc({ ...conflict, text: doc.text });
            setConflict(null);
          }}
          onTakeTheirs={() => {
            if (!conflict) return;
            setDoc(conflict);
            setTyped(conflict.text);
            setConflict(null);
            setSaveState("idle");
          }}
          onSelect={onSelect}
          onAsk={onAsk}
          onClose={onClose}
        />
      </div>
    </>
  );
}
