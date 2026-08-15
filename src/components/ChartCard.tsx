import { useState } from "react";

import { CopyButton } from "./CopyButton";
import type { TranscriptView } from "../lib/api";

type ChartView = Extract<TranscriptView, { type: "chart" }>;

/**
 * A `show_chart` result.
 *
 * Bars only, on purpose. A model that has just counted something has a
 * categorical series — turns, files, crates — and a line through categories
 * claims a continuity the data does not have. The one case a line would serve
 * better is a long time series, which is also the case that does not fit in a
 * conversation.
 *
 * Several series become tabs rather than several charts stacked: they share the
 * labels by construction, and flipping between them at the same scale is what
 * makes two metrics comparable.
 */
export function ChartCard({ view }: { view: ChartView }) {
  const { title, caption, labels, series } = view;
  const [shown, setShown] = useState(0);
  const [hovered, setHovered] = useState<number | null>(null);

  const active = series[shown] ?? series[0];
  if (!active) return null;

  // Against the largest value rather than zero-to-max, so a series that never
  // dips still shows its shape.
  const max = Math.max(...active.values, 0);
  const total = active.values.reduce((sum, v) => sum + v, 0);
  const peak = active.values.indexOf(max);

  return (
    <div className="view-card">
      <div className="view-head">
        <div>
          <h3>{title}</h3>
          {caption && <p className="view-caption">{caption}</p>}
        </div>
        <div className="spacer" />
        {series.length > 1 && (
          <div className="pill-row chart-tabs">
            {series.map((s, i) => (
              <button
                key={s.name}
                className={`pill${i === shown ? " on" : ""}`}
                onClick={() => setShown(i)}
              >
                {s.name}
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="chart-plot">
        {labels.map((label, i) => {
          const value = active.values[i] ?? 0;
          return (
            <div
              key={`${label}-${i}`}
              className={`chart-bar${hovered === i ? " on" : ""}`}
              onMouseEnter={() => setHovered(i)}
              onMouseLeave={() => setHovered(null)}
            >
              <span className="chart-value">
                {hovered === i ? `${format(value)}${active.unit}` : ""}
              </span>
              <div
                className="chart-fill"
                style={{
                  // Never zero for a real measurement: a bar that rounds away
                  // reads as missing data rather than as a small number.
                  height: max > 0 ? `${Math.max((value / max) * 100, value > 0 ? 2 : 0)}%` : "0%",
                }}
              />
            </div>
          );
        })}
      </div>
      <div className="chart-labels">
        {labels.map((label, i) => (
          <span key={`${label}-${i}`} className={hovered === i ? "on" : undefined}>
            {label}
          </span>
        ))}
      </div>

      <div className="view-foot">
        <span className="micro">
          {format(total)}
          {active.unit} total · peak {labels[peak] ?? "—"}
        </span>
        <div className="spacer" />
        <CopyButton className="pill" label="copy as CSV" text={() => csv(view)} />
      </div>
    </div>
  );
}

/**
 * `42.1` rather than `42.10000000000001`, and `318` rather than `318.0`.
 *
 * Values cross the boundary as JSON numbers, so a count and a duration are
 * indistinguishable by the time they arrive. One decimal with a bare integer
 * left bare covers both without asking the model to declare which it sent.
 */
function format(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

/** Every series, not just the one on screen: the tabs are one dataset. */
function csv(view: ChartView): string {
  const header = ["", ...view.series.map((s) => s.name)];
  const rows = view.labels.map((label, i) => [
    label,
    ...view.series.map((s) => String(s.values[i] ?? "")),
  ]);
  return [header, ...rows]
    .map((row) => row.map(escape).join(","))
    .join("\n");
}

function escape(cell: string): string {
  return /[",\n]/.test(cell) ? `"${cell.replace(/"/g, '""')}"` : cell;
}
