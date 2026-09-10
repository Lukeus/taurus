import type { Scope } from "../bindings/Scope";
import type { TranscriptView } from "../lib/api";
import { pathOf } from "../state/notebook";

type NoteView = Extract<TranscriptView, { type: "note" }>;

/**
 * What an `open_note` call draws in the transcript.
 *
 * `DocumentCard`'s argument, carried to the notebook: a doorway rather than a
 * copy. It holds the notebook and the name and nothing that can go stale, so a
 * conversation from last month opens this month's note.
 *
 * Unlike the canvas, nothing opens by itself. The notes pane replaces the
 * transcript rather than sitting beside it, so the turn that drew this card
 * swapping the screen out from under its own answer would be the app deciding
 * where somebody looks. **Open** is the person deciding.
 */
export function NoteCard({
  view,
  onOpen,
}: {
  view: NoteView;
  /** Opens the note in the notes pane. Absent where there is no pane — inside a
   *  delegate's transcript, which is read on its own. */
  onOpen?: (scope: Scope, name: string) => void;
}) {
  const where =
    view.scope === "workspace" ? pathOf(view.name, "note") : `~/.taurus/notes/${view.name}.md`;
  return (
    <div className="view-card document-card">
      <span className="dataset-mark">✎</span>
      <div className="dataset-copy">
        <b>{view.name}</b>
        <span data-tip={where}>
          {view.scope === "workspace" ? "project note" : "global note"}
        </span>
      </div>
      <div className="spacer" />
      {onOpen && (
        <button className="pill" onClick={() => onOpen(view.scope, view.name)}>
          Open
        </button>
      )}
    </div>
  );
}
