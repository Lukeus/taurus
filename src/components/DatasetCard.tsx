import { useEffect, useRef } from "react";

import type { Dataset, TranscriptView } from "../lib/api";
import { useStore } from "../state/store";

type DatasetView = Extract<TranscriptView, { type: "dataset" }>;

/**
 * What a `load_dataset` or `profile_dataset` call draws in the transcript.
 *
 * A reference, not a result — the one card here that is. Every other view is
 * the answer it carries: a table has its rows in it, a chart has its numbers.
 * This one has a name, and the thing it names is a million rows in a file that
 * would be worthless in a conversation and is worth a great deal in a pane. So
 * the card is the doorway and nothing more.
 *
 * That is also what keeps the transcript readable once data work starts. A turn
 * that loads four files and profiles two leaves six lines of prose and six small
 * cards, rather than six screens of somebody else's spreadsheet.
 */
export function DatasetCard({
  view,
  onOpen,
}: {
  view: DatasetView;
  /** Shows this dataset in the Data pane. Absent where there is no pane to
   *  show it in — inside a delegate's transcript, which is read on its own. */
  onOpen?: (name: string) => void;
}) {
  const datasets = useStore((s) => s.datasets);
  const refresh = useStore((s) => s.refreshDatasets);
  const dataset = datasets.find((d) => d.name === view.name);

  // One catch-up read, and only when this card cannot find what it names.
  //
  // The list is refreshed by the turn that loads a dataset, so the ordinary
  // path never gets here. What does is a conversation reopened before the
  // first fetch has landed — the entries are drawn from the transcript
  // immediately and the list arrives a round trip later. Guarded so a card
  // naming a dataset that genuinely no longer exists asks once rather than on
  // every render for the rest of the session.
  const asked = useRef(false);
  useEffect(() => {
    if (dataset || asked.current) return;
    asked.current = true;
    void refresh();
  }, [dataset, refresh]);

  if (!dataset) {
    return (
      <div className="view-card dataset-card gone">
        <span className="dataset-mark">▦</span>
        <div className="dataset-copy">
          <b>{view.name}</b>
          {/* Not an error. Forgetting a dataset is an ordinary correction, and
              a transcript records what happened at the time either way. */}
          <span>is not loaded in this workspace</span>
        </div>
      </div>
    );
  }

  return (
    <div className="view-card dataset-card">
      <span className="dataset-mark">▦</span>
      <div className="dataset-copy">
        <b>{dataset.name}</b>
        <span title={dataset.path}>
          {dataset.path} · {FORMAT_LABEL[dataset.format]}
        </span>
      </div>
      <div className="spacer" />
      {onOpen && (
        <button className="pill" onClick={() => onOpen(dataset.name)}>
          Open in Data
        </button>
      )}
    </div>
  );
}

/** What each format is called on screen. */
export const FORMAT_LABEL: Record<Dataset["format"], string> = {
  csv: "CSV",
  tsv: "TSV",
  parquet: "Parquet",
  ndjson: "NDJSON",
};
