import type { LineRange, TranscriptView } from "../lib/api";

type DocumentView = Extract<TranscriptView, { type: "document" }>;

/**
 * What an `open_file` call draws in the transcript.
 *
 * The second card here that is a reference rather than a result — see
 * `DatasetCard` for the first, and the same argument one step further on. A
 * dataset card carries a name because the rows belong in a pane; this carries a
 * path because the *file* belongs on disk, where it goes on being edited after
 * the conversation has moved on.
 *
 * So the card holds nothing that can go stale. It is a doorway, and it opens
 * onto whatever the file says today — which is the right answer rather than a
 * compromise: a conversation from last month that says "opened `parser.rs` at
 * line 240" should show today's line 240, because the reason to reopen it is to
 * find out whether the thing discussed is still true.
 *
 * Unlike the dataset card it never renders a "gone" state. A file that has been
 * moved or deleted fails when it is opened, in the canvas, with the path in the
 * message — and looking it up here to say so early would mean a disk read per
 * card on every scroll through a long conversation.
 */
export function DocumentCard({
  view,
  onOpen,
}: {
  view: DocumentView;
  /** Opens the file in the canvas. Absent where there is no canvas to open it
   *  in — inside a delegate's transcript, which is read on its own. */
  onOpen?: (path: string, lines: LineRange | null) => void;
}) {
  const name = view.path.split("/").pop() ?? view.path;
  const folder = view.path.includes("/")
    ? view.path.slice(0, view.path.lastIndexOf("/"))
    : null;

  return (
    <div className="view-card document-card">
      <span className="dataset-mark">¶</span>
      <div className="dataset-copy">
        <b>{name}</b>
        <span data-tip={view.path}>
          {folder ? `${folder} · ` : ""}
          {whereIn(view.lines ?? null)}
        </span>
      </div>
      <div className="spacer" />
      {onOpen && (
        <button
          className="pill"
          onClick={() => onOpen(view.path, view.lines ?? null)}
        >
          Open
        </button>
      )}
    </div>
  );
}

/**
 * How the card names the place in the file.
 *
 * Exported because it is the only decision in this component and it reads
 * better as a tested function than as a nested ternary in JSX. One line says
 * "line 40" rather than "lines 40–40", which is the sort of thing nobody
 * notices until it is wrong.
 */
export function whereIn(lines: LineRange | null): string {
  if (!lines) return "opened";
  return lines.from === lines.to
    ? `line ${lines.from}`
    : `lines ${lines.from}–${lines.to}`;
}
