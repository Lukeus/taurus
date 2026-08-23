// @vitest-environment jsdom
//
// Everything this pane shows arrives after the first paint: a profile is a scan
// of the whole file and a page is a query, so a static render would only ever
// catch the reading state.
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { DataPane, type DataTab } from "./DataPane";
import type { DataColumnProfile, Dataset } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

beforeEach(() => {
  invoke.mockReset();
});

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

const EVENTS: Dataset = {
  name: "events",
  path: "data/events.csv",
  format: "csv",
};

function column(
  name: string,
  over: Partial<DataColumnProfile> = {},
): DataColumnProfile {
  return {
    head: { name, kind: "text", type_name: "Utf8", nullable: true },
    nulls: 0,
    distinct: { kind: "exact", count: 3 },
    min: null,
    max: null,
    common: [],
    ...over,
  };
}

/**
 * The pane with the state its caller holds, held here instead.
 *
 * `tab`, `sql` and the pending query all live in `App` in the real thing — see
 * the notes on the props — so a test that passed them as constants could not
 * click a tab. This is the smallest stand-in for that caller: it keeps the
 * three, and it is the reason the assertions below can click through the pane
 * the way somebody using it does.
 */
function Harness({
  datasets,
  onForget,
  onRan,
  sql: initialSql,
  pending,
  onAsk,
}: {
  datasets: Dataset[];
  onForget: (name: string) => void;
  onRan: () => void;
  sql: string;
  pending: string | null;
  onAsk: (text: string) => void;
}) {
  const [tab, setTab] = useState<DataTab>("columns");
  const [sql, setSql] = useState(initialSql);
  const [errand, setErrand] = useState(pending);
  return (
    <DataPane
      datasets={datasets}
      selected={datasets[0]?.name ?? null}
      onSelect={() => {}}
      onForget={onForget}
      onRan={onRan}
      tab={tab}
      onTab={setTab}
      sql={sql}
      onSql={setSql}
      pending={errand}
      onPendingRun={() => setErrand(null)}
      onAsk={onAsk}
    />
  );
}

async function mount(
  datasets: Dataset[] = [EVENTS],
  onForget: (name: string) => void = () => {},
  onRan: () => void = () => {},
  sql = "SELECT * FROM events LIMIT 20",
  over: { pending?: string; onAsk?: (text: string) => void } = {},
) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <Harness
        datasets={datasets}
        onForget={onForget}
        onRan={onRan}
        sql={sql}
        pending={over.pending ?? null}
        onAsk={over.onAsk ?? (() => {})}
      />,
    );
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return host;
}

describe("the Data pane with nothing loaded", () => {
  it("says how a dataset gets here rather than showing an empty table", async () => {
    const host = await mount([]);
    expect(host.textContent).toContain("No data loaded");
    // The route in is asking, which is the whole model of the app — there is
    // deliberately no "add a file" button anywhere in this pane.
    expect(host.textContent).toContain("Ask Taurus to load a file");
    expect(invoke).not.toHaveBeenCalled();
  });
});

describe("the columns view", () => {
  it("draws a row per column with what was counted", async () => {
    invoke.mockResolvedValue({
      rows: 1_240_913,
      engine: "DataFusion",
      columns: [
        column("event", {
          distinct: { kind: "exact", count: 4 },
          min: "click",
          max: "view",
          common: [{ value: "view", count: 500_000 }],
        }),
        column("price", {
          head: {
            name: "price",
            kind: "number",
            type_name: "Float64",
            nullable: true,
          },
          nulls: 1_204,
          distinct: { kind: "exact", count: 3_912 },
        }),
      ],
    });

    const host = await mount();

    expect(invoke).toHaveBeenCalledWith("dataset_profile", { name: "events" });
    expect(host.textContent).toContain("1,240,913");
    // The engine is named on the numbers it produced, not assumed.
    expect(host.textContent).toContain("DataFusion");
    expect(host.textContent).toContain("click");
    expect(host.textContent).toContain("view");
    expect(host.textContent).toContain("3,912");
  });

  /**
   * The bar is the reason the missing column exists at all, and the tiny-share
   * case is the one it is for: 1,204 of 1.2 million rounds to zero, and a flat
   * `0%` beside a clean column is exactly the reading somebody must not get.
   */
  it("keeps a small but real share of nulls visible", async () => {
    invoke.mockResolvedValue({
      rows: 1_240_913,
      engine: "DataFusion",
      columns: [column("price", { nulls: 1_204 })],
    });

    const host = await mount();

    expect(host.textContent).toContain("0.1%");
    const bar = host.querySelector(".profile-bar span") as HTMLElement | null;
    expect(bar).not.toBeNull();
    // Floored rather than drawn to scale, or a real gap would be zero pixels.
    expect(parseFloat(bar!.style.width)).toBeGreaterThan(0);
  });

  it("distinguishes a column with no common values from one with too many", async () => {
    invoke.mockResolvedValue({
      rows: 100,
      engine: "DataFusion",
      columns: [
        column("user_id", {
          distinct: { kind: "exact", count: 100 },
          common: [],
        }),
        column("payload", { distinct: { kind: "unavailable" }, common: [] }),
      ],
    });

    const host = await mount();

    // An empty list has two causes that mean opposite things, and a blank cell
    // says the wrong one.
    expect(host.textContent).toContain("too many values to rank");
    expect(host.textContent).toContain("nested");
  });

  it("says the file could not be read rather than sitting on a spinner", async () => {
    invoke.mockRejectedValue(
      new Error("data/events.csv could not be read as CSV: unexpected EOF"),
    );
    const host = await mount();
    expect(host.querySelector(".data-problem")?.textContent).toContain(
      "could not be read as CSV",
    );
  });
});

describe("the rows view", () => {
  async function rowsTab(page: unknown) {
    invoke.mockImplementation((name: string) =>
      name === "dataset_page"
        ? Promise.resolve(page)
        : Promise.resolve({ rows: 3, engine: "DataFusion", columns: [] }),
    );
    const host = await mount();
    const tab = [...host.querySelectorAll("button.seg")].find(
      (b) => b.textContent === "Rows",
    ) as HTMLButtonElement;
    await act(async () => {
      tab.click();
    });
    return host;
  }

  /**
   * The payoff of every cell being `string | null` all the way from the engine.
   * Both draw as nothing otherwise, and a column that is 40% missing and one
   * that is 40% blank string are different problems with different fixes.
   */
  it("draws a null and an empty string differently", async () => {
    const host = await rowsTab({
      columns: [{ name: "name", kind: "text", type_name: "Utf8", nullable: true }],
      rows: [["alice"], [null], [""]],
      offset: 0,
      total: 3,
    });

    const cells = [...host.querySelectorAll(".grid-cell")].map((c) => ({
      text: c.textContent,
      missing: c.classList.contains("null"),
    }));
    expect(cells).toEqual([
      { text: "alice", missing: false },
      { text: "null", missing: true },
      { text: "empty", missing: true },
    ]);
  });

  it("says where in the file the page is", async () => {
    const host = await rowsTab({
      columns: [{ name: "id", kind: "number", type_name: "Int64", nullable: false }],
      rows: [["1"], ["2"]],
      offset: 0,
      total: 1_240_913,
    });
    expect(host.textContent).toContain("1,240,913");
    // Back is dead on the first page; next is not, with a million rows behind.
    const [back, next] = [...host.querySelectorAll(".data-summary .pill")];
    expect((back as HTMLButtonElement).disabled).toBe(true);
    expect((next as HTMLButtonElement).disabled).toBe(false);
  });

  it("cannot page past the end of a short file", async () => {
    const host = await rowsTab({
      columns: [{ name: "id", kind: "number", type_name: "Int64", nullable: false }],
      rows: [["1"], ["2"]],
      offset: 0,
      total: 2,
    });
    const [, next] = [...host.querySelectorAll(".data-summary .pill")];
    expect((next as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("the query view", () => {
  const TABLES = [
    {
      name: "events",
      path: "data/events.csv",
      rows: null,
      columns: [
        { name: "user_id", kind: "number", type_name: "Int64", nullable: true },
        { name: "event", kind: "text", type_name: "Utf8", nullable: true },
      ],
    },
    {
      name: "items",
      path: "data/items.parquet",
      rows: 4_200,
      columns: [
        { name: "user_id", kind: "number", type_name: "Int64", nullable: false },
        { name: "price", kind: "number", type_name: "Float64", nullable: true },
      ],
    },
  ];

  /** An empty profile for the tab that renders first, and `answer` for the
   *  query itself — the pane opens on Columns, so a single mock would feed a
   *  query result to the profile view. */
  function answering(answer: () => Promise<unknown>, schemas: unknown[] = TABLES) {
    invoke.mockImplementation((name: string) =>
      name === "query_data"
        ? answer()
        : // The schema read the completion list runs on. A list, not a
          // profile: the two commands answer different shapes and the box
          // asks for both.
          name === "dataset_tables"
          ? Promise.resolve(schemas)
          : Promise.resolve({ rows: 0, engine: "DataFusion", columns: [] }),
    );
  }

  async function queryTab(datasets: Dataset[] = [EVENTS]) {
    const host = await mount(datasets);
    const tab = [...host.querySelectorAll("button.seg")].find(
      (b) => b.textContent === "Query",
    ) as HTMLButtonElement;
    await act(async () => {
      tab.click();
    });
    return host;
  }

  const sqlBox = (host: HTMLElement) =>
    host.querySelector(".sql-input") as HTMLTextAreaElement;

  async function run(host: HTMLElement) {
    const button = [...host.querySelectorAll("button")].find((b) =>
      b.textContent?.startsWith("Run"),
    ) as HTMLButtonElement;
    await act(async () => {
      button.click();
    });
  }

  it("starts on the selected dataset and names every table that can be joined", async () => {
    answering(() => Promise.resolve(null));
    const host = await queryTab([
      EVENTS,
      { name: "items", path: "data/items.parquet", format: "parquet" },
    ]);
    expect(sqlBox(host).value).toBe("SELECT * FROM events LIMIT 20");
    // Both, because a query spans them — that is what makes a join possible,
    // and a hint naming only the selected one would hide it.
    expect(host.textContent).toContain("tables: events, items");
  });

  it("draws the answer, with what it cost", async () => {
    answering(() =>
      Promise.resolve({
        columns: [
          { name: "event", kind: "text", type_name: "Utf8", nullable: false },
          { name: "n", kind: "number", type_name: "Int64", nullable: false },
        ],
        rows: [
          ["view", "219922"],
          ["click", "100189"],
        ],
        truncated: false,
        took_ms: 41,
      }),
    );

    const host = await queryTab();
    await run(host);

    expect(invoke).toHaveBeenCalledWith("query_data", {
      sql: "SELECT * FROM events LIMIT 20",
    });
    expect(host.textContent).toContain("2 rows");
    expect(host.textContent).toContain("41 ms");
    // Verbatim, not `219,922`. A cell arrives already rendered by the engine,
    // and re-formatting one would corrupt every value that only looks like a
    // number — a zero-padded code, an id, a version. Grouping separators are
    // for the counts the pane computes itself.
    expect(host.textContent).toContain("219922");
  });

  // A result that filled the cap and one that is the whole answer look
  // identical, so the difference has to be said.
  it("says when the answer was capped", async () => {
    answering(() =>
      Promise.resolve({
        columns: [{ name: "id", kind: "number", type_name: "Int64", nullable: false }],
        rows: Array.from({ length: 30 }, (_, i) => [String(i)]),
        truncated: true,
        took_ms: 12,
      }),
    );
    const host = await queryTab();
    await run(host);
    expect(host.textContent).toContain("capped");
  });

  it("shows the engine's refusal rather than a frontend guess at one", async () => {
    // The pane deliberately does not check the SQL itself. A second rule here
    // would be one more thing to keep in step with the real one, and this box
    // is not where the guarantee lives.
    answering(() =>
      Promise.reject(
        new Error(
          "that is not a read-only query. `query_data` runs SELECT and nothing else — no COPY.",
        ),
      ),
    );
    const host = await queryTab();
    await run(host);
    expect(host.querySelector(".data-problem")?.textContent).toContain("COPY");
  });

  it("drops the previous answer when the next query fails", async () => {
    answering(() =>
      Promise.resolve({
        columns: [{ name: "n", kind: "number", type_name: "Int64", nullable: false }],
        rows: [["7"]],
        truncated: false,
        took_ms: 3,
      }),
    );
    const host = await queryTab();
    await run(host);
    expect(host.querySelector(".grid-box")).not.toBeNull();

    // Left standing, the old grid reads as the answer to the query that just
    // failed.
    answering(() =>
      Promise.reject(new Error("that query will not run: no column 'nope'")),
    );
    await run(host);
    expect(host.querySelector(".grid-box")).toBeNull();
    expect(host.textContent).toContain("nope");
  });

  /*
   * The trip in from the transcript. `App` puts the SQL in the box and hands
   * the pane the errand; the pane's job is to spend it once and only once.
   */
  it("runs a query handed over from a card, without being asked twice", async () => {
    answering(() =>
      Promise.resolve({
        columns: [{ name: "n", kind: "number", type_name: "Int64", nullable: false }],
        rows: [["3"]],
        truncated: false,
        took_ms: 4,
      }),
    );
    const host = await mount([EVENTS], () => {}, () => {}, "SELECT 3", {
      pending: "SELECT 3",
    });
    const tab = [...host.querySelectorAll("button.seg")].find(
      (b) => b.textContent === "Query",
    ) as HTMLButtonElement;
    await act(async () => {
      tab.click();
    });
    // Ran on arrival — the card's button says `Run`, and it has to mean it.
    expect(host.textContent).toContain("1 row");
    const asked = invoke.mock.calls.filter(([name]) => name === "query_data");
    expect(asked).toEqual([["query_data", { sql: "SELECT 3" }]]);

    // And the errand is spent. Leaving the pane and coming back re-mounts
    // everything in it, which is exactly the case a token held down here would
    // get wrong.
    const columns = [...host.querySelectorAll("button.seg")].find(
      (b) => b.textContent === "Columns",
    ) as HTMLButtonElement;
    await act(async () => {
      columns.click();
    });
    await act(async () => {
      tab.click();
    });
    expect(
      invoke.mock.calls.filter(([name]) => name === "query_data"),
    ).toHaveLength(1);
  });

  it("offers a failure to Taurus, quoting the query that failed", async () => {
    answering(() =>
      Promise.reject(
        new Error(
          `No field named s.material. Did you mean 's."Material"'? Column names are case sensitive.`,
        ),
      ),
    );
    const drafts: string[] = [];
    const host = await mount([EVENTS], () => {}, () => {}, "SELECT s.material FROM s", {
      onAsk: (text) => drafts.push(text),
    });
    const tab = [...host.querySelectorAll("button.seg")].find(
      (b) => b.textContent === "Query",
    ) as HTMLButtonElement;
    await act(async () => {
      tab.click();
    });
    await run(host);

    const ask = [...host.querySelectorAll(".data-problem button")].find(
      (b) => b.textContent === "Ask Taurus",
    ) as HTMLButtonElement;
    await act(async () => {
      ask.click();
    });

    expect(drafts).toHaveLength(1);
    // Both halves, and the reason both are needed: the context the composer
    // attaches reaches the model but never the transcript, so a message that
    // leant on it would be a question about nothing when reopened.
    expect(drafts[0]).toContain("SELECT s.material FROM s");
    expect(drafts[0]).toContain("case sensitive");
    // Nothing is sent. The next thing typed is the half only the person knows.
    expect(invoke).not.toHaveBeenCalledWith("send_message", expect.anything());
  });

  /** The button quotes what ran, not what is in the box — by the time anybody
   *  clicks it they have usually started editing, and quoting the half-fixed
   *  version would ask about a query that never failed. */
  it("quotes the query that failed even after the box has moved on", async () => {
    answering(() => Promise.reject(new Error("no column 'nope'")));
    const drafts: string[] = [];
    const host = await mount([EVENTS], () => {}, () => {}, "SELECT nope FROM events", {
      onAsk: (text) => drafts.push(text),
    });
    const tab = [...host.querySelectorAll("button.seg")].find(
      (b) => b.textContent === "Query",
    ) as HTMLButtonElement;
    await act(async () => {
      tab.click();
    });
    await run(host);

    const box = sqlBox(host);
    await act(async () => {
      box.value = "SELECT nope_2 FROM events";
      box.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const ask = [...host.querySelectorAll(".data-problem button")].find(
      (b) => b.textContent === "Ask Taurus",
    ) as HTMLButtonElement;
    await act(async () => {
      ask.click();
    });
    expect(drafts[0]).toContain("SELECT nope FROM events");
    expect(drafts[0]).not.toContain("nope_2");
  });

  it("offers a query that worked to a recipe, and one that did not to nobody", async () => {
    answering(() =>
      Promise.resolve({
        columns: [{ name: "n", kind: "number", type_name: "Int64", nullable: false }],
        rows: [["7"]],
        truncated: false,
        took_ms: 3,
      }),
    );
    const drafts: string[] = [];
    const host = await mount([EVENTS], () => {}, () => {}, "SELECT count(*) FROM events", {
      onAsk: (text) => drafts.push(text),
    });
    const tab = [...host.querySelectorAll("button.seg")].find(
      (b) => b.textContent === "Query",
    ) as HTMLButtonElement;
    await act(async () => {
      tab.click();
    });
    // Nothing to keep until something has come back.
    expect(host.textContent).not.toContain("Make this a step");

    await run(host);
    const keep = [...host.querySelectorAll("button")].find(
      (b) => b.textContent === "Make this a step",
    ) as HTMLButtonElement;
    await act(async () => {
      keep.click();
    });
    expect(drafts[0]).toContain("recipe");
    expect(drafts[0]).toContain("SELECT count(*) FROM events");
  });

  it("names a table that is actually loaded in the empty box", async () => {
    // The shape of a SELECT is the easy half. Which file this workspace has is
    // the half worth being told without having to ask.
    answering(() => Promise.resolve(null));
    const host = await mount([EVENTS], () => {}, () => {}, "");
    const tab = [...host.querySelectorAll("button.seg")].find(
      (b) => b.textContent === "Query",
    ) as HTMLButtonElement;
    await act(async () => {
      tab.click();
    });
    expect(sqlBox(host).placeholder).toBe("SELECT * FROM events LIMIT 20");
  });

  /*
   * The reference half, next to the completion half. You cannot type the first
   * three letters of a column you have never seen, which is what this answers
   * and what a completion list structurally cannot.
   */
  it("shows what each table holds, and marks the columns two of them share", async () => {
    answering(() => Promise.resolve(null));
    const host = await queryTab();
    expect(host.querySelector(".schema-box")).toBeNull();

    const disclosure = host.querySelector(".query-tables") as HTMLButtonElement;
    expect(disclosure.textContent).toContain("tables: events, items");
    await act(async () => {
      disclosure.click();
    });

    const panel = host.querySelector(".schema-box") as HTMLElement;
    expect(panel.textContent).toContain("price");
    expect(panel.textContent).toContain("Float64");
    // A Parquet footer carries a count; a CSV does not, and inventing one
    // would be the panel claiming something this call refuses to read.
    expect(panel.textContent).toContain("4,200 rows · 2 columns");

    // The join key, tinted where the columns are rather than listed somewhere
    // else that has to be looked at as well.
    const shared = [...panel.querySelectorAll(".schema-column.shared")].map(
      (e) => e.textContent,
    );
    expect(shared).toHaveLength(2);
    expect(shared.every((text) => text?.startsWith("user_id"))).toBe(true);
    expect(panel.querySelector(".schema-join")?.textContent).toContain("user_id");
  });

  it("says so when the query matched nothing, rather than drawing an empty table", async () => {
    answering(() =>
      Promise.resolve({
        columns: [{ name: "n", kind: "number", type_name: "Int64", nullable: false }],
        rows: [],
        truncated: false,
        took_ms: 5,
      }),
    );
    const host = await queryTab();
    await run(host);
    expect(host.textContent).toContain("matched nothing");
  });
});

describe("forgetting a dataset", () => {
  it("asks nothing first, and says the file is untouched", async () => {
    // Unlike deleting a conversation, which arms. Forgetting removes a pointer
    // and destroys nothing, so a confirmation would be ceremony over an action
    // whose whole purpose is correcting a mistake.
    invoke.mockResolvedValue({ rows: 0, engine: "DataFusion", columns: [] });
    const forgotten: string[] = [];
    const host = await mount([EVENTS], (name) => forgotten.push(name));

    const button = [...host.querySelectorAll("button.pill")].find(
      (b) => b.textContent === "Forget",
    ) as HTMLButtonElement;
    expect(button.title).toContain("is not touched");

    await act(async () => {
      button.click();
    });
    expect(forgotten).toEqual(["events"]);
  });
});

describe("the recipes view", () => {
  const CLEAN = {
    name: "clean",
    source: "events",
    output: "data/clean.parquet",
    description: "drop duplicates and the rows with no user",
    path: ".taurus/recipes/clean.sql",
    tables: [],
    steps: [
      { title: "drop exact duplicates", sql: "SELECT DISTINCT * FROM input" },
      {
        title: "keep the rows that name a user",
        sql: "SELECT * FROM input WHERE user_id IS NOT NULL",
      },
    ],
  };

  /** Routes each command to its own answer. One blanket `mockResolvedValue`
   *  feeds a recipe list to `dataset_profile`, which crashes the Columns tab
   *  the pane opens on. */
  function answering(over: Record<string, () => Promise<unknown>>) {
    invoke.mockImplementation((name: string) => {
      const answer = over[name];
      if (answer) return answer();
      if (name === "list_recipes")
        return Promise.resolve({ recipes: [], problems: [] });
      return Promise.resolve({ rows: 0, engine: "DataFusion", columns: [] });
    });
  }

  async function recipesTab(
    datasets: Dataset[] = [EVENTS],
    onAsk: (text: string) => void = () => {},
  ) {
    const host = await mount(datasets, () => {}, () => {}, "", { onAsk });
    const tab = [...host.querySelectorAll("button.seg")].find(
      (b) => b.textContent === "Recipes",
    ) as HTMLButtonElement;
    await act(async () => {
      tab.click();
    });
    return host;
  }

  it("says what a recipe is when there are none, rather than showing a blank", async () => {
    answering({});
    const host = await recipesTab();
    expect(host.textContent).toContain("No recipes");
    // The route in is asking, the same as loading a dataset.
    expect(host.textContent).toContain("Ask Taurus to write one");
    expect(host.textContent).toContain(".taurus/recipes");
  });

  it("lists a recipe with what it reads and where it writes", async () => {
    answering({
      list_recipes: () =>
        Promise.resolve({ recipes: [CLEAN], problems: [] }),
    });
    const host = await recipesTab();
    expect(host.textContent).toContain("clean");
    expect(host.textContent).toContain("events → data/clean.parquet");
    expect(host.textContent).toContain("2 steps");
    expect(host.textContent).toContain("drop duplicates and the rows");
  });

  /** The button writes a file and asks nothing first, so the path it writes
   *  has to be on the button rather than in a dialog after it. */
  it("names the file the run will write on the button itself", async () => {
    answering({
      list_recipes: () =>
        Promise.resolve({ recipes: [CLEAN], problems: [] }),
    });
    const host = await recipesTab();
    const button = [...host.querySelectorAll("button.primary")].find((b) =>
      b.textContent?.startsWith("Run"),
    ) as HTMLButtonElement;
    expect(button.textContent).toContain("data/clean.parquet");
    expect(button.title).toContain("2 steps over events");
  });

  it("shows the SQL of each step when the recipe is opened", async () => {
    answering({
      list_recipes: () =>
        Promise.resolve({ recipes: [CLEAN], problems: [] }),
    });
    const host = await recipesTab();
    expect(host.textContent).not.toContain("SELECT DISTINCT");
    await act(async () => {
      (host.querySelector(".recipe-name") as HTMLButtonElement).click();
    });
    expect(host.textContent).toContain("SELECT DISTINCT * FROM input");
    expect(host.textContent).toContain("WHERE user_id IS NOT NULL");
  });

  /** A recipe that joins against another file is reading something the header
   *  line does not name, so opening it has to say so. */
  it("names the extra files a recipe binds for itself", async () => {
    answering({
      list_recipes: () =>
        Promise.resolve({
          recipes: [
            {
              ...CLEAN,
              tables: [{ name: "items", path: "data/catalogue.parquet" }],
            },
          ],
          problems: [],
        }),
    });
    const host = await recipesTab();
    expect(host.textContent).not.toContain("data/catalogue.parquet");
    await act(async () => {
      (host.querySelector(".recipe-name") as HTMLButtonElement).click();
    });
    expect(host.textContent).toContain("also reads");
    expect(host.textContent).toContain("data/catalogue.parquet");
  });

  it("reports what each step did to the row count", async () => {
    answering({
      list_recipes: () =>
        Promise.resolve({ recipes: [CLEAN], problems: [] }),
      run_recipe: () =>
        Promise.resolve({
          started_with: 400000,
          steps: [
            { title: "drop exact duplicates", rows: 398412, columns: 4, took_ms: 211 },
            {
              title: "keep the rows that name a user",
              rows: 219922,
              columns: 4,
              took_ms: 180,
            },
          ],
          columns: [
            { name: "id", kind: "number", type_name: "Int64", nullable: false },
          ],
          rows: 219922,
          bytes: 3_250_000,
          took_ms: 612,
        }),
    });
    const host = await recipesTab([EVENTS]);

    const button = [...host.querySelectorAll("button.primary")].find((b) =>
      b.textContent?.startsWith("Run"),
    ) as HTMLButtonElement;
    await act(async () => {
      button.click();
    });

    // The deltas are the whole reason this is reported per step. A step meant
    // to drop a hundred duplicates that dropped four hundred thousand rows is
    // invisible in the SQL and unmissable here.
    expect(host.textContent).toContain("400,000");
    expect(host.textContent).toContain("−1,588");
    expect(host.textContent).toContain("−178,490");
    expect(host.textContent).toContain("3.1 MB");
  });

  /*
   * The other half of a failed run. The message is usually a step that will
   * not plan — a column named as the model remembered it rather than as the
   * file spells it — and the fix is one line in a file the model wrote.
   */
  it("offers a failed run to Taurus, naming the file to open", async () => {
    answering({
      list_recipes: () =>
        Promise.resolve({ recipes: [CLEAN], problems: [] }),
      run_recipe: () =>
        Promise.reject("step 1 (costs) will not run: No field named s.material."),
    });
    const drafts: string[] = [];
    const host = await recipesTab([EVENTS], (text) => drafts.push(text));
    const button = [...host.querySelectorAll("button.primary")].find((b) =>
      b.textContent?.startsWith("Run"),
    ) as HTMLButtonElement;
    await act(async () => {
      button.click();
    });

    const fix = [...host.querySelectorAll(".data-problem button")].find(
      (b) => b.textContent === "Fix this",
    ) as HTMLButtonElement;
    await act(async () => {
      fix.click();
    });
    expect(drafts).toHaveLength(1);
    expect(drafts[0]).toContain("clean");
    // The path, not just the name: the name alone is not enough to open it.
    expect(drafts[0]).toContain(".taurus/recipes/clean.sql");
    expect(drafts[0]).toContain("No field named s.material");
  });

  /** A file somebody is halfway through writing should not hide the four that
   *  work. */
  it("shows a broken recipe beside the ones that parsed", async () => {
    answering({
      list_recipes: () =>
        Promise.resolve({
          recipes: [CLEAN],
          problems: [".taurus/recipes/torn.sql: there are no steps."],
        }),
    });
    const host = await recipesTab();
    expect(host.textContent).toContain("clean");
    expect(host.textContent).toContain("torn.sql");
  });

  it("tells the dataset list to refresh, because a run loads what it wrote", async () => {
    answering({
      list_recipes: () =>
        Promise.resolve({ recipes: [CLEAN], problems: [] }),
      run_recipe: () =>
        Promise.resolve({
          started_with: 5,
          steps: [{ title: "one", rows: 5, columns: 2, took_ms: 3 }],
          columns: [],
          rows: 5,
          bytes: 900,
          took_ms: 9,
        }),
    });
    let refreshed = 0;
    const host = await mount([EVENTS], () => {}, () => {
      refreshed += 1;
    });
    const tab = [...host.querySelectorAll("button.seg")].find(
      (b) => b.textContent === "Recipes",
    ) as HTMLButtonElement;
    await act(async () => {
      tab.click();
    });
    const button = [...host.querySelectorAll("button.primary")].find((b) =>
      b.textContent?.startsWith("Run"),
    ) as HTMLButtonElement;
    await act(async () => {
      button.click();
    });
    expect(refreshed).toBe(1);
  });

  it("keeps a failed run's error where the recipe is, and drops no other recipe", async () => {
    answering({
      list_recipes: () =>
        Promise.resolve({ recipes: [CLEAN], problems: [] }),
      run_recipe: () =>
        Promise.reject("step 2 (typo) will not run: No field named nope."),
    });
    const host = await recipesTab();
    const button = [...host.querySelectorAll("button.primary")].find((b) =>
      b.textContent?.startsWith("Run"),
    ) as HTMLButtonElement;
    await act(async () => {
      button.click();
    });
    expect(host.textContent).toContain("step 2 (typo)");
    expect(host.textContent).toContain("clean");
  });
});
