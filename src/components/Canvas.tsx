import { useEffect, useMemo, useState } from "react";

import { grammarFor } from "../lib/ink";
import type { Document } from "../bindings/Document";
import { DocumentEditor } from "./DocumentEditor";
import { Markdown } from "./Markdown";

/**
 * The file open beside the conversation.
 *
 * # Why this is a split and not a third tab
 *
 * The Data pane is the experiment that already ran. It is a full-width view
 * that replaces the transcript, and replacing the transcript turned out to cost
 * enough that two mechanisms had to be invented afterwards to put the agent
 * back into it — `OnScreen`, so a question asked from the pane has a subject,
 * and `TurnStrip`, so a turn running elsewhere is visible at all.
 *
 * The whole point of a document canvas is the sentence "read this part while I
 * work on it", and there is no version of that where the conversation is on
 * another screen. So this sits beside the transcript, and the composer — which
 * already lives outside `<main>` — spans both.
 *
 * # Why the text is fetched here rather than carried in
 *
 * The tool call that opens a document carries a path and never the file. See
 * `taurus_host::document` for the argument; the consequence here is that this
 * component owns a fetch, a loading state, and an error state, and that a card
 * in a year-old conversation opens today's version of the file.
 */
export function Canvas({
  path,
  reveal,
  document,
  draft,
  state,
  flash,
  conflict,
  error,
  onEdit,
  onKeepMine,
  onTakeTheirs,
  onSelect,
  onAsk,
  onClose,
}: {
  /** What is open, workspace-relative. `null` when nothing is. */
  path: string | null;
  /** Where in it to go, when the call that opened it said. */
  reveal: { from: number; to: number } | null;
  /** The file as last read from or written to disk. `null` while reading. */
  document: Document | null;
  /** What is in the editor now, which is what is shown. */
  draft: string;
  /** Where the save has got to, for the one word in the header that says so. */
  state: SaveState;
  /** Lines somebody else just changed, to tint briefly. */
  flash: { from: number; to: number } | null;
  /**
   * The other version, when a save was refused because somebody got there
   * first. Both are kept and neither is chosen — see `reconcile`.
   */
  conflict: Document | null;
  error: string | null;
  onEdit: (text: string) => void;
  /** Saves what is in the editor over the other version. */
  onKeepMine: () => void;
  /** Throws away what is in the editor and takes the other version. */
  onTakeTheirs: () => void;
  onSelect: (selection: { from: number; to: number; text: string } | null) => void;
  /**
   * Hands the composer a sentence about the selection.
   *
   * Fills the box and does not send it, which is the rule every other button
   * in the app that offers a draft follows: the user finishes the thought. See
   * `withDraft`.
   */
  onAsk: (draft: string) => void;
  onClose: () => void;
}) {
  const [mode, setMode] = useState<Mode>("source");
  const [selection, setSelection] = useState<Sel | null>(null);

  const prose = useMemo(() => grammarFor(path) === "markdown", [path]);

  /*
   * A new file is a new set of choices about how to look at it.
   *
   * Markdown opens on its preview, because the thing somebody asked to see
   * when they said "open the readme" is the readme and not its asterisks. Code
   * has no preview to open on.
   *
   * **Unless the call named lines.** Found by photographing it: a model that
   * says "the rule is at lines 15–19" and is answered with a rendered page
   * where nothing is marked has had its whole sentence thrown away — the
   * pointing is the part that made the answer worth more than a quote, and
   * preview has nowhere to put it. So a range wins over the format, and the
   * Preview switch is still right there for afterwards.
   *
   * Both reset per file rather than persisting: this is a property of the
   * document and of how it was opened, not a preference of the reader's.
   */
  useEffect(() => {
    setMode(prose && !reveal ? "preview" : "source");
    setSelection(null);
    // `reveal` by identity, so being sent to a *new* place in a file already
    // open switches back to the source it will be marked in.
  }, [path, prose, reveal]);

  const take = (next: Sel | null) => {
    setSelection(next);
    onSelect(next);
  };

  if (!path) return null;

  const name = path.split("/").pop() ?? path;

  return (
    <section className="canvas" aria-label={`Editor: ${path}`}>
      <header className="canvas-head">
        <span className="canvas-name" title={path}>
          {name}
        </span>
        {/* The rest of the path, dimmed. The filename is what anybody is
            looking for and the folder is what tells two of them apart. */}
        {path.includes("/") && (
          <span className="canvas-where">{path.slice(0, path.lastIndexOf("/"))}</span>
        )}

        <div className="spacer" />

        {/* One word, and only when it is not the resting state. "Saved" that
            never goes away is furniture; "Saving…" that appears for 200ms on
            every keystroke is a flicker. So: silence when there is nothing to
            say, and a word when there is. */}
        {state !== "idle" && (
          <span className={`canvas-state ${state}`}>{SAVE_WORD[state]}</span>
        )}

        {document && state === "idle" && (
          <span className="canvas-count">
            {document.lines === 1 ? "1 line" : `${document.lines} lines`}
          </span>
        )}

        {/* Only where there is something to switch to. A `.rs` file has one
            way of being read and a control offering one choice is furniture. */}
        {prose && (
          <div className="canvas-modes" role="tablist" aria-label="How to read it">
            {(["preview", "source"] as const).map((m) => (
              <button
                key={m}
                role="tab"
                aria-selected={mode === m}
                className={`seg${mode === m ? " on" : ""}`}
                onClick={() => setMode(m)}
              >
                {m === "preview" ? "Preview" : "Source"}
              </button>
            ))}
          </div>
        )}

        <button className="canvas-close" onClick={onClose} aria-label="Close the editor">
          ✕
        </button>
      </header>

      {/*
       * Somebody else wrote the file while there was typing in the editor.
       *
       * Both versions are kept and neither is chosen, which is the whole rule:
       * the one thing that must not happen here is the app deciding whose work
       * to throw away. Drawn above the editor rather than over it, so what is
       * being decided about stays readable while the decision is made.
       */}
      {conflict && (
        <div className="canvas-conflict" role="alert">
          <div className="canvas-conflict-say">
            {/* Not "wrote over it" — nothing was overwritten, which is the
                entire point. The save was refused, so both versions exist and
                the sentence has to say that rather than describe a loss that
                did not happen. */}
            <b>Taurus changed this file while you were typing.</b>
            <span>Your version is still here, unsaved.</span>
          </div>
          <button className="pill" onClick={onTakeTheirs}>
            Take theirs
          </button>
          <button className="pill primary" onClick={onKeepMine}>
            Keep mine
          </button>
        </div>
      )}

      {error ? (
        <div className="canvas-problem">{error}</div>
      ) : !document ? (
        // Deliberately blank rather than a spinner. Reading a file this size is
        // a few milliseconds, and a spinner that appears and vanishes inside
        // one frame reads as a flicker rather than as progress.
        <div className="canvas-loading" />
      ) : mode === "preview" ? (
        <div className="canvas-preview">
          {/* The draft, not what was read: a preview showing the version on
              disk while the editor holds a newer one would be two answers to
              the same question on the same screen. */}
          <Markdown text={draft} streaming={false} />
        </div>
      ) : (
        <DocumentEditor
          // Keyed on the path so a different file gets a fresh box rather than
          // the previous one's scroll position and selection. Deliberately not
          // on the fingerprint: a save must not remount the editor and throw
          // away the caret in the middle of a sentence.
          key={document.path}
          text={draft}
          path={document.path}
          reveal={reveal}
          flash={flash}
          onSelect={take}
          onChange={onEdit}
        />
      )}

      {/*
       * The one gesture that makes this a place to work rather than a place to
       * read: highlight a passage and ask about it. Anchored to the panel
       * rather than floating at the pointer, because a popover that follows a
       * drag is in the way of the drag.
       */}
      {selection && (
        <div className="canvas-ask">
          <span className="canvas-ask-where">
            {selection.from === selection.to
              ? `Line ${selection.from}`
              : `Lines ${selection.from}–${selection.to}`}
          </span>
          <button
            className="canvas-ask-go"
            onClick={() => onAsk(draftFor(name, selection))}
          >
            Ask about this
          </button>
        </div>
      )}
    </section>
  );
}

type Mode = "source" | "preview";
type Sel = { from: number; to: number; text: string };

/** Where a save has got to. `idle` covers both "nothing typed" and "written". */
export type SaveState = "idle" | "typing" | "saving" | "failed";

const SAVE_WORD: Record<Exclude<SaveState, "idle">, string> = {
  typing: "Unsaved",
  saving: "Saving…",
  failed: "Not saved",
};

/**
 * What goes in the composer when somebody asks about a selection.
 *
 * A pointer and not a quote. The lines themselves travel to the model on the
 * prompt — see `OnScreen` — so putting them in the box as well would show the
 * user a wall of their own file where their question should be, and send it
 * twice. What the box needs is the half a sentence they are about to finish.
 */
export function draftFor(name: string, selection: Sel): string {
  const where =
    selection.from === selection.to
      ? `line ${selection.from}`
      : `lines ${selection.from}–${selection.to}`;
  return `About ${where} of ${name}: `;
}
