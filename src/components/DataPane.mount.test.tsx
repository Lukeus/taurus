// @vitest-environment jsdom
//
// Everything this pane shows arrives after the first paint: a profile is a scan
// of the whole file and a page is a query, so a static render would only ever
// catch the reading state.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { DataPane } from "./DataPane";
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

async function mount(
  datasets: Dataset[] = [EVENTS],
  onForget: (name: string) => void = () => {},
) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <DataPane
        datasets={datasets}
        selected={datasets[0]?.name ?? null}
        onSelect={() => {}}
        onForget={onForget}
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
  /** An empty profile for the tab that renders first, and `answer` for the
   *  query itself — the pane opens on Columns, so a single mock would feed a
   *  query result to the profile view. */
  function answering(answer: () => Promise<unknown>) {
    invoke.mockImplementation((name: string) =>
      name === "query_data"
        ? answer()
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
    host.querySelector(".query-sql") as HTMLTextAreaElement;

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
