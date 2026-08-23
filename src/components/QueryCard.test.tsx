// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { QueryCard } from "./QueryCard";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

const SQL = "SELECT category, count(*)\n  FROM events\n GROUP BY category";

function mount(onRun?: (sql: string) => void) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => {
    root.render(<QueryCard view={{ type: "query", sql: SQL }} onRun={onRun} />);
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return host;
}

describe("the query card", () => {
  it("shows the query whole, not cut to a line", () => {
    // The row above it already carries the one-line version. This exists to be
    // read closely, so a `GROUP BY` three lines down has to survive.
    const host = mount();
    expect(host.querySelector(".query-card-sql")?.textContent).toBe(SQL);
  });

  it("carries no rows at all", () => {
    // Deliberate, and the same argument the dataset card makes: the answer was
    // true of the files as they stood when the call ran, and the file
    // underneath is rewritten by the next turn as often as not. Asking again is
    // cheap; a stale answer redrawn on reopen is not.
    const host = mount();
    expect(host.querySelector(".grid-box")).toBeNull();
    expect(host.querySelector(".table-box")).toBeNull();
  });

  it("hands the query back exactly as it was sent", () => {
    const taken: string[] = [];
    const host = mount((sql) => taken.push(sql));
    const run = [...host.querySelectorAll("button")].find(
      (b) => b.textContent === "Run in Query",
    );
    act(() => run?.click());
    expect(taken).toEqual([SQL]);
  });

  it("offers nothing to run where there is no pane to run it in", () => {
    // A delegate's transcript is read on its own, so the button would point at
    // a surface that is not on screen.
    const host = mount();
    expect(
      [...host.querySelectorAll("button")].map((b) => b.textContent),
    ).not.toContain("Run in Query");
    // The query is still worth having in hand, so copy stays.
    expect(host.querySelector(".query-card-sql")).not.toBeNull();
  });
});
