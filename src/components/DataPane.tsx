import { useEffect, useState } from "react";

import { FORMAT_LABEL } from "./DatasetCard";
import * as api from "../lib/api";
import type {
  DataColumn,
  DataColumnProfile,
  DataDistinct,
  DataPage,
  DataProfile,
  DataQueryResult,
  DataRun,
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
}: {
  datasets: Dataset[];
  /** Which dataset is open. Held by the caller so a transcript card can
   *  choose it — see `DatasetCard`. */
  selected: string | null;
  onSelect: (name: string) => void;
  onForget: (name: string) => void;
  /** A recipe wrote a file and loaded it, so the dataset list has changed. */
  onRan: () => void;
}) {
  const [tab, setTab] = useState<"columns" | "rows" | "query" | "recipes">(
    "columns",
  );

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
        <button
          className={`seg${tab === "query" ? " on" : ""}`}
          onClick={() => setTab("query")}
        >
          Query
        </button>
        <button
          className={`seg${tab === "recipes" ? " on" : ""}`}
          onClick={() => setTab("recipes")}
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
        <Query tables={datasets.map((d) => d.name)} start={dataset.name} />
      )}
      {/* Not scoped to the selected dataset, and not keyed on it. A recipe
          names its own source and can join every other loaded one, so hiding
          the ones whose source is not the open chip would hide most of them
          most of the time. */}
      {tab === "recipes" && <Recipes onRan={onRan} />}
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
        {columns.map((column, i) => (
          <span key={`${column.name}-${i}`} title={column.type_name}>
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
function Query({ tables, start }: { tables: string[]; start: string }) {
  // Seeded once, from whichever dataset was selected when the tab was first
  // opened. Not re-seeded on every chip click: this component is deliberately
  // unkeyed so that choosing another table to name does not throw away the
  // query being written.
  const [sql, setSql] = useState(() => `SELECT * FROM ${start} LIMIT 20`);
  const [result, setResult] = useState<DataQueryResult | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [running, setRunning] = useState(false);

  const run = async () => {
    if (!sql.trim() || running) return;
    setRunning(true);
    setProblem(null);
    try {
      setResult(await api.queryData(sql));
    } catch (e) {
      // The previous result is dropped rather than left standing under a new
      // error, which would read as the answer to the query that just failed.
      setResult(null);
      setProblem(String(e));
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="data-body">
      <div className="query-box">
        <textarea
          className="query-sql"
          value={sql}
          spellCheck={false}
          rows={4}
          aria-label="SQL"
          onChange={(e) => setSql(e.target.value)}
          onKeyDown={(e) => {
            // Enter is a newline: this is SQL, and a query worth writing is
            // more than one line. ⌘↵ and ⌃↵ run it, as every SQL client does.
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void run();
            }
          }}
        />
        <div className="query-foot">
          <span className="micro">
            {tables.length === 1
              ? `table: ${tables[0]}`
              : `tables: ${tables.join(", ")}`}
          </span>
          <div className="spacer" />
          <span className="micro">⌘↵ run</span>
          <button
            className="primary"
            disabled={running || !sql.trim()}
            onClick={() => void run()}
          >
            {running ? "Running…" : "Run"}
          </button>
        </div>
      </div>

      {problem && <p className="data-problem">{problem}</p>}

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
function Recipes({ onRan }: { onRan: () => void }) {
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
        <RecipeCard key={recipe.name} recipe={recipe} onRan={onRan} />
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
}: {
  recipe: Recipe;
  onRan: () => void;
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
          title={recipe.path}
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
          title={`Run ${recipe.steps.length} steps over ${recipe.source} and write ${recipe.output}`}
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

      {problem && <p className="data-problem">{problem}</p>}
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
  // Verbatim. The engine already rendered this cell, and re-formatting it here
  // would corrupt every value that only looks like a number: a zero-padded
  // product code, an id, a version string. Grouping separators are for the
  // counts the pane computes itself, which are real numbers.
  return (
    <span className={`grid-cell${numeric ? " right" : ""}`} title={value}>
      {value}
    </span>
  );
}
