import { describe, expect, it } from "vitest";

import { cutAtHits, findAll, findKey, hitFrom, lineAsked, MOST_HITS } from "./find";
import type { Ink } from "./ink";

describe("findAll", () => {
  it("ignores case until the query has a capital in it", () => {
    expect(findAll("Retry retry", "retry")).toEqual([
      { from: 0, to: 5 },
      { from: 6, to: 11 },
    ]);
    expect(findAll("Retry retry", "Retry")).toEqual([{ from: 0, to: 5 }]);
  });

  it("doesn't overlap", () => expect(findAll("aaaa", "aa")).toHaveLength(2));
  it("finds nothing for an empty query", () => expect(findAll("a", "")).toEqual([]));
  it("finds across lines", () => expect(findAll("a\nb", "a\nb")).toHaveLength(1));

  it("stops counting at the limit", () => {
    expect(findAll("a".repeat(MOST_HITS + 5), "a")).toHaveLength(MOST_HITS);
  });

  it("keeps offsets true in text that lowering would lengthen", () => {
    const text = "İ retry";
    expect(text.toLowerCase().length).not.toBe(text.length);
    const [hit] = findAll(text, "retry");
    expect(text.slice(hit.from, hit.to)).toBe("retry");
  });
});

describe("lineAsked", () => {
  it("reads a line", () => expect(lineAsked(":120")).toBe(120));
  it("treats line zero as the first", () => expect(lineAsked(":0")).toBe(1));
  it("is nothing else", () => {
    for (const query of [":", "120", ":a", ":12a"]) expect(lineAsked(query)).toBeNull();
  });
});

describe("hitFrom", () => {
  const hits = [
    { from: 5, to: 6 },
    { from: 10, to: 11 },
  ];
  it("takes the next one", () => expect(hitFrom(hits, 6)).toBe(1));
  it("takes one starting right there", () => expect(hitFrom(hits, 5)).toBe(0));
  it("wraps past the last", () => expect(hitFrom(hits, 11)).toBe(0));
  it("says when there are none", () => expect(hitFrom([], 0)).toBe(-1));
});

describe("cutAtHits", () => {
  const RUNS: Ink[] = [
    { text: "abc", kind: "keyword" },
    { text: "def", kind: "string" },
  ];

  it("gives back every character in order", () => {
    for (let from = 0; from < 6; from += 1) {
      for (let to = from + 1; to <= 6; to += 1) {
        const cut = cutAtHits(RUNS, 0, [{ from, to }], 0);
        expect(cut.map((run) => run.text).join("")).toBe("abcdef");
      }
    }
  });

  it("marks a match across two colours and keeps both", () => {
    const cut = cutAtHits(RUNS, 0, [{ from: 2, to: 4 }], 0);
    expect(cut).toEqual([
      { text: "ab", kind: "keyword" },
      { text: "c", kind: "keyword", hit: "current" },
      { text: "d", kind: "string", hit: "current" },
      { text: "ef", kind: "string" },
    ]);
  });

  it("tells the current match from the others", () => {
    const hits = [
      { from: 0, to: 1 },
      { from: 2, to: 3 },
    ];
    expect(cutAtHits(RUNS, 0, hits, 1).map((run) => run.hit).filter(Boolean)).toEqual([
      "other",
      "current",
    ]);
  });

  it("reads offsets from where the window starts", () => {
    const cut = cutAtHits(RUNS, 100, [{ from: 101, to: 102 }], 0);
    expect(cut.find((run) => run.hit)?.text).toBe("b");
  });

  it("marks what's left of a match that began above the window", () => {
    const cut = cutAtHits(RUNS, 100, [{ from: 90, to: 102 }], 0);
    expect(cut[0]).toEqual({ text: "ab", kind: "keyword", hit: "current" });
  });

  it("leaves runs alone when every match is above them", () => {
    expect(cutAtHits(RUNS, 100, [{ from: 0, to: 1 }], 0)).toBe(RUNS);
  });
});

describe("findKey", () => {
  const press = (init: Partial<KeyboardEvent>) => init as KeyboardEvent;
  it("reads F3 everywhere", () => {
    expect(findKey(press({ key: "F3" }))).toBe("next");
    expect(findKey(press({ key: "F3", shiftKey: true }))).toBe("previous");
  });
  it("reads Control and G as going to a line", () => {
    expect(findKey(press({ key: "g", ctrlKey: true }))).toBe("line");
  });
  it("leaves other keys alone", () => {
    expect(findKey(press({ key: "f" }))).toBeNull();
  });
});
