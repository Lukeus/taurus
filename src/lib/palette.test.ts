import { describe, expect, it } from "vitest";

import { marked, rank, score, type Action } from "./palette";

const action = (label: string, patch: Partial<Action> = {}): Action => ({
  id: label,
  label,
  group: "Do",
  run: () => {},
  ...patch,
});

const best = (labels: string[], query: string) =>
  rank(labels.map((l) => action(l)), query, (a) => ({
    label: a.label,
    hidden: a.keywords,
  })).map((s) => s.item.label);

describe("scoring one candidate", () => {
  it("matches characters as a subsequence rather than a substring", () => {
    // The whole reason this is not `includes`: `nc` should find "New
    // conversation", which does not contain those two letters together.
    expect("new conversation".includes("nc")).toBe(false);
    expect(score("New conversation", "nc")).not.toBe(null);
  });

  it("requires the characters in the order they were typed", () => {
    expect(score("abc", "ac")).not.toBe(null);
    expect(score("abc", "ca")).toBe(null);
  });

  it("finds nothing for a character that is not there", () => {
    expect(score("Settings", "z")).toBe(null);
  });

  it("matches everything for an empty query", () => {
    expect(score("anything", "")).toEqual({ score: 0, spans: [] });
  });

  it("reports where it matched", () => {
    expect(score("Open changes", "oc")?.spans).toEqual([0, 5]);
  });

  it("ignores whitespace in the query", () => {
    // "new conv" is two words to look for, and the space between them is
    // neither of them.
    const spaced = score("New conversation", "new conv");
    const tight = score("New conversation", "newconv");
    expect(spaced?.spans).toEqual(tight?.spans);
  });
});

describe("ranking a list", () => {
  it("puts a word-start match above a mid-word one", () => {
    expect(best(["Toggle the git pane", "Git status"], "git")[0]).toBe("Git status");
  });

  it("puts adjacent characters above scattered ones", () => {
    expect(best(["Copy the last message", "Commit"], "com")[0]).toBe("Commit");
  });

  it("prefers the shorter of two labels that matched the same way", () => {
    expect(best(["Settings", "Settings for this workspace only"], "settings")[0]).toBe(
      "Settings",
    );
  });

  it("keeps the given order when nothing is typed", () => {
    // A palette opened on nothing is a menu, and a menu that reorders itself
    // is a menu nobody learns.
    expect(best(["Third", "First", "Second"], "")).toEqual(["Third", "First", "Second"]);
  });

  it("keeps the given order between two equally good matches", () => {
    const items = [action("Alpha"), action("Alpha")];
    items[1].id = "second";
    const ordered = rank(items, "alpha", (a) => ({ label: a.label }));
    expect(ordered.map((s) => s.item.id)).toEqual(["Alpha", "second"]);
  });

  it("matches a synonym that is not on the row, below anything that is", () => {
    const items = [
      action("Changes", { keywords: "undo rewind revert" }),
      action("Undo the last turn"),
    ];
    const ordered = rank(items, "undo", (a) => ({ label: a.label, hidden: a.keywords }));
    expect(ordered[0].item.label).toBe("Undo the last turn");
    expect(ordered[1].item.label).toBe("Changes");
    // Nothing to highlight: the characters that matched are not on the row,
    // and marking some of the label instead would be marking the wrong thing.
    expect(ordered[1].spans).toEqual([]);
  });

  it("honours a limit", () => {
    expect(best(["aa", "ab", "ac"], "a").length).toBe(3);
    const limited = rank(
      ["aa", "ab", "ac"].map((l) => action(l)),
      "a",
      (a) => ({ label: a.label }),
      2,
    );
    expect(limited.length).toBe(2);
  });
});

describe("marking what matched", () => {
  it("joins adjacent matches into one run", () => {
    // Three adjacent characters are one mark; an element each would put a
    // seam through the middle of a word.
    expect(marked("Commit", [0, 1, 2])).toEqual([
      { text: "Com", on: true },
      { text: "mit", on: false },
    ]);
  });

  it("leaves a label with no matches in one piece", () => {
    expect(marked("Changes", [])).toEqual([{ text: "Changes", on: false }]);
  });

  it("marks a run in the middle", () => {
    expect(marked("abcde", [2])).toEqual([
      { text: "ab", on: false },
      { text: "c", on: true },
      { text: "de", on: false },
    ]);
  });
});
