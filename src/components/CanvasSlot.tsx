import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { Document, Selection } from "../lib/api";
import { changedLines, FLASH_MS, reconcile, SAVE_AFTER_MS } from "../lib/document";
import { useStore } from "../state/store";
import { Canvas, type SaveState } from "./Canvas";
import { ResizeHandle, type Resizable } from "./ResizeHandle";

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
    api
      .openDocument(path)
      .then((read) => {
        if (!current) return;
        setDoc(read);
        // The buffer starts as what was read. Everything after this point is
        // the difference between the two.
        setTyped(read.text);
      })
      .catch((e) => current && setDocError(String(e)));
    // A file opened, then closed, then opened again before the first read
    // landed would otherwise show the first file's contents under the second
    // one's name.
    return () => {
      current = false;
    };
  }, [path, workspace]);

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
    const timer = setTimeout(() => {
      setSaveState("saving");
      api
        .saveDocument(doc.path, typed, doc.fingerprint)
        .then((result) => {
          if (result.type === "stale") {
            // Not an error. Somebody wrote the file after this editor read it,
            // so both versions exist and which one survives is not a decision
            // this code gets to make.
            setConflict(result.current);
            setSaveState("typing");
            return;
          }
          setDoc(result.document);
          setSaveState("idle");
        })
        .catch((e) => {
          setSaveState("failed");
          noteError(String(e));
        });
    }, SAVE_AFTER_MS);
    return () => clearTimeout(timer);
    // `noteError` is a store action and never changes, so naming it restarts
    // nothing — a fast typist still reaches the end of the timer.
  }, [typed, doc, conflict, noteError]);

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
    api
      .openDocument(doc.path)
      .then((read) => {
        if (!current) return;
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
        // empty editor would be a lie about the file.
      });
    return () => {
      current = false;
    };
    // Only the counter. The document and the typing are read through the
    // closure, and depending on them would re-read the file on every keystroke
    // while the same write was still the newest one.
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

  if (!showing) return null;

  return (
    <>
      <ResizeHandle pane={pane} label="Editor width" />
      <div className="side-slot" style={{ width: pane.size }}>
        <Canvas
          path={path}
          reveal={reveal}
          document={doc}
          draft={typed}
          state={saveState}
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
