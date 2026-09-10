import { useCallback, useState } from "react";

import { when } from "../lib/format";
import { pathOf, type Notebook } from "../state/notebook";
import type { PageRef, Scope } from "../lib/api";
import { Markdown } from "./Markdown";
import { Problem } from "./Problem";
import { ProseEditor } from "./ProseEditor";

/**
 * Notes, in two scopes, beside the conversation about them.
 *
 * # Why this is a pane and not a split
 *
 * The canvas argues hard for being a split rather than a third tab, because the
 * whole point of a document canvas is "read this part while I work on it" and
 * there is no version of that where the conversation is on another screen. A
 * notebook is the other case. Writing a note is a thing somebody does *instead*
 * of watching a turn — the sentence is "let me write this down" — and it wants
 * the width. The two mechanisms that made the Data pane cost so much when it
 * arrived, `OnScreen` and `TurnStrip`, both exist now, so the pane is no longer
 * a screen the agent cannot see.
 *
 * # Why the list carries no preview
 *
 * A row is a name and a date. Notes are prose, and the first line of a note is
 * its title — which is already the name in the row, because the name *is* the
 * filename. A snippet under each row would show the same words twice and cost a
 * read of every note in the directory to do it.
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
    choose,
    edit,
    setMode,
    create,
    rename,
    forget,
    keepMine,
    takeTheirs,
  } = notebook;

  /** The name being typed for a new note, and where it will go. */
  const [making, setMaking] = useState<{ scope: Scope; name: string } | null>(null);
  /** Why the last create or rename was refused. Kept beside the box that asked
   *  for the name, which is the only place it means anything. */
  const [refused, setRefused] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  /** Armed once, so a note is never deleted by one stray click. */
  const [arming, setArming] = useState(false);

  const start = useCallback(
    async (scope: Scope, name: string) => {
      try {
        await create(scope, name);
        setMaking(null);
        setRefused(null);
      } catch (e) {
        setRefused(String(e));
      }
    },
    [create],
  );

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
            making={making?.scope === scope ? making.name : null}
            refused={making?.scope === scope ? refused : null}
            onChoose={(p) => {
              setArming(false);
              setRenaming(null);
              choose({ scope: p.scope, name: p.name });
            }}
            onOpen={() => {
              setMaking({ scope, name: "" });
              setRefused(null);
            }}
            onTyping={(name) => setMaking({ scope, name })}
            onStart={(name) => void start(scope, name)}
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
                A <code>mermaid</code> block draws as a diagram in Read.
              </p>
            </>
          )}
        </div>
      ) : (
        <section className="notes-doc" aria-label={`Note: ${page.name}`}>
          <header className="notes-head">
            {renaming === null ? (
              <>
                <span className="notes-name">{page.name}</span>
                <span className="notes-where">
                  {page.scope === "workspace" ? pathOf(page.name) : "global"}
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
                aria-label="Rename this note"
              />
            )}

            <div className="spacer" />

            {/* One word, and only when it is not the resting state. "Saved" that
                never goes away is furniture; "Saving…" for 200ms on every
                keystroke is a flicker. The canvas's rule, and the same words. */}
            {saveState !== "idle" && (
              <span className={`notes-state ${saveState}`}>{SAVE_WORD[saveState]}</span>
            )}

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
           * Somebody else wrote the note while there was typing in the editor.
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
                <b>This note changed while you were typing.</b>
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

          <div className="notes-body">
            {mode === "read" ? (
              <div className="notes-read">
                {/* What has been typed, not what was read: a preview showing the
                    version on disk while the editor holds a newer one would be
                    two answers to the same question on one screen. */}
                <Markdown text={typed} streaming={false} />
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

/** One scope's notes, with the row that makes a new one. */
function Group({
  scope,
  pages,
  open,
  disabled,
  making,
  refused,
  onChoose,
  onOpen,
  onTyping,
  onStart,
  onCancel,
}: {
  scope: Scope;
  pages: PageRef[] | null;
  open: { scope: Scope; name: string } | null;
  disabled: boolean;
  making: string | null;
  refused: string | null;
  onChoose: (page: PageRef) => void;
  /** Opens the row that asks for a name. */
  onOpen: () => void;
  /** The name as it is being typed into that row. */
  onTyping: (name: string) => void;
  onStart: (name: string) => void;
  onCancel: () => void;
}) {
  const mine = (pages ?? []).filter((p) => p.scope === scope);

  return (
    <div className="notes-group">
      <div className="notes-group-head">
        <span className="micro">{scope === "workspace" ? "Project" : "Global"}</span>
        <div className="spacer" />
        {!disabled && (
          <button
            className="notes-new"
            onClick={onOpen}
            aria-label={`New ${scope === "workspace" ? "project" : "global"} note`}
          >
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
              <input
                className="notes-make-name"
                autoFocus
                value={making}
                placeholder="Name it"
                onChange={(e) => onTyping(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") onCancel();
                  if (e.key === "Enter" && making.trim() !== "") onStart(making.trim());
                }}
                aria-label="Name for the new note"
              />
              {refused && <Problem>{refused}</Problem>}
            </div>
          )}

          {mine.length === 0 && making === null && (
            <p className="notes-group-empty">Nothing here yet.</p>
          )}

          <ul className="notes-rows">
            {mine.map((p) => (
              <li key={p.name}>
                <button
                  className="notes-row"
                  data-current={
                    open?.scope === p.scope && open.name === p.name ? "" : undefined
                  }
                  onClick={() => onChoose(p)}
                >
                  <b>{p.name}</b>
                  <span className="micro">{when(p.at)}</span>
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
 * A question mark and nothing after it: the sentence is deliberately unfinished,
 * because the person is about to say what they actually want to know and the
 * cursor is left at the end for them to do it.
 */
export function draftFor(scope: Scope, name: string): string {
  const which = scope === "workspace" ? "project" : "global";
  return `About my ${which} note "${name}": `;
}
