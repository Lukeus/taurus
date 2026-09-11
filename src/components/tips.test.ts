// A tip that explains something is read through the same layer as every
// other: it opens on hover and on focus. A `<span>` never takes focus, so an
// explanation written on one could be read with a mouse and not a keyboard —
// what a rewind does to HEAD, why a call counted as failed.
//
// Walked from the source rather than a rendered tree, because the tips that
// matter are the rare ones — a warning tag, a failure count — that no single
// mount test would think to draw.
import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const SRC = new URL("../", import.meta.url);

/** Spans that carry a written tip without taking focus, on purpose. */
const EXEMPT: Record<string, string> = {
  "grid-cell":
    "one per row of a data grid; a tab stop per empty cell would bury the grid's own controls",
  "sql-shared":
    "a row of the completion menu, which is driven from the query box and never takes focus",
};

function sources(dir: URL): URL[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (entry.isDirectory()) return sources(new URL(`${entry.name}/`, dir));
    return entry.name.endsWith(".tsx") && !entry.name.includes(".test.")
      ? [new URL(entry.name, dir)]
      : [];
  });
}

describe("an explanation in a tooltip", () => {
  it("can be reached from the keyboard", () => {
    const unreachable: string[] = [];
    for (const file of sources(SRC)) {
      const text = readFileSync(file, "utf8");
      for (const match of text.matchAll(/<span\b([^>]*?)>/gs)) {
        const tag = match[1];
        // A tip written out is an explanation; one that is an expression is
        // almost always the value itself, echoed for a cell too narrow for it.
        if (!/data-tip="/.test(tag) || /tabIndex=/.test(tag)) continue;
        if (Object.keys(EXEMPT).some((name) => tag.includes(name))) continue;
        const line = text.slice(0, match.index).split("\n").length;
        unreachable.push(`${file.pathname.slice(SRC.pathname.length)}:${line}`);
      }
    }
    expect(unreachable, "give each of these tabIndex={0}").toEqual([]);
  });
});
