import { useEffect, useState } from "react";

import { FORMAT_LABEL } from "./DatasetCard";
import * as api from "../lib/api";
import type {
  DataColumnProfile,
  DataDistinct,
  DataPage,
  DataProfile,
  Dataset,
} from "../lib/api";

/**
 * The Data pane: what is in the files this workspace has loaded.
 *
 * It takes the centre column rather than a drawer or a dock, and that is the
 * whole of the layout decision. A drawer is where configuration lives — skills,
 * servers, settings — and a dataset is not configuration, it is the work. A
 * dock is where the terminal lives, and a terminal is short while a grid is
 * not: forty columns and thirty rows need the window, not a strip at the
 * bottom of it.
 *
 * What does not move is the rail and the composer. The conversation is still
 * what drives this — you get a dataset here by asking for one — so the box you
 * type into stays exactly where it was, and switching back to the transcript is
 * one click rather than closing something.
 *
 * # Nothing here is cached
 *
 * Every profile and every page is read when it is asked for. A dataset entry is
 * a pointer to a file that anything can rewrite — the agent, a script, the
 * terminal three inches below this pane — and a remembered row count is the
 * exact kind of number that is right for a week and then quietly wrong. The
 * cost is a re-read on every visit, which for the questions this pane asks is
 * cheaper than being wrong once.
 */
export function DataPane({
  datasets,
  selected,
  onSelect,
  onForget,
}: {
  datasets: Dataset[];
  /** Which dataset is open. Held by the caller so a transcript card can
   *  choose it — see `DatasetCard`. */
  selected: string | null;
  onSelect: (name: string) => void;
  onForget: (name: string) => void;
}) {
  const [tab, setTab] = useState<"columns" | "rows">("columns");

  // Falls back to the first, so the pane is never open on nothing while there
  // is something to be open on. A name that no longer exists — a dataset
  // forgotten from under a card that linked to it — lands here too.
  const dataset =
    datasets.find((d) => d.name === selected) ?? datasets[0] ?? null;

  if (!dataset) {
    return (
      <div className="data-pane empty">
        <div className="hero">
          <div className="hero-mark">▦</div>
          <div className="hero-copy">
            <h1>No data loaded here</h1>
            <p>
              Ask Taurus to load a file — a CSV, a TSV, a Parquet file, or
              newline-delimited JSON — and it will appear here with its columns
              read and its shape described.
            </p>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="data-pane">
      <div className="data-head">
        {/* Chips rather than a second sidebar. The rail is already one, and a
            workspace with three datasets does not need a column of its own to
            hold three names. */}
        <div className="data-tabs" role="tablist" aria-label="Datasets">
          {datasets.map((d) => (
            <button
              key={d.name}
              role="tab"
              aria-selected={d.name === dataset.name}
              className={`data-chip${d.name === dataset.name ? " on" : ""}`}
              title={d.path}
              onClick={() => onSelect(d.name)}
            >
              {d.name}
            </button>
          ))}
        </div>

        <div className="spacer" />

        <span className="micro data-source" title={dataset.path}>
          {dataset.path} · {FORMAT_LABEL[dataset.format]}
        </span>
        {/* Unarmed, unlike deleting a conversation. Forgetting a dataset
            removes a pointer and touches no file, so there is nothing to lose
            and nothing to confirm — it is how a mistaken load is corrected. */}
        <button
          className="pill"
          title={`Remove ${dataset.name} from this list. ${dataset.path} is not touched.`}
          onClick={() => onForget(dataset.name)}
        >
          Forget
        </button>
      </div>

      <div className="data-switch">
        <button
          className={`seg${tab === "columns" ? " on" : ""}`}
          onClick={() => setTab("columns")}
        >
          Columns
        </button>
        <button
          className={`seg${tab === "rows" ? " on" : ""}`}
          onClick={() => setTab("rows")}
        >
          Rows
        </button>
      </div>

      {/* Keyed on the dataset so switching between two of them starts the new
          one clean rather than showing the old one's numbers under the new
          one's name while the read is in flight. */}
      {tab === "columns" ? (
        <Columns key={dataset.name} name={dataset.name} />
      ) : (
        <Rows key={dataset.name} name={dataset.name} />
      )}
    </div>
  );
}

/** How many rows one page of the grid holds. */
const PAGE = 100;

function Columns({ name }: { name: string }) {
  const [profile, setProfile] = useState<DataProfile | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    setProfile(null);
    setProblem(null);
    api
      .datasetProfile(name)
      .then((p) => current && setProfile(p))
      .catch((e) => current && setProblem(String(e)));
    return () => {
      current = false;
    };
  }, [name]);

  if (problem) return <p className="data-problem">{problem}</p>;
  if (!profile) {
    // Named rather than a bare spinner. A profile is a full scan, so on a large
    // file this is seconds, and "reading" is the difference between a wait and
    // a hang.
    return <p className="data-reading">Reading {name}…</p>;
  }

  return (
    <div className="data-body">
      <p className="data-summary">
        <b>{profile.rows.toLocaleString()}</b> rows ·{" "}
        <b>{profile.columns.length}</b> columns · read by {profile.engine}
      </p>

      <div className="profile-box">
        <div className="profile-row head">
          <span>Column</span>
          <span>Type</span>
          <span className="right">Missing</span>
          <span className="right">Distinct</span>
          <span>Range</span>
          <span>Most common</span>
        </div>
        {profile.columns.map((column) => (
          <ColumnRow
            key={column.head.name}
            column={column}
            rows={profile.rows}
          />
        ))}
      </div>
    </div>
  );
}

function ColumnRow({
  column,
  rows,
}: {
  column: DataColumnProfile;
  rows: number;
}) {
  const share = rows === 0 ? 0 : column.nulls / rows;
  return (
    <div className="profile-row">
      <span className="profile-name" title={column.head.name}>
        {column.head.name}
      </span>
      <span className="profile-type" title={column.head.type_name}>
        {column.head.type_name}
      </span>

      <span className="right profile-nulls">
        {column.nulls === 0 ? (
          <span className="faint">none</span>
        ) : (
          <>
            {/* The bar is the point. A null count is a number you have to read;
                a bar is a column you can scan a forty-row profile down and stop
                at the one that is wrong. */}
            <span className="profile-bar" aria-hidden>
              <span style={{ width: `${Math.max(share * 100, 2)}%` }} />
            </span>
            {percent(column.nulls, rows)}
          </>
        )}
      </span>

      <span className="right">{distinct(column.distinct)}</span>

      <span
        className="profile-range"
        title={
          column.min !== null && column.max !== null
            ? `${column.min} … ${column.max}`
            : undefined
        }
      >
        {column.min !== null && column.max !== null ? (
          <>
            {column.min} <span className="faint">…</span> {column.max}
          </>
        ) : (
          <span className="faint">—</span>
        )}
      </span>

      <span className="profile-common">{common(column)}</span>
    </div>
  );
}

/**
 * The commonest values, or why there are none to show.
 *
 * The empty case is the one worth handling carefully. An empty list has two
 * causes that mean opposite things — a column with nothing in it, and a column
 * with too much in it — and leaving the cell blank says the first when it is
 * usually the second.
 */
function common(column: DataColumnProfile) {
  if (column.common.length > 0) {
    return column.common.map((value, i) => (
      <span key={i} className="profile-value">
        <b>{value.value === null ? <i>null</i> : value.value || <i>empty</i>}</b>{" "}
        {value.count.toLocaleString()}
      </span>
    ));
  }
  if (column.distinct.kind === "exact" && column.distinct.count > 0) {
    return <span className="faint">too many values to rank</span>;
  }
  return <span className="faint">—</span>;
}

function distinct(value: DataDistinct) {
  return value.kind === "exact" ? (
    value.count.toLocaleString()
  ) : (
    <span className="faint" title="A nested column has no single value to compare">
      nested
    </span>
  );
}

function percent(part: number, whole: number): string {
  if (whole === 0) return "0%";
  const share = (part / whole) * 100;
  // Matches the backend's own rule: a real but tiny share keeps a decimal, so
  // it never renders as the same flat `0%` a clean column shows.
  return share >= 1 || share === 0 ? `${Math.round(share)}%` : `${share.toFixed(1)}%`;
}

function Rows({ name }: { name: string }) {
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<DataPage | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    setProblem(null);
    api
      .datasetPage(name, offset, PAGE)
      .then((p) => current && setPage(p))
      .catch((e) => current && setProblem(String(e)));
    return () => {
      current = false;
    };
  }, [name, offset]);

  if (problem) return <p className="data-problem">{problem}</p>;
  if (!page) return <p className="data-reading">Reading {name}…</p>;

  const last = Math.min(offset + page.rows.length, page.total);
  const template = `3rem repeat(${page.columns.length}, minmax(7rem, 1fr))`;

  return (
    <div className="data-body">
      <p className="data-summary">
        {page.total === 0 ? (
          "no rows"
        ) : (
          <>
            rows <b>{(offset + 1).toLocaleString()}</b>–
            <b>{last.toLocaleString()}</b> of {page.total.toLocaleString()}
          </>
        )}
        <span className="spacer" />
        <button
          className="pill"
          disabled={offset === 0}
          onClick={() => setOffset(Math.max(0, offset - PAGE))}
        >
          ‹ back
        </button>
        <button
          className="pill"
          disabled={last >= page.total}
          onClick={() => setOffset(offset + PAGE)}
        >
          next ›
        </button>
      </p>

      <div className="grid-box">
        <div className="grid-row head" style={{ gridTemplateColumns: template }}>
          <span className="grid-n">#</span>
          {page.columns.map((column) => (
            <span key={column.name} title={column.type_name}>
              {column.name}
            </span>
          ))}
        </div>
        {page.rows.map((row, i) => (
          <div key={i} className="grid-row" style={{ gridTemplateColumns: template }}>
            <span className="grid-n">{(offset + i + 1).toLocaleString()}</span>
            {row.map((cell, j) => (
              <Cell key={j} value={cell} numeric={page.columns[j]?.kind === "number"} />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

/**
 * One cell, with null and the empty string drawn differently.
 *
 * They are the same three pixels of nothing otherwise, and telling them apart
 * is most of what looking at raw rows is for — a column that is 40% missing and
 * a column that is 40% blank string are different problems with different
 * fixes. This is the reason the payload carries `null` rather than `""`.
 */
function Cell({ value, numeric }: { value: string | null; numeric: boolean }) {
  if (value === null) {
    return (
      <span className="grid-cell null" title="No value">
        null
      </span>
    );
  }
  if (value === "") {
    return (
      <span className="grid-cell null" title="An empty string, which is not the same as no value">
        empty
      </span>
    );
  }
  return (
    <span className={`grid-cell${numeric ? " right" : ""}`} title={value}>
      {value}
    </span>
  );
}
