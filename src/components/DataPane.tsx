import { useEffect, useState } from "react";

import { FORMAT_LABEL } from "./DatasetCard";
import { SqlEditor } from "./SqlEditor";
import * as api from "../lib/api";
import type {
  DataColumn,
  DataColumnProfile,
  DataDistinct,
  DataPage,
  DataProfile,
  DataQueryResult,
  DataRun,
  DataTable,
  Dataset,
  Recipe,
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
  onRan,
  tab,
  onTab,
  sql,
  onSql,
  pending,
  onPendingRun,
  onAsk,
}: {
  datasets: Dataset[];
  /** Which dataset is open. Held by the caller so a transcript card can
   *  choose it — see `DatasetCard`. */
  selected: string | null;
  onSelect: (name: string) => void;
  onForget: (name: string) => void;
  /** A recipe wrote a file and loaded it, so the dataset list has changed. */
  onRan: () => void;
  /**
   * What is in the query box.
   *
   * Held by `App` rather than by the box, for two reasons that pull the same
   * way. It survives switching to the conversation and back, which a lazily
   * mounted pane's own state does not. And it is what travels with a message
   * sent from here — "why does this not work?" is a question about the text in
   * the box, and the model cannot see the box.
   */
  sql: string;
  onSql: (sql: string) => void;
  /**
   * Which of the four views is showing.
   *
   * Held by `App` for the same two reasons `sql` is: it survives the round
   * trip to the conversation, which this lazily mounted pane's own state
   * would not, and a card in the transcript can choose it — clicking `Run in
   * Query` has to land on the Query tab and not on whichever one was open
   * last.
   */
  tab: DataTab;
  onTab: (tab: DataTab) => void;
  /**
   * A query to run the moment this appears, from a card in the transcript.
   *
   * An errand rather than a state, which is why it is cleared through
   * `onPendingRun` the instant it is taken. Held by `App` rather than as a
   * counter down here because this pane is unmounted every time the
   * conversation is shown — a token compared against a ref would be forgotten
   * with it, and switching back would re-run a query nobody asked for twice.
   */
  pending: string | null;
  onPendingRun: () => void;
  /**
   * Hands the composer something to say, without sending it.
   *
   * The pane's way of starting a turn. Not sent, because every one of these
   * is a sentence somebody will want to add to — *why does this fail* is
   * usually followed by *and it should be per region* — and a button that
   * fires a turn takes that away. What it does instead is fill the box and
   * put the cursor in it, which is the same thing one keystroke later and is
   * undoable by deleting.
   */
  onAsk: (text: string) => void;
}) {

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
              data-tip={d.path}
              onClick={() => onSelect(d.name)}
            >
              {d.name}
            </button>
          ))}
        </div>

        <div className="spacer" />

        <span className="micro data-source" data-tip={dataset.path}>
          {dataset.path} · {FORMAT_LABEL[dataset.format]}
        </span>
        {/* Unarmed, unlike deleting a conversation. Forgetting a dataset
            removes a pointer and touches no file, so there is nothing to lose
            and nothing to confirm — it is how a mistaken load is corrected. */}
        <button
          className="pill"
          data-tip={`Remove ${dataset.name} from this list. ${dataset.path} is not touched.`}
          onClick={() => onForget(dataset.name)}
        >
          Forget
        </button>
      </div>

      <div className="data-switch">
        <button
          className={`seg${tab === "columns" ? " on" : ""}`}
          onClick={() => onTab("columns")}
        >
          Columns
        </button>
        <button
          className={`seg${tab === "rows" ? " on" : ""}`}
          onClick={() => onTab("rows")}
        >
          Rows
        </button>
        <button
          className={`seg${tab === "query" ? " on" : ""}`}
          onClick={() => onTab("query")}
        >
          Query
        </button>
        <button
          className={`seg${tab === "recipes" ? " on" : ""}`}
          onClick={() => onTab("recipes")}
        >
          Recipes
        </button>
      </div>

      {/* The first two are keyed on the dataset, so switching between two of
          them starts clean rather than showing the old one's numbers under the
          new one's name while the read is in flight.

          Query deliberately is not. A query spans every loaded dataset — that
          is what makes joins possible — so clicking another chip while writing
          one is choosing a table to name, not changing the subject, and
          throwing the half-written SQL away would be the pane disagreeing. */}
      {tab === "columns" && <Columns key={dataset.name} name={dataset.name} />}
      {tab === "rows" && <Rows key={dataset.name} name={dataset.name} />}
      {tab === "query" && (
        <Query
          datasets={datasets}
          sql={sql}
          onSql={onSql}
          pending={pending}
          onPendingRun={onPendingRun}
          onAsk={onAsk}
        />
      )}
      {/* Not scoped to the selected dataset, and not keyed on it. A recipe
          names its own source and can join every other loaded one, so hiding
          the ones whose source is not the open chip would hide most of them
          most of the time. */}
      {tab === "recipes" && <Recipes onRan={onRan} onAsk={onAsk} />}
    </div>
  );
}

/** The four things the pane can be showing. */
export type DataTab = "columns" | "rows" | "query" | "recipes";

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
      <span className="profile-name" data-tip={column.head.name}>
        {column.head.name}
      </span>
      <span className="profile-type" data-tip={column.head.type_name}>
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
        data-tip={
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
    <span className="faint" data-tip="A nested column has no single value to compare">
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

      <Grid columns={page.columns} rows={page.rows} from={offset + 1} />
    </div>
  );
}

/**
 * Columns and rows, drawn.
 *
 * Shared by the paged view of a file and the result of a query, because they
 * are the same picture — and because the one thing that must not differ
 * between them is how a missing value is drawn.
 */
function Grid({
  columns,
  rows,
  from,
}: {
  columns: DataColumn[];
  rows: (string | null)[][];
  /** What to number the first row. A page counts from its offset in the file;
   *  a query result counts from one, because it has no file position. */
  from: number;
}) {
  const template = `3rem repeat(${columns.length}, minmax(7rem, 1fr))`;
  return (
    <div className="grid-box">
      <div className="grid-row head" style={{ gridTemplateColumns: template }}>
        <span className="grid-n">#</span>
        {/* A heading takes its column's alignment, or it is not that column's
            heading: the numbers below sit against the right edge of the track,
            and a label left in the same track reads as belonging to whatever
            is to its left. */}
        {columns.map((column, i) => (
          <span
            key={`${column.name}-${i}`}
            className={column.kind === "number" ? "right" : undefined}
            data-tip={column.type_name}
          >
            {column.name}
          </span>
        ))}
      </div>
      {rows.map((row, i) => (
        <div key={i} className="grid-row" style={{ gridTemplateColumns: template }}>
          <span className="grid-n">{(from + i).toLocaleString()}</span>
          {row.map((cell, j) => (
            <Cell key={j} value={cell} numeric={columns[j]?.kind === "number"} />
          ))}
        </div>
      ))}
    </div>
  );
}

/**
 * Ad-hoc SQL over every loaded dataset.
 *
 * The pane's answer to the questions a profile cannot reach — how many users
 * bought more than once, which category refunds most, whether two files agree
 * on a key. Every dataset is a table under its own name, so a join is just a
 * join.
 *
 * Read-only is not enforced here and must not be: this box takes whatever is
 * typed into it, and `COPY … TO` is one line of SQL. The refusal lives in the
 * engine, where the tool the model calls goes through the same gate — see
 * `taurus_data::Engine::query`. A frontend check would be a second rule to
 * keep in step with the real one.
 */
function Query({
  datasets,
  sql,
  onSql,
  pending,
  onPendingRun,
  onAsk,
}: {
  /** Which datasets are loaded. Only used to notice that the set has changed,
   *  so the schemas are read again — the columns themselves come from the
   *  backend, because a name is not a schema. */
  datasets: Dataset[];
  /** Held by `App`, not here. See the note where it is declared. */
  sql: string;
  onSql: (sql: string) => void;
  /** A query handed over from the transcript, to be run on arrival. */
  pending: string | null;
  onPendingRun: () => void;
  onAsk: (text: string) => void;
}) {
  const [result, setResult] = useState<DataQueryResult | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  /**
   * The SQL that produced what is showing below.
   *
   * Not the same as what is in the box, and the difference matters exactly
   * once: the button that asks Taurus about a failure has to quote the query
   * that failed, and by the time anybody clicks it they have usually already
   * started editing. The box keeps showing the error while that happens, which
   * is right — the edit is being made *because* of it.
   */
  const [ran, setRan] = useState("");
  /**
   * Every table this query can name, with its columns.
   *
   * Read here rather than passed in, and read again whenever the set of
   * datasets changes. It is the cheap half of what the pane knows how to ask
   * — a Parquet footer, a CSV header — where a profile is a full scan, which
   * is why completion can afford this on every visit and could not afford
   * that. Empty until it lands, which costs the first keystroke its
   * suggestions and nothing else.
   */
  const [schemas, setSchemas] = useState<DataTable[]>([]);
  const [showSchema, setShowSchema] = useState(false);

  const loaded = datasets.map((d) => d.name).join(",");
  useEffect(() => {
    let live = true;
    api
      .datasetTables()
      .then((found) => live && setSchemas(found))
      // Silently. This feeds completion and a reference panel, and a workspace
      // whose files have moved should still be able to type a query and be
      // told what is wrong by the engine, in the one message that knows.
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [loaded]);

  const run = async (text = sql) => {
    if (!text.trim() || running) return;
    setRunning(true);
    setProblem(null);
    setRan(text);
    try {
      setResult(await api.queryData(text));
    } catch (e) {
      // The previous result is dropped rather than left standing under a new
      // error, which would read as the answer to the query that just failed.
      setResult(null);
      setProblem(String(e));
    } finally {
      setRunning(false);
    }
  };

  /*
   * A query sent over from a card in the transcript, run as it lands.
   *
   * `App` has already put it in the box; this is the half that makes the
   * button say `Run` rather than `Open`. Cleared through the caller before the
   * query is even started, so a run in flight when the pane is closed and
   * reopened is not silently started a second time.
   */
  useEffect(() => {
    if (pending === null) return;
    onPendingRun();
    void run(pending);
    // Deliberately only on the errand itself. `run` closes over state that
    // changes while it runs, and re-firing on any of that would be a loop.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pending]);

  return (
    <div className="data-body">
      <div className="query-box">
        <SqlEditor
          value={sql}
          onChange={onSql}
          onRun={() => void run()}
          tables={schemas}
          // Names a table that is actually here. The shape of a `SELECT` is
          // the easy half; which file this workspace happens to have loaded is
          // the half worth being told without asking.
          placeholder={
            datasets[0] ? `SELECT * FROM ${datasets[0].name} LIMIT 20` : "SELECT …"
          }
        />
        <div className="query-foot">
          {/* Was a line of text naming the tables; it is a control now,
              because with two files loaded the next question is always which
              of them has the column you half-remember. */}
          <button
            className="pill query-tables"
            aria-expanded={showSchema}
            onClick={() => setShowSchema(!showSchema)}
            data-tip="What each table has in it"
          >
            <span className="recipe-caret">{showSchema ? "▾" : "▸"}</span>
            {tableLine(schemas, datasets)}
          </button>
          <div className="spacer" />
          <span className="micro">⌃space complete · ⌘↵ run</span>
          <button
            className="primary"
            disabled={running || !sql.trim()}
            onClick={() => void run()}
          >
            {running ? "Running…" : "Run"}
          </button>
        </div>
      </div>

      {showSchema && <Schema tables={schemas} />}

      {problem && (
        <p className="data-problem">
          {problem}
          {/* The one button in the pane that exists because of a specific
              failure we watched happen: a query written against `material`
              when the column is `"Material"`, refused with a message that says
              exactly what to do and is nine words too long to want to retype.
              Handing the whole thing to the model is faster than reading it,
              and the model is the one that has to learn the lesson anyway. */}
          <button className="pill" onClick={() => onAsk(askAboutFailure(ran, problem))}>
            Ask Taurus
          </button>
        </p>
      )}

      {result && !problem && (
        <>
          <p className="data-summary">
            <b>{result.rows.length.toLocaleString()}</b>{" "}
            {result.rows.length === 1 ? "row" : "rows"}
            {/* Said, not left to be inferred. A result that filled the cap and
                one that is the whole answer look identical. */}
            {result.truncated && <> · capped — add a LIMIT or aggregate</>}
            {" · "}
            {result.took_ms.toLocaleString()} ms
            <span className="spacer" />
            {/* Offered on a query that worked, and nowhere else. A step in a
                recipe is a query somebody has decided to keep, and the moment
                that decision gets made is the moment the right rows come back
                — not before, when it does not run, and not from the transcript
                a week later, when the reason it mattered is gone. */}
            <button className="pill" onClick={() => onAsk(askForAStep(ran))}>
              Make this a step
            </button>
          </p>
          {result.rows.length > 0 ? (
            <Grid columns={result.columns} rows={result.rows} from={1} />
          ) : (
            <p className="data-reading">The query matched nothing.</p>
          )}
        </>
      )}

      {!result && !problem && (
        <p className="data-reading">
          SELECT only. Anything that writes is refused — that is what a recipe
          is for.
        </p>
      )}
    </div>
  );
}

/**
 * What each table has in it, and which columns two of them share.
 *
 * Reference, not input: nothing here is clickable, and that is a decision
 * rather than an omission. Completion is the way text gets into the box —
 * ⌃space, or just keep typing — and a second insertion route would be a second
 * set of rules about where the caret goes. What this answers is the question
 * completion cannot, which is *what is there at all*: you cannot type the first
 * three letters of a column you have never seen.
 *
 * The line at the top is the reason it is worth opening. Two files that share a
 * column are two files that can be joined on it, and finding that out otherwise
 * means reading two profiles side by side and holding forty names in your head.
 */
function Schema({ tables }: { tables: DataTable[] }) {
  const shared = joinable(tables);
  return (
    <div className="schema-box">
      {shared.length > 0 && (
        <p className="micro schema-join">
          Shared by more than one table:{" "}
          {shared.map((name, i) => (
            <span key={name}>
              {i > 0 && ", "}
              <code>{name}</code>
            </span>
          ))}
        </p>
      )}
      {tables.map((table) => (
        <div className="schema-table" key={table.name}>
          <div className="schema-name">
            <b>{table.name}</b>
            <span className="micro" data-tip={table.path}>
              {/* A CSV has no row count until something reads it, and this is
                  the call that refuses to. Saying nothing is better than
                  saying a number the footer did not have. */}
              {table.rows === null
                ? `${table.columns.length} columns`
                : `${table.rows.toLocaleString()} rows · ${table.columns.length} columns`}
            </span>
          </div>
          <div className="schema-columns">
            {table.columns.map((column) => (
              <span
                key={column.name}
                className={`schema-column${shared.includes(column.name) ? " shared" : ""}`}
                data-tip={`${column.type_name}${
                  shared.includes(column.name) ? " · more than one table has this" : ""
                }`}
              >
                {column.name}
                <i>{column.type_name}</i>
              </span>
            ))}
          </div>
        </div>
      ))}
      {tables.length === 0 && (
        <p className="micro">Reading what each table has in it…</p>
      )}
    </div>
  );
}

/** Column names more than one loaded table has, in the order the first table
 *  lists them — which is the order somebody reading a profile met them in. */
function joinable(tables: DataTable[]): string[] {
  const seen = new Map<string, number>();
  for (const table of tables) {
    for (const name of new Set(table.columns.map((c) => c.name))) {
      seen.set(name, (seen.get(name) ?? 0) + 1);
    }
  }
  return [...seen].filter(([, n]) => n > 1).map(([name]) => name);
}

/**
 * The label on the disclosure, which has to say something true before the
 * schemas have arrived.
 *
 * The dataset list is in hand immediately and the schema read is a round trip
 * behind it, so for one frame there are names and no columns. Naming the
 * tables from whichever list is further along keeps the control from flashing
 * `no tables` at a workspace that has two.
 */
function tableLine(schemas: DataTable[], datasets: Dataset[]): string {
  const names = (schemas.length > 0 ? schemas : datasets).map((t) => t.name);
  if (names.length === 0) return "no tables";
  return names.length === 1 ? `table: ${names[0]}` : `tables: ${names.join(", ")}`;
}

/*
 * What the pane's three buttons put in the composer.
 *
 * Written out here, together, because they are the pane's only prose and the
 * one thing about this feature a model reads. Three rules they all keep:
 *
 * They quote the SQL or name the file. A message that said "this query fails"
 * and nothing else would rely on the context the composer attaches — which
 * reaches the model but not the transcript, so a conversation reopened next
 * month would be a question about nothing. Everything needed to understand
 * these is inside them.
 *
 * They end with the ask, not the evidence. The last line is where a person
 * appends — *and it should be per region* — and it is where the model's
 * attention lands.
 *
 * They are a draft, not a message. None of them is sent; each fills the box
 * and waits. So they are phrased as an opening rather than as a complete
 * instruction, which is what makes adding a sentence read naturally instead of
 * as a contradiction.
 */

/** Why a query in the box did not run. */
export function askAboutFailure(sql: string, problem: string): string {
  return `This query fails:\n\n${fence(sql)}\n\n${problem.trim()}\n\nWhat should it be?`;
}

/** A query worth keeping, offered to a recipe. */
export function askForAStep(sql: string): string {
  return `Add this as a step in a recipe:\n\n${fence(sql)}`;
}

/** A recipe that ran and did not finish. */
export function askAboutRun(name: string, path: string, problem: string): string {
  return `Running the ${name} recipe failed:\n\n${problem.trim()}\n\nHave a look at ${path}.`;
}

/** SQL in a fence, so the model reads it as SQL and the transcript draws it as
 *  a block rather than as four unindented lines of prose. */
function fence(sql: string): string {
  return `\`\`\`sql\n${sql.trim()}\n\`\`\``;
}

/**
 * The recipes this workspace has, and the button that runs one.
 *
 * A recipe is the difference between having looked at some data and having a
 * dataset: a committed chain of SQL steps that produces the same file from
 * next month's export without anybody remembering what was decided. So this
 * list is read off `.taurus/recipes` every time it is opened rather than
 * cached — the file is in the project, and the agent, an editor, or a `git
 * pull` can all have changed it since.
 *
 * # Why Run does not confirm
 *
 * It writes a file, and it asks nothing first. The same arrangement the query
 * box has: the person clicked it, and the button says where it writes, so a
 * dialog repeating the path back would be asking somebody to agree with
 * themselves. What is still refused — one layer down, in the engine — is a
 * *step* that writes somewhere other than the path on the button. That
 * refusal is not repeated here, for the reason the query box does not repeat
 * its own: a second rule in the frontend is a rule that drifts.
 */
function Recipes({
  onRan,
  onAsk,
}: {
  onRan: () => void;
  onAsk: (text: string) => void;
}) {
  const [recipes, setRecipes] = useState<Recipe[] | null>(null);
  const [problems, setProblems] = useState<string[]>([]);
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    void (async () => {
      try {
        const answer = await api.listRecipes();
        if (!live) return;
        setRecipes(answer.recipes);
        setProblems(answer.problems);
      } catch (e) {
        if (live) setFailed(String(e));
      }
    })();
    return () => {
      live = false;
    };
  }, []);

  if (failed) return <p className="data-problem">{failed}</p>;
  if (!recipes) return <p className="data-reading">Reading recipes…</p>;

  return (
    <div className="data-body">
      {recipes.length === 0 && problems.length === 0 && (
        <div className="recipe-blank">
          <p>No recipes in this workspace yet.</p>
          <p className="micro">
            A recipe is a <code>.sql</code> file in{" "}
            <code>.taurus/recipes</code> — a <code>source:</code> and an{" "}
            <code>output:</code> between <code>---</code> lines, then one{" "}
            <code>SELECT</code> per step, each under a{" "}
            <code>-- step: what it does</code> line. Every step reads from{" "}
            <code>input</code>, which is the step before it. Ask Taurus to
            write one.
          </p>
        </div>
      )}

      {recipes.map((recipe) => (
        <RecipeCard
          key={recipe.name}
          recipe={recipe}
          onRan={onRan}
          onAsk={onAsk}
        />
      ))}

      {/* Below the list rather than instead of it: a file somebody is halfway
          through writing should not hide the four that work. */}
      {problems.map((problem) => (
        <p key={problem} className="data-problem">
          {problem}
        </p>
      ))}
    </div>
  );
}

function RecipeCard({
  recipe,
  onRan,
  onAsk,
}: {
  recipe: Recipe;
  onRan: () => void;
  onAsk: (text: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [running, setRunning] = useState(false);
  const [run, setRun] = useState<DataRun | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const go = async () => {
    if (running) return;
    setRunning(true);
    setProblem(null);
    try {
      setRun(await api.runRecipe(recipe.name));
      // The output is loaded as a dataset on the way out, so the chips above
      // this and the count on the rail have both just changed.
      onRan();
    } catch (e) {
      setRun(null);
      setProblem(String(e));
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="recipe">
      <div className="recipe-head">
        <button
          className="recipe-name"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          data-tip={recipe.path}
        >
          <span className="recipe-caret">{open ? "▾" : "▸"}</span>
          <b>{recipe.name}</b>
        </button>
        <span className="micro recipe-flow">
          {recipe.source} → {recipe.output}
        </span>
        <div className="spacer" />
        <span className="micro">
          {recipe.steps.length} {recipe.steps.length === 1 ? "step" : "steps"}
        </span>
        {/* The path is on the button, not in a dialog after it. This writes a
            file, and what it writes is the thing worth reading before
            clicking. */}
        <button
          className="primary"
          disabled={running}
          data-tip={`Run ${recipe.steps.length} steps over ${recipe.source} and write ${recipe.output}`}
          onClick={() => void go()}
        >
          {running ? "Running…" : `Run → ${recipe.output}`}
        </button>
      </div>

      {recipe.description && (
        <p className="micro recipe-note">{recipe.description}</p>
      )}

      {open && recipe.tables.length > 0 && (
        <p className="micro recipe-note">
          {/* Shown only when opened, because it is a detail of what the steps
              can see rather than of what the run writes — but shown, because a
              recipe that joins against another file is reading something the
              header line does not name. */}
          also reads{" "}
          {recipe.tables.map((t, i) => (
            <span key={t.name}>
              {i > 0 && ", "}
              <code>{t.name}</code> from {t.path}
            </span>
          ))}
        </p>
      )}

      {open && (
        <ol className="recipe-steps">
          {recipe.steps.map((step, i) => (
            <li key={i}>
              <span className="recipe-step-title">{step.title}</span>
              <pre className="recipe-sql">{step.sql}</pre>
            </li>
          ))}
        </ol>
      )}

      {problem && (
        <p className="data-problem">
          {problem}
          {/* The failure this most often answers is a step that will not plan
              — a column named as the model remembered it rather than as the
              file spells it — and the fix is one line in a file the model
              wrote and can read again. The recipe's path goes in the message
              because the name alone is not enough to open it. */}
          <button
            className="pill"
            onClick={() => onAsk(askAboutRun(recipe.name, recipe.path, problem))}
          >
            Fix this
          </button>
        </p>
      )}
      {run && !problem && <RunReport run={run} output={recipe.output} />}
    </div>
  );
}

/**
 * What a run did, step by step.
 *
 * The deltas are the reason this exists rather than a single "done". A step
 * meant to drop a hundred duplicates that dropped four hundred thousand rows
 * is invisible in the SQL and unmissable in this column, and finding that out
 * a week later from a model that trained on the result is the failure the
 * whole feature is arranged against.
 */
function RunReport({ run, output }: { run: DataRun; output: string }) {
  let previous = run.started_with;
  const rows = run.steps.map((step) => {
    const change = step.rows - previous;
    previous = step.rows;
    return { step, change };
  });

  return (
    <div className="recipe-run">
      <div className="recipe-run-row start">
        <span className="recipe-run-n" />
        <span className="recipe-run-title">before any step</span>
        <span className="recipe-run-rows">{run.started_with.toLocaleString()}</span>
        <span className="recipe-run-delta" />
        <span className="recipe-run-ms" />
      </div>
      {rows.map(({ step, change }, i) => (
        <div className="recipe-run-row" key={i}>
          <span className="recipe-run-n">{i + 1}</span>
          <span className="recipe-run-title">{step.title}</span>
          <span className="recipe-run-rows">{step.rows.toLocaleString()}</span>
          <span className={`recipe-run-delta${change < 0 ? " down" : change > 0 ? " up" : ""}`}>
            {/* A step that changed nothing says so. A `0` here reads as a row
                count rather than as a difference. */}
            {change === 0
              ? "—"
              : `${change < 0 ? "−" : "+"}${Math.abs(change).toLocaleString()}`}
          </span>
          <span className="recipe-run-ms">{step.took_ms.toLocaleString()} ms</span>
        </div>
      ))}
      <p className="micro recipe-run-foot">
        Wrote <b>{run.rows.toLocaleString()}</b> rows ×{" "}
        {run.columns.length.toLocaleString()} columns to <code>{output}</code> ·{" "}
        {size(run.bytes)} · {run.took_ms.toLocaleString()} ms
      </p>
    </div>
  );
}

/** A file size as a person reads it. */
function size(bytes: number): string {
  const MB = 1024 * 1024;
  if (bytes >= 1024 * MB) return `${(bytes / (1024 * MB)).toFixed(1)} GB`;
  if (bytes >= MB) return `${(bytes / MB).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} bytes`;
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
      <span className="grid-cell null" data-tip="No value">
        null
      </span>
    );
  }
  if (value === "") {
    return (
      <span className="grid-cell null" data-tip="An empty string, which is not the same as no value">
        empty
      </span>
    );
  }
  // Verbatim. The engine already rendered this cell, and re-formatting it here
  // would corrupt every value that only looks like a number: a zero-padded
  // product code, an id, a version string. Grouping separators are for the
  // counts the pane computes itself, which are real numbers.
  return (
    <span className={`grid-cell${numeric ? " right" : ""}`} data-tip={value}>
      {value}
    </span>
  );
}
