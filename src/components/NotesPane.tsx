import { lazy, Suspense, useCallback, useMemo, useState } from "react";

import { when } from "../lib/format";
import { embedFor, pathOf, same, type Notebook } from "../state/notebook";
import type { PageKind, PageRef, Scope } from "../lib/api";
import { CopyButton } from "./CopyButton";
import { Markdown } from "./Markdown";
import { Problem } from "./Problem";
import { ProseEditor } from "./ProseEditor";
import { SketchHost } from "./SketchEmbed";

/**
 * The sketch editor, loaded the first time a sketch is opened.
 *
 * It is Excalidraw, the largest thing the app carries, and a window that is
 * only ever used for notes — or for no notebook at all — never pays for it.
 */
const SketchEditor = lazy(() => import("./SketchEditor"));

/**
 * Notes and sketches, in two scopes, beside the conversation about them.
 *
 * # Why this is a pane and not a split
 *
 * The canvas argues hard for being a split rather than a third tab, because the
 * whole point of a document canvas is "read this part while I work on it" and
 * there is no version of that where the conversation is on another screen. A
 * notebook is the other case. Writing a note is a thing somebody does *instead*
 * of watching a turn — the sentence is "let me write this down" — and a sketch
 * wants every pixel of the width. The two mechanisms that made the Data pane
 * cost so much when it arrived, `OnScreen` and `TurnStrip`, both exist now, so
 * the pane is no longer a screen the agent cannot see.
 *
 * # Why the list carries no preview
 *
 * A row is a name and a date. The first line of a note is its title — which is
 * already the name in the row, because the name *is* the filename — and a
 * thumbnail of every sketch would cost a render of every drawing in the
 * directory to show the same thing the name says.
 */
export function NotesPane({
  notebook,
  /**
   * Puts a sentence in the composer without sending it.
   *
   * Every button in the app that offers a draft works this way: it fills the box
   * and leaves the person to finish the thought. See `withDraft`.
   */
  onAsk,
  /** Whether a workspace is open. A project note has nowhere to go without one. */
  hasWorkspace,
}: {
  notebook: Notebook;
  onAsk: (draft: string) => void;
  hasWorkspace: boolean;
}) {
  const {
    pages,
    open,
    page,
    typed,
    saveState,
    conflict,
    error,
    mode,
    generation,
    choose,
    edit,
    setMode,
    create,
    rename,
    forget,
    keepMine,
    takeTheirs,
  } = notebook;

  /** What is being made, where, and what it is being called. */
  const [making, setMaking] = useState<{ scope: Scope; kind: PageKind; name: string } | null>(
    null,
  );
  /** Why the last create or rename was refused. Kept beside the box that asked
   *  for the name, which is the only place it means anything. */
  const [refused, setRefused] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  /** Armed once, so nothing is deleted by one stray click. */
  const [arming, setArming] = useState(false);

  const start = useCallback(
    async (scope: Scope, kind: PageKind, name: string) => {
      try {
        await create(scope, kind, name);
        setMaking(null);
        setRefused(null);
      } catch (e) {
        setRefused(String(e));
      }
    },
    [create],
  );

  /*
   * Where an embedded sketch in the note being read is found, and how it opens.
   *
   * Memoized on the scope, because every embed re-reads its sketch when this
   * changes identity — and a value rebuilt on every render of the pane would
   * have them all reading on every keystroke anywhere in it.
   */
  const noteScope = page?.kind === "note" ? page.scope : null;
  const sketches = useMemo(
    () =>
      noteScope === null
        ? null
        : {
            scope: noteScope,
            open: (name: string) => choose({ scope: noteScope, kind: "sketch", name }),
          },
    [noteScope, choose],
  );

  const word = page?.kind === "sketch" ? "sketch" : "note";

  return (
    <section className="notes-pane" aria-label="Notes">
      <aside className="notes-list">
        {(["workspace", "global"] as const).map((scope) => (
          <Group
            key={scope}
            scope={scope}
            pages={pages}
            open={open}
            disabled={scope === "workspace" && !hasWorkspace}
            making={making?.scope === scope ? making : null}
            refused={making?.scope === scope ? refused : null}
            onChoose={(p) => {
              setArming(false);
              setRenaming(null);
              choose({ scope: p.scope, kind: p.kind, name: p.name });
            }}
            onOpen={() => {
              setMaking({ scope, kind: "note", name: "" });
              setRefused(null);
            }}
            onChange={(next) => setMaking({ scope, ...next })}
            onStart={(kind, name) => void start(scope, kind, name)}
            onCancel={() => {
              setMaking(null);
              setRefused(null);
            }}
          />
        ))}
      </aside>

      {page === null ? (
        <div className="notes-none">
          {error ? (
            <Problem>{error}</Problem>
          ) : (
            <>
              <h2>Notes</h2>
              <p>
                Markdown, kept as files. A <b>project</b> note lives in{" "}
                <code>.taurus/notes/</code> and is committed with the repository,
                so a design note and its diagrams reach whoever clones it. A{" "}
                <b>global</b> note lives in <code>~/.taurus/notes/</code> and
                follows you between projects.
              </p>
              <p className="notes-none-more">
                A <b>sketch</b> is a drawing kept beside them. A note shows one
                with <code>![Name](&lt;Name.excalidraw&gt;)</code>, and a{" "}
                <code>mermaid</code> block draws as a diagram.
              </p>
            </>
          )}
        </div>
      ) : (
        <section className="notes-doc" aria-label={`${word}: ${page.name}`}>
          <header className="notes-head">
            {renaming === null ? (
              <>
                <span className="notes-name">{page.name}</span>
                <span className="notes-where">
                  {page.scope === "workspace" ? pathOf(page.name, page.kind) : "global"}
                </span>
              </>
            ) : (
              <input
                className="notes-rename"
                autoFocus
                value={renaming}
                onChange={(e) => setRenaming(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    setRenaming(null);
                    setRefused(null);
                  }
                  if (e.key === "Enter") {
                    const to = renaming.trim();
                    if (to === "" || to === page.name) {
                      setRenaming(null);
                      return;
                    }
                    rename(page, to)
                      .then(() => {
                        setRenaming(null);
                        setRefused(null);
                      })
                      .catch((err) => setRefused(String(err)));
                  }
                }}
                aria-label={`Rename this ${word}`}
              />
            )}

            <div className="spacer" />

            {/* One word, and only when it is not the resting state. "Saved" that
                never goes away is furniture; "Saving…" for 200ms on every
                keystroke is a flicker. The canvas's rule, and the same words. */}
            {saveState !== "idle" && (
              <span className={`notes-state ${saveState}`}>{SAVE_WORD[saveState]}</span>
            )}

            {page.kind === "note" ? (
              <>
                <div className="notes-modes" role="tablist" aria-label="How to read it">
                  {(["write", "read"] as const).map((m) => (
                    <button
                      key={m}
                      role="tab"
                      aria-selected={mode === m}
                      className={`seg${mode === m ? " on" : ""}`}
                      onClick={() => setMode(m)}
                    >
                      {m === "write" ? "Write" : "Read"}
                    </button>
                  ))}
                </div>
                <button
                  className="pill"
                  onClick={() => onAsk(draftFor(page.scope, page.name))}
                >
                  Ask about this
                </button>
              </>
            ) : (
              // The line a note needs to show this sketch, rather than a button
              // that picks a note and edits it. Which note, and where in it, is
              // the writer's call — and the line is the whole of the syntax.
              <CopyButton className="pill" label="Copy embed" text={embedFor(page.name)} />
            )}
            {renaming === null && (
              <button className="pill" onClick={() => setRenaming(page.name)}>
                Rename
              </button>
            )}
            <button
              className={`pill${arming ? " armed" : ""}`}
              onClick={() => {
                if (!arming) {
                  setArming(true);
                  return;
                }
                setArming(false);
                void forget(page);
              }}
            >
              {arming ? "Delete it" : "Delete"}
            </button>
          </header>

          {refused && <Problem>{refused}</Problem>}

          {/*
           * Somebody else wrote the file while there was work in the editor.
           *
           * Both versions are kept and neither is chosen, which is the whole
           * rule. Drawn above the editor rather than over it, so what is being
           * decided about stays readable while the decision is made.
           */}
          {conflict && (
            <div className="notes-conflict" role="alert">
              <div className="notes-conflict-say">
                {/* Not "wrote over it" — nothing was overwritten, which is the
                    entire point. The save was refused, so both versions exist. */}
                <b>This {word} changed while you were working on it.</b>
                <span>Your version is still here, unsaved.</span>
              </div>
              <button className="pill" onClick={takeTheirs}>
                Take theirs
              </button>
              <button className="pill primary" onClick={keepMine}>
                Keep mine
              </button>
            </div>
          )}

          {page.kind === "sketch" ? (
            <div className="notes-body sketch">
              {/* Blank rather than a spinner while the chunk arrives: it is a
                  moment on first use and nothing after, and a spinner that
                  flashes for a quarter of a second reads as a flicker. */}
              <Suspense fallback={<div className="sketch-loading" />}>
                <SketchEditor
                  // Keyed on the sketch, so a different one is a fresh canvas.
                  // `generation` covers everything else — see `useNotebook`.
                  key={`${page.scope}:${page.name}`}
                  text={typed}
                  generation={generation}
                  onChange={edit}
                />
              </Suspense>
            </div>
          ) : (
            <div className="notes-body">
              {mode === "read" ? (
                <div className="notes-read">
                  {/* What has been typed, not what was read: a preview showing
                      the version on disk while the editor holds a newer one
                      would be two answers to one question on one screen. */}
                  <SketchHost.Provider value={sketches}>
                    <Markdown text={typed} streaming={false} />
                  </SketchHost.Provider>
                </div>
              ) : (
                <ProseEditor
                  // Keyed on the note so a different one gets a fresh box rather
                  // than the previous note's caret. Deliberately not on the
                  // fingerprint: a save must not remount the editor and lose the
                  // caret in the middle of a sentence.
                  key={`${page.scope}:${page.name}`}
                  text={typed}
                  onChange={edit}
                  placeholder="Write in Markdown. A mermaid block draws as a diagram."
                />
              )}
            </div>
          )}
        </section>
      )}
    </section>
  );
}

const SAVE_WORD: Record<"typing" | "saving" | "failed", string> = {
  typing: "Unsaved",
  saving: "Saving…",
  failed: "Not saved",
};

/** One scope's notes and sketches, with the row that makes a new one. */
function Group({
  scope,
  pages,
  open,
  disabled,
  making,
  refused,
  onChoose,
  onOpen,
  onChange,
  onStart,
  onCancel,
}: {
  scope: Scope;
  pages: PageRef[] | null;
  open: { scope: Scope; kind: PageKind; name: string } | null;
  disabled: boolean;
  making: { kind: PageKind; name: string } | null;
  refused: string | null;
  onChoose: (page: PageRef) => void;
  /** Opens the row that asks for a name. */
  onOpen: () => void;
  /** The kind and the name as they are being chosen in that row. */
  onChange: (next: { kind: PageKind; name: string }) => void;
  onStart: (kind: PageKind, name: string) => void;
  onCancel: () => void;
}) {
  const mine = (pages ?? []).filter((p) => p.scope === scope);
  const label = scope === "workspace" ? "project" : "global";

  return (
    <div className="notes-group">
      <div className="notes-group-head">
        <span className="micro">{scope === "workspace" ? "Project" : "Global"}</span>
        <div className="spacer" />
        {!disabled && (
          <button className="notes-new" onClick={onOpen} aria-label={`New ${label} note or sketch`}>
            +
          </button>
        )}
      </div>

      {disabled ? (
        // Said rather than hidden. An empty Project group with no explanation
        // reads as a feature that is broken rather than one that is waiting.
        <p className="notes-group-empty">Open a folder to keep notes with it.</p>
      ) : (
        <>
          {making !== null && (
            <div className="notes-make">
              <div className="notes-kind" role="radiogroup" aria-label="What to make">
                {(["note", "sketch"] as const).map((kind) => (
                  <button
                    key={kind}
                    role="radio"
                    aria-checked={making.kind === kind}
                    className={`seg${making.kind === kind ? " on" : ""}`}
                    onClick={() => onChange({ kind, name: making.name })}
                  >
                    {kind === "note" ? "Note" : "Sketch"}
                  </button>
                ))}
              </div>
              <input
                className="notes-make-name"
                autoFocus
                value={making.name}
                placeholder="Name it"
                onChange={(e) => onChange({ kind: making.kind, name: e.target.value })}
                onKeyDown={(e) => {
                  if (e.key === "Escape") onCancel();
                  if (e.key === "Enter" && making.name.trim() !== "") {
                    onStart(making.kind, making.name.trim());
                  }
                }}
                aria-label={`Name for the new ${making.kind}`}
              />
              {refused && <Problem>{refused}</Problem>}
            </div>
          )}

          {mine.length === 0 && making === null && (
            <p className="notes-group-empty">Nothing here yet.</p>
          )}

          <ul className="notes-rows">
            {mine.map((p) => (
              // Keyed on the kind too: a note and its sketch may share a name.
              <li key={`${p.kind}:${p.name}`}>
                <button
                  className="notes-row"
                  data-current={same(open, p) ? "" : undefined}
                  onClick={() => onChoose(p)}
                >
                  <b>{p.name}</b>
                  <span className="micro">
                    {p.kind === "sketch" ? "sketch · " : ""}
                    {when(p.at)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

/**
 * What the composer says once the button has filled it.
 *
 * Names the note rather than quoting it. `read_note` is one call away and reads
 * the file as it is now — pasting the text instead would put a copy in the
 * transcript that is wrong the moment either writer touches the file, which is
 * the argument every card in this app makes about carrying payloads.
 *
 * Deliberately unfinished: the person is about to say what they actually want
 * to know, and the cursor is left at the end for them to do it.
 */
export function draftFor(scope: Scope, name: string): string {
  const which = scope === "workspace" ? "project" : "global";
  return `About my ${which} note "${name}": `;
}
