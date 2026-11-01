import { describe, expect, it } from "vitest";

import { enterEdit, indentOf, outdentEdit, tabEdit, type Edit } from "./indent";

const apply = (text: string, edit: Edit) =>
  text.slice(0, edit.from) + edit.insert + text.slice(edit.to);

describe("indentOf", () => {
  it("reads tabs", () => expect(indentOf("a\n\tb\n", "ts")).toBe("\t"));
  it("reads the smallest step of spaces", () =>
    expect(indentOf("a\n    b\n  c\n      d\n", "ts")).toBe("  "));
  it("takes one space as alignment, not a step", () =>
    expect(indentOf(" * a\n    b\n", "ts")).toBe("    "));
  it("falls back by language", () => {
    expect(indentOf("a\n", "go")).toBe("\t");
    expect(indentOf("a\n", "makefile")).toBe("\t");
    expect(indentOf("a\n", "rust")).toBe("    ");
  });
});

describe("tabEdit", () => {
  it("puts a level at the caret", () => {
    const edit = tabEdit("ab", 1, 1, "  ");
    expect(apply("ab", edit)).toBe("a  b");
    expect([edit.start, edit.end]).toEqual([3, 3]);
  });

  it("indents every selected line and keeps them selected", () => {
    const text = "one\ntwo\nthree";
    const edit = tabEdit(text, 1, 6, "  ");
    expect(apply(text, edit)).toBe("  one\n  two\nthree");
    expect([edit.start, edit.end]).toEqual([3, 10]);
  });

  it("leaves a line alone when the selection stops at its start", () => {
    const text = "one\ntwo\n";
    expect(apply(text, tabEdit(text, 0, 4, "  "))).toBe("  one\ntwo\n");
  });

  it("indents a word's line rather than replacing the word", () => {
    expect(apply("one two", tabEdit("one two", 4, 7, "  "))).toBe("  one two");
  });

  it("gives blank lines no trailing spaces", () => {
    const text = "a\n\nb";
    expect(apply(text, tabEdit(text, 0, 4, "  "))).toBe("  a\n\n  b");
  });
});

describe("outdentEdit", () => {
  it("takes a level off the caret's line", () => {
    const edit = outdentEdit("    ab", 6, 6, "  ");
    expect(edit.insert).toBe("  ab");
    expect(edit.start).toBe(4);
  });

  it("takes what's there from a line with less than a level", () => {
    expect(outdentEdit(" ab", 3, 3, "    ").insert).toBe("ab");
  });

  it("takes a tab from a file of spaces", () => {
    expect(outdentEdit("\tab", 3, 3, "  ").insert).toBe("ab");
  });

  it("does nothing to a line with no indent", () => {
    expect(outdentEdit("ab", 1, 1, "  ").insert).toBe("ab");
  });

  it("keeps the caret on its line when it sat inside the indent", () => {
    const text = "a\n    b";
    expect(outdentEdit(text, 3, 3, "    ").start).toBe(2);
  });

  it("undoes what Tab did to a selection", () => {
    const text = "one\ntwo";
    const once = apply(text, tabEdit(text, 0, 7, "  "));
    expect(apply(once, outdentEdit(once, 0, once.length, "  "))).toBe(text);
  });
});

describe("enterEdit", () => {
  const enter = (text: string, grammar: string | null = "rust", unit = "  ") => {
    const at = text.indexOf("|");
    const plain = text.replace("|", "");
    const edit = enterEdit(plain, at, at, unit, grammar);
    const out = apply(plain, edit);
    return `${out.slice(0, edit.start)}|${out.slice(edit.start)}`;
  };

  it("keeps the line's indent", () => expect(enter("    a|")).toBe("    a\n    |"));
  it("goes a level deeper after an opener", () => expect(enter("fn a() {|")).toBe("fn a() {\n  |"));
  it("gives the closer its own line", () =>
    expect(enter("  fn a() {|}")).toBe("  fn a() {\n    |\n  }"));
  it("reads a colon as an opener only in Python", () => {
    expect(enter("def a():|", "python")).toBe("def a():\n  |");
    expect(enter("a:|", "yaml")).toBe("a:\n|");
  });
  it("opens nothing in prose", () => {
    expect(enter("see (|", "markdown")).toBe("see (\n|");
    expect(enter("see (|", null)).toBe("see (\n|");
  });
  it("replaces a selection", () => {
    const edit = enterEdit("  abc", 3, 4, "  ", "rust");
    expect(apply("  abc", edit)).toBe("  a\n  c");
  });
});
