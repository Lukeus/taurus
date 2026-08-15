import { useMemo, useState } from "react";

import { CopyButton } from "./CopyButton";
import type { TranscriptView } from "../lib/api";

type TableView = Extract<TranscriptView, { type: "table" }>;

/**
 * A `show_table` result.
 *
 * Sorting is the whole reason this is a component rather than a markdown table.
 * The model sends the rows in whatever order it gathered them, which is rarely
 * the order the question wants them in, and re-asking for the same rows sorted
 * differently costs a round trip and a rewrite of something already correct.
 *
 * The sort lives here and nowhere else: nothing is sent back, and reopening the
 * conversation shows the model's order again. That is the honest arrangement —
 * the transcript records what the model said, and how the reader chose to look
 * at it is not part of the conversation.
 */
export function TableCard({ view }: { view: TableView }) {
  const { title, caption, columns, rows } = view;
  const [sort, setSort] = useState<{ column: number; descending: boolean } | null>(
    null,
  );

  const ordered = useMemo(() => {
    if (!sort) return rows;
    const { column, descending } = sort;
    const numeric = columns[column]?.kind !== "text";
    // Copied first: `rows` belongs to the entry in the store, and sorting it in
    // place would reorder the transcript's own record of what the model sent.
    return [...rows].sort((a, b) => {
      const cmp = numeric
        ? compareNumbers(a[column], b[column])
        : (a[column] ?? "").localeCompare(b[column] ?? "");
      return descending ? -cmp : cmp;
    });
  }, [rows, columns, sort]);

  // Text columns get the room, because a path or a crate name is what runs out
  // of it; a number column is as wide as its widest number and no wider.
  const template = columns
    .map((column) => (column.kind === "text" ? "1.5fr" : "0.8fr"))
    .join(" ");

  return (
    <div className="view-card">
      <div className="view-head">
        <div>
          <h3>{title}</h3>
          {caption && <p className="view-caption">{caption}</p>}
        </div>
        <div className="spacer" />
        <span className="micro">{plural(rows.length, "row")}</span>
      </div>

      <div className="table-box">
        <div className="table-row head" style={{ gridTemplateColumns: template }}>
          {columns.map((column, i) => (
            <button
              key={i}
              className={`table-sort${sort?.column === i ? " on" : ""}${
                column.kind === "text" ? "" : " right"
              }`}
              onClick={() =>
                setSort((current) =>
                  current?.column === i
                    ? { column: i, descending: !current.descending }
                    : // First click on a column sorts descending: for the
                      // numbers a table like this holds — durations, counts,
                      // regressions — the interesting end is the top.
                      { column: i, descending: true },
                )
              }
              title={`Sort by ${column.label}`}
            >
              {column.label}
              {sort?.column === i && (sort.descending ? " ↓" : " ↑")}
            </button>
          ))}
        </div>
        {ordered.map((row, i) => (
          <div
            key={i}
            className="table-row"
            style={{ gridTemplateColumns: template }}
          >
            {row.map((cell, j) => (
              <span key={j} className={cellClass(columns[j]?.kind, cell)}>
                {cell}
              </span>
            ))}
          </div>
        ))}
      </div>

      <div className="view-foot">
        <span className="micro">
          {sort
            ? `sorted by ${columns[sort.column]?.label.toLowerCase() ?? ""}`
            : "click a column to sort"}
        </span>
        <div className="spacer" />
        {/* Serialized from the model's rows rather than the sorted ones: what
            gets pasted into a spreadsheet should be the data, and a spreadsheet
            can sort it again. */}
        <CopyButton className="pill" label="copy as CSV" text={() => csv(view)} />
      </div>
    </div>
  );
}

function cellClass(kind: string | undefined, cell: string): string {
  if (kind === "delta") return `table-cell delta ${direction(cell)}`;
  return kind === "text" ? "table-cell" : "table-cell right";
}

/**
 * Which way a delta points, read off the text the model formatted.
 *
 * The cell is already written for a human — `+22%`, `-8`, `—` — so the sign is
 * found rather than computed. Up is tinted as the bad direction because every
 * delta a table like this carries is a cost: build time, error count, bundle
 * size. Nothing here can tell a cost from a benefit, so it commits to the
 * common case rather than colouring at random.
 */
function direction(cell: string): "up" | "down" | "flat" {
  const first = cell.trim()[0];
  if (first === "+") return "up";
  if (first === "-" || first === "−") return "down";
  return "flat";
}

/**
 * Compares two cells by the number inside them, with anything unparseable last.
 *
 * Cells arrive pre-formatted — `42.1s`, `+22%`, `1.2 MB` — because the model
 * knows what the units are and the frontend does not. So the number is pulled
 * back out for sorting rather than being asked for twice.
 */
function compareNumbers(a: string, b: string): number {
  const left = parseNumber(a);
  const right = parseNumber(b);
  if (left === null && right === null) return 0;
  if (left === null) return -1;
  if (right === null) return 1;
  return left - right;
}

function parseNumber(cell: string): number | null {
  const match = cell?.replace(/−/g, "-").match(/-?\d+(\.\d+)?/);
  return match ? Number(match[0]) : null;
}

function csv(view: TableView): string {
  const lines = [
    view.columns.map((c) => escape(c.label)).join(","),
    ...view.rows.map((row) => row.map(escape).join(",")),
  ];
  return lines.join("\n");
}

function escape(cell: string): string {
  return /[",\n]/.test(cell) ? `"${cell.replace(/"/g, '""')}"` : cell;
}

function plural(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}
