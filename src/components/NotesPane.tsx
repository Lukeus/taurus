import {
  lazy,
  Suspense,
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { when } from "../lib/format";
import { chord, isChord } from "../lib/keys";
import { toggleTask } from "../lib/prose";
import { embedFor, pathOf, same, type Notebook, type NoteMode } from "../state/notebook";
import type { PageKind, PageRef, Scope } from "../lib/api";
import { CopyButton } from "./CopyButton";
import { ListPaneIcon } from "./icons";
import { Markdown, NoteHost } from "./Markdown";
import { NoteBody, type NoteBodyHandle } from "./NoteBody";
import { Problem } from "./Problem";
import { ProseEditor } from "./ProseEditor";
import { SketchHost } from "./SketchEmbed";
import { useArmed } from "../lib/armed";
import { ConflictBanner } from "./ConflictBanner";

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
    drain,
    keptAs,
    viewKeyFor,
  } = notebook;

  /**
   * Whether the list is folded away, giving the open file the whole width.
   *
   * Remembered, because it is a habit rather than a moment — somebody who
   * sketches wants the room every time. Only ever folded with a file open: with
   * nothing open, the list is the only thing on the screen that can open one.
   */
  const [listShut, setListShut] = useState(readListShut);
  const foldList = useCallback(
    () =>
      setListShut((shut) => {
        keepListShut(!shut);
        return !shut;
      }),
    [],
  );

  /** What is being made, where, and what it is being called. */
  const [making, setMaking] = useState<{ scope: Scope; kind: PageKind; name: string } | null>(
    null,
  );
  /** Why the last create was refused. Kept beside the box that asked for the
   *  name, which is the only place it means anything. */
  const [refused, setRefused] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  /** Why the last rename was refused — its own rather than shared with the
   *  create box, so a name refused in the list is never reported under the
   *  open note's header, or the other way round. */
  const [renameRefused, setRenameRefused] = useState<string | null>(null);
  /** Armed once, so nothing is deleted by one stray click. */
  const { armed: arming, arm, disarm } = useArmed();

  /*
   * A different file is open, by whichever route — a row, an embed's open, a
   * card in the transcript. A half-typed rename, an armed Delete and a refused
   * rename are all about the file they began on, and none follows the pane to
   * the next one.
   */
  const openKey = open ? `${open.scope}:${open.kind}:${open.name}` : "";
  useEffect(() => {
    setRenaming(null);
    setRenameRefused(null);
    disarm();
  }, [openKey, disarm]);

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

  /** The sketches a note being written can embed: the ones in its notebook. */
  const sketchNames = useMemo(
    () =>
      (pages ?? [])
        .filter((p) => p.kind === "sketch" && p.scope === noteScope)
        .map((p) => p.name),
    [pages, noteScope],
  );

  /** The notes a link in the open note can reach: its own notebook's. */
  const noteNames = useMemo(
    () =>
      (pages ?? [])
        .filter((p) => p.kind === "note" && p.scope === noteScope)
        .map((p) => p.name),
    [pages, noteScope],
  );
  /** And the ones worth offering to link to, which is all but itself. */
  const linkable = useMemo(
    () => noteNames.filter((name) => name !== page?.name),
    [noteNames, page?.name],
  );

  /*
   * The rendered note trails the keys by a render when it has to. In Split it
   * is on screen while somebody types, and a whole note parsed and laid out
   * between one key and the next is a keystroke that waits for it. Deferred,
   * React draws the text first and the note when there is time.
   */
  const shown = useDeferredValue(typed);
  const latest = useRef({ typed, shown });
  latest.current = { typed, shown };

  const pane = useRef<HTMLElement>(null);
  const body = useRef<NoteBodyHandle | null>(null);
  /** The mode before Split, which ⌘\ goes back to. */
  const beforeSplit = useRef<NoteMode>("write");

  /** Changes how the note is shown, keeping the reader where they were. */
  const switchTo = useCallback(
    (next: NoteMode) => {
      if (next === mode) return;
      if (next === "split") beforeSplit.current = mode;
      body.current?.capture();
      setMode(next);
    },
    [mode, setMode],
  );

  /*
   * ⌘E between Write and Read, and ⌘\ for the two side by side — from Split,
   * ⌘E goes to Read. On the window rather than the editor, because in Read the
   * editor is not where the focus is. Not while somebody is typing somewhere
   * else, like the composer under the pane: a key there is for that box.
   */
  const isNote = page?.kind === "note";
  useEffect(() => {
    if (!isNote) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing || e.shiftKey) return;
      const target = e.target instanceof HTMLElement ? e.target : null;
      const typing = target?.closest("input, textarea, [contenteditable='true']");
      if (typing && !pane.current?.contains(typing)) return;
      if (isChord(e, "e")) {
        e.preventDefault();
        switchTo(mode === "read" ? "write" : "read");
      } else if (isChord(e, "\\")) {
        e.preventDefault();
        switchTo(mode === "split" ? beforeSplit.current : "split");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isNote, mode, switchTo]);

  /*
   * What a note being read can do to the notebook — see `NoteHost`.
   *
   * A tick goes against the text on screen, and only while that is the text in
   * the editor: the rendered note can trail the keys by a render, and a
   * position read off a note one keystroke old could be the next task's box.
   * The editor's caret is put back afterwards, because the text changing under
   * it would otherwise send it to the end.
   */
  const noteHost = useMemo(() => {
    if (noteScope === null || !page || page.kind !== "note") return null;
    return {
      hasNote: (name: string) => noteNames.includes(name),
      openNote: (name: string) => choose({ scope: noteScope, kind: "note" as const, name }),
      toggleTask: (at: number) => {
        const { typed: now, shown: seen } = latest.current;
        if (seen !== now) return;
        const next = toggleTask(now, at);
        if (next === null) return;
        const area = pane.current?.querySelector<HTMLTextAreaElement>("textarea.prose-input");
        const kept = area ? [area.selectionStart, area.selectionEnd] : null;
        edit(next, page);
        if (area && kept) {
          requestAnimationFrame(() => area.setSelectionRange(kept[0], kept[1]));
        }
      },
    };
  }, [noteScope, page, noteNames, choose, edit]);

  const word = page?.kind === "sketch" ? "sketch" : "note";
  const folded = listShut && page !== null;

  return (
    <section className="notes-pane" aria-label="Notes" ref={pane}>
      {!folded && (
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
            keptAs={keptAs}
            onChoose={choose}
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
      )}

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
            <button
              className="notes-fold"
              onClick={foldList}
              aria-expanded={!listShut}
              aria-label={listShut ? "Show the list" : "Hide the list"}
              data-tip={listShut ? "Show the list" : "Hide the list"}
            >
              <ListPaneIcon size={14} />
            </button>
            {renaming === null ? (
              <>
                {/* Titled, because a long one is cut to fit the header. */}
                <span className="notes-name" title={page.name}>
                  {page.name}
                </span>
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
                    setRenameRefused(null);
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
                        setRenameRefused(null);
                      })
                      .catch((err) =>
                        setRenameRefused(err instanceof Error ? err.message : String(err)),
                      );
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
                <div className="notes-modes" role="tablist" aria-label="How to show it">
                  {(["write", "split", "read"] as const).map((m) => (
                    <button
                      key={m}
                      role="tab"
                      aria-selected={mode === m}
                      className={`seg${mode === m ? " on" : ""}`}
                      onClick={() => switchTo(m)}
                      data-tip={MODE_TIP[m]}
                    >
                      {MODE_WORD[m]}
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
                  arm(true);
                  return;
                }
                disarm();
                void forget(page);
              }}
            >
              {arming ? "Delete it" : "Delete"}
            </button>
          </header>

          {renameRefused && <Problem>{renameRefused}</Problem>}

          {/* Somebody else wrote the file while there was work in the editor.
              Drawn above the editor rather than over it, so what is being
              decided about stays readable while the decision is made. */}
          {conflict && (
            <ConflictBanner
              className="notes-conflict"
              title={`This ${word} changed while you were working on it.`}
              onTakeTheirs={takeTheirs}
              onKeepMine={keepMine}
            />
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
                  // Bound to this sketch, so a stroke it hands over after the
                  // pane has moved on is not taken as the next file's text.
                  onChange={(text) => edit(text, page)}
                  drain={drain}
                  // Where it was being looked at, kept on this machine rather
                  // than in the file — see `sketchView.ts`.
                  viewKey={viewKeyFor(page)}
                />
              </Suspense>
            </div>
          ) : (
            <NoteBody
              mode={mode}
              handle={body}
              editor={
                <ProseEditor
                  // Keyed on the note so a different one gets a fresh box rather
                  // than the previous note's caret. Deliberately not on the
                  // fingerprint: a save must not remount the editor and lose the
                  // caret in the middle of a sentence.
                  key={`${page.scope}:${page.name}`}
                  text={typed}
                  onChange={(text) => edit(text, page)}
                  // Leaving the box saves what it holds now, rather than a
                  // debounce later that somebody who has moved on is not
                  // watching for.
                  onBlur={() => void notebook.flush()}
                  // The one key worth knowing, said where it is used.
                  placeholder="Write in Markdown. Type / at the start of a line for a heading, a list, a diagram or a sketch."
                  sketches={sketchNames}
                  notes={linkable}
                />
              }
              preview={
                mode === "write" ? null : (
                  // What has been typed, not what was read: a preview showing
                  // the version on disk while the editor holds a newer one
                  // would be two answers to one question on one screen. Keyed
                  // by content, so a keystroke in the first paragraph does not
                  // lay out every diagram below it again.
                  <NoteHost.Provider value={noteHost}>
                    <SketchHost.Provider value={sketches}>
                      <Markdown text={shown} streaming={false} keyBy="content" />
                    </SketchHost.Provider>
                  </NoteHost.Provider>
                )
              }
            />
          )}
        </section>
      )}
    </section>
  );
}

const MODE_WORD: Record<NoteMode, string> = { write: "Write", split: "Split", read: "Read" };

/** Each mode's key, where the mode is chosen. ⌘E is both halves of one toggle. */
const MODE_TIP: Record<NoteMode, string> = {
  write: `Write (${chord("E")})`,
  split: `Write and Read side by side (${chord("\\")})`,
  read: `Read (${chord("E")})`,
};

const LIST_KEY = "taurus.notesListShut";

/** Whether the list was left folded. Guarded the way `ResizeHandle`'s
 *  `remembered` is, for the same three reasons. */
function readListShut(): boolean {
  try {
    return typeof localStorage !== "undefined" && localStorage.getItem(LIST_KEY) === "1";
  } catch {
    return false;
  }
}

function keepListShut(shut: boolean): void {
  try {
    localStorage.setItem(LIST_KEY, shut ? "1" : "0");
  } catch {
    // Storage turned off still folds. It just forgets.
  }
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
  keptAs,
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
  /** Whether a row's file has a version of yours kept for it. */
  keptAs: Notebook["keptAs"];
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
                  aria-current={same(open, p) ? "true" : undefined}
                  onClick={() => onChoose(p)}
                >
                  <b>{p.name}</b>
                  <span className="micro">
                    {p.kind === "sketch" ? "sketch · " : ""}
                    {when(p.at)}
                    {/* A version of yours kept when this was left. Said in the
                        row, because the row is all that is on screen of it. */}
                    {keptAs(p) === "conflict" && (
                      <span className="notes-kept"> · two versions</span>
                    )}
                    {keptAs(p) === "unsaved" && <span className="notes-kept"> · not saved</span>}
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
