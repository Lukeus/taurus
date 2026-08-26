// @vitest-environment jsdom
//
// The parts of the box that only exist once it is wired up: the painted layer
// tracking what was typed, and the completion list opening, moving and being
// taken. `lib/sql.test.ts` covers what it decides to offer; this covers what
// happens when somebody presses a key.
//
// What jsdom cannot check is where the list *lands* — there is no layout, so
// every measurement is zero. That is the `query-run` screenshot's job; see
// `docs/development.md`.
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { SqlEditor } from "./SqlEditor";
import type { DataTable } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

const TABLES: DataTable[] = [
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
    name: "users",
    path: "data/users.parquet",
    rows: 4_200,
    columns: [
      { name: "user_id", kind: "number", type_name: "Int64", nullable: false },
      { name: "country", kind: "text", type_name: "Utf8", nullable: true },
    ],
  },
];

/** The editor with somewhere for its text to live, which is `App` in the real
 *  thing — a controlled box handed a constant cannot be typed into. */
function Harness({ start, onRun }: { start: string; onRun: () => void }) {
  const [sql, setSql] = useState(start);
  return (
    <SqlEditor value={sql} onChange={setSql} onRun={onRun} tables={TABLES} />
  );
}

function mount(start = "", onRun: () => void = () => {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => {
    root.render(<Harness start={start} onRun={onRun} />);
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return host;
}

const box = (host: HTMLElement) =>
  host.querySelector(".sql-input") as HTMLTextAreaElement;

/** Types, the way React hears it: the value set through the prototype so the
 *  tracker does not swallow the event, then an `input`. */
function type(host: HTMLElement, text: string, caret = text.length) {
  const area = box(host);
  const set = Object.getOwnPropertyDescriptor(
    HTMLTextAreaElement.prototype,
    "value",
  )!.set!;
  act(() => {
    set.call(area, text);
    area.setSelectionRange(caret, caret);
    area.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function press(host: HTMLElement, key: string, over: KeyboardEventInit = {}) {
  act(() => {
    box(host).dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...over }),
    );
  });
}

const rows = (host: HTMLElement) =>
  [...host.querySelectorAll(".sql-label")].map((e) => e.textContent);

describe("the painted layer", () => {
  /*
   * The property the whole arrangement rests on. The `<pre>` sits under a
   * transparent textarea; if it ever held different text the colour would
   * slide off the query and keep sliding.
   */
  it("holds exactly what the box holds", () => {
    const host = mount("select * from events where event = 'a'");
    const painted = host.querySelector(".sql-ink") as HTMLElement;
    // One trailing newline, which a `<pre>` swallows — without it a query
    // ending in a newline would paint a line short.
    expect(painted.textContent).toBe(box(host).value + "\n");

    type(host, "select 1 -- and a comment\n");
    expect(painted.textContent).toBe(box(host).value + "\n");
  });

  it("tints the keywords and leaves the identifiers alone", () => {
    const host = mount("SELECT event FROM events");
    const painted = host.querySelector(".sql-ink") as HTMLElement;
    expect(
      [...painted.querySelectorAll(".ink-keyword")].map((e) => e.textContent),
    ).toEqual(["SELECT", "FROM"]);
  });
});

describe("finishing a word", () => {
  it("offers what fits as soon as there is something to fit", () => {
    const host = mount();
    expect(host.querySelector(".sql-menu")).toBeNull();
    type(host, "SELECT coun");
    // The column this workspace has, then the function everybody was going to
    // type next anyway. Identifiers first — the vocabulary is the half that
    // does not need looking up.
    expect(rows(host)).toEqual(["country", "count()"]);
  });

  it("takes the highlighted one on Enter and leaves the caret after it", () => {
    const host = mount();
    type(host, "SELECT coun");
    press(host, "Enter");
    expect(box(host).value).toBe("SELECT country");
    expect(host.querySelector(".sql-menu")).toBeNull();
  });

  it("takes one on Tab as well, which is the other habit", () => {
    const host = mount();
    type(host, "SELECT coun");
    press(host, "Tab");
    expect(box(host).value).toBe("SELECT country");
  });

  it("walks the list with the arrows and wraps at the end", () => {
    const host = mount();
    type(host, "SELECT user");
    // The table first — a table name is typed once and its columns many
    // times, so the one time is worth putting at the top — then `user_id`
    // from each file, which are two suggestions because they are two columns.
    expect(rows(host)).toEqual(["users", "user_id", "user_id"]);
    const chosen = () =>
      [...host.querySelectorAll(".sql-choice")].findIndex((b) =>
        b.classList.contains("on"),
      );
    expect(chosen()).toBe(0);
    press(host, "ArrowDown");
    expect(chosen()).toBe(1);
    // Off the top wraps to the bottom, so the last item is one key away.
    press(host, "ArrowUp");
    press(host, "ArrowUp");
    expect(chosen()).toBe(rows(host).length - 1);
  });

  /*
   * The whole reason this was built. Two files sharing a column are two files
   * that can be joined on it, and the list is where that is cheapest to
   * notice — while writing the join rather than by reading two profiles.
   */
  it("says which table each column came from, and marks the shared ones", () => {
    const host = mount();
    type(host, "SELECT user_");
    expect(
      [...host.querySelectorAll(".sql-note")].map((e) => e.textContent),
    ).toEqual(["events · Int64", "users · Int64"]);
    expect(host.querySelectorAll(".sql-shared")).toHaveLength(2);

    type(host, "SELECT countr");
    expect(host.querySelector(".sql-shared")).toBeNull();
  });

  it("offers the aggregate functions, bracket and all", () => {
    const host = mount();
    type(host, "SELECT av");
    expect(rows(host)).toContain("avg()");
    press(host, "Enter");
    // The caret goes after the bracket, which is where the argument goes.
    expect(box(host).value).toBe("SELECT avg(");
  });

  it("narrows to one table's columns after its alias", () => {
    const host = mount();
    type(host, "SELECT * FROM events e JOIN users u ON u.");
    expect(rows(host)).toEqual(["country", "user_id"]);
  });

  it("closes rather than hanging over a word that is already finished", () => {
    const host = mount();
    type(host, "SELECT countr");
    expect(host.querySelector(".sql-menu")).not.toBeNull();
    type(host, "SELECT country");
    expect(host.querySelector(".sql-menu")).toBeNull();
  });

  it("goes away on Escape, and on leaving the box", () => {
    const host = mount();
    type(host, "SELECT coun");
    press(host, "Escape");
    expect(host.querySelector(".sql-menu")).toBeNull();

    type(host, "SELECT countr");
    expect(host.querySelector(".sql-menu")).not.toBeNull();
    // `focusout`, not `blur`: React maps `onBlur` onto the bubbling one, and a
    // dispatched `blur` would pass this test without the handler existing.
    act(() => box(host).dispatchEvent(new FocusEvent("focusout", { bubbles: true })));
    expect(host.querySelector(".sql-menu")).toBeNull();
  });

  it("opens on ⌃space without another letter being typed", () => {
    const host = mount("SELECT * FROM ", () => {});
    expect(host.querySelector(".sql-menu")).toBeNull();
    act(() => box(host).setSelectionRange(14, 14));
    press(host, " ", { ctrlKey: true });
    // Only the tables, because that is what goes after FROM.
    expect(rows(host)).toEqual(["events", "users"]);
  });
});

describe("running it", () => {
  it("runs on ⌘↵ and on ⌃↵", () => {
    let ran = 0;
    const host = mount("SELECT 1", () => {
      ran += 1;
    });
    press(host, "Enter", { metaKey: true });
    press(host, "Enter", { ctrlKey: true });
    expect(ran).toBe(2);
  });

  /* Enter means "take this one" while a list is open, and ⌘↵ has to keep
   * meaning run either way — a chord that works only sometimes is worse than
   * one that does not exist. */
  it("still runs on ⌘↵ with the completion list open", () => {
    let ran = 0;
    const host = mount("", () => {
      ran += 1;
    });
    type(host, "SELECT coun");
    expect(host.querySelector(".sql-menu")).not.toBeNull();
    press(host, "Enter", { metaKey: true });
    expect(ran).toBe(1);
    expect(box(host).value).toBe("SELECT coun");
    expect(host.querySelector(".sql-menu")).toBeNull();
  });

  it("does not run on a bare Enter, because this is SQL", () => {
    let ran = 0;
    const host = mount("SELECT 1", () => {
      ran += 1;
    });
    press(host, "Enter");
    expect(ran).toBe(0);
  });
});
