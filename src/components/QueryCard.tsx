import { CopyButton } from "./CopyButton";
import type { TranscriptView } from "../lib/api";

type QueryView = Extract<TranscriptView, { type: "query" }>;

/**
 * What a `query_data` call draws in the transcript.
 *
 * The row above it already said the query ran and what it returned. This is
 * about the question *after* that one: an answer from a query is almost never
 * the end of it — the next thing anybody wants is the same query with a
 * different window, a different grouping, one more column — and asking the
 * model to make that change costs a turn and comes back with something that
 * has to be read before it can be trusted.
 *
 * So the card is a handle on the SQL rather than a second copy of the answer.
 * Taking it to the query box means the next version is typed, run, and seen in
 * the time a round trip would have taken to start.
 *
 * # Why there are no rows here
 *
 * Deliberately, and it is the same argument `DatasetCard` makes. The rows were
 * true of the files as they stood when the call ran; the file underneath can be
 * rewritten by the next turn, by the terminal in the dock, by a `git pull`. A
 * card that redrew last week's answer on reopening would be confidently wrong
 * in exactly the way this feature is arranged against — so it offers to ask
 * again instead, which is cheap and is never stale.
 *
 * # A query that failed draws no card
 *
 * Not a decision made here: the store drops the view off any call the harness
 * refused, because a chart whose series did not line up must not be left on
 * screen beside the word "failed". The argument is weaker for this card — the
 * SQL is the *input*, and a failure does not make it wrong to want it — but
 * the case barely arises. A model handed a `No field named` error fixes it and
 * calls again, so the card that matters appears a moment later; and a query
 * the *person* wrote and could not run has `Ask Taurus` on it already, in the
 * pane, next to the error itself.
 */
export function QueryCard({
  view,
  onRun,
}: {
  view: QueryView;
  /** Puts this query in the Data pane's box and runs it there. Absent where
   *  there is no pane to run it in — inside a delegate's transcript, and in a
   *  workspace with nothing loaded. */
  onRun?: (sql: string) => void;
}) {
  return (
    <div className="view-card query-card">
      <div className="query-card-head">
        <span className="dataset-mark">▦</span>
        <span className="micro">queried the loaded data</span>
        <div className="spacer" />
        <CopyButton className="pill" label="copy" text={() => view.sql} />
        {/* `Run`, not `Open`, unlike the dataset card next to it. This one
            does not just take you somewhere — it asks the question again, at
            full width and with no cell truncated — and a button that quietly
            starts work should say that it does. */}
        {onRun && (
          <button className="pill" onClick={() => onRun(view.sql)}>
            Run in Query
          </button>
        )}
      </div>
      {/* Scrolled rather than clamped past a few lines. A query is read to
          check what it asked, and a fade over the last line of a CTE is the
          part you most need to see. */}
      <pre className="query-card-sql">{view.sql}</pre>
    </div>
  );
}
