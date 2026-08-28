import { describe, expect, it } from "vitest";

import { grammarFor, mark, paint, type Ink } from "./ink";

/** The text back out of the runs, which is what every property here is about. */
const text = (runs: Ink[]) => runs.map((run) => run.text).join("");
const kinds = (runs: Ink[], kind: string) =>
  runs.filter((run) => run.kind === kind).map((run) => run.text);

/** A line of each language, chosen for the constructs that are easy to get
 *  wrong rather than for being representative. */
const CORPUS: [string, string][] = [
  ["rust", `let s: &'a str = "a // b"; /* c */ foo(1_000, 0xff); // done`],
  ["ts", "const x = `a ${b} c`; // t\nexport function f(): Promise<void> {}"],
  ["python", `def f(x: int) -> str:\n    """doc ' with quote"""\n    return f'{x}'  # n`],
  ["go", "func main() {\n\tfmt.Println(`raw\nstring`) // hi\n}"],
  ["shell", `if [ -n "$x" ]; then echo 'it'"'"'s'; fi # done`],
  ["json", `{"a": [1, 2.5, true, null], "b": "\\"q\\""}`],
  ["yaml", "key: value # note\nlist:\n  - 'one'\n  - \"two\""],
  ["toml", 'name = "taurus"\n[table]\nx = 1_000 # c'],
];

describe("painting a language", () => {
  it("gives back every character exactly once", () => {
    // The property the whole scanner rests on. These runs are concatenated
    // back into a line of code on screen, so dropping or doubling a character
    // would put something on the page that the file does not say. Held
    // directly rather than by checking any particular colour.
    for (const [grammar, source] of CORPUS) {
      expect(text(paint(source, grammar)), grammar).toBe(source);
    }
    // Including for input the grammar has no idea what to do with.
    for (const [grammar] of CORPUS) {
      const junk = "\u0000\t§¶∆ 🙂 <<>> ]]]";
      expect(text(paint(junk, grammar)), grammar).toBe(junk);
    }
  });

  it("leaves a language it does not know as one plain run", () => {
    // Not a failure — the honest answer. It renders as exactly the `<code>`
    // that was there before any of this, which is why nothing downstream has
    // to branch on whether colouring worked.
    const source = "SELECT ain't\nnot really anything";
    expect(paint(source, null)).toEqual([{ text: source, kind: "plain" }]);
    expect(paint(source, "klingon")).toEqual([{ text: source, kind: "plain" }]);
    expect(paint("", null)).toEqual([]);
  });

  it("does not read a lifetime as a string", () => {
    // The failure that made `spans` a property of a quote rather than a
    // constant: `'a` opens nothing, and treating it as an opening turns the
    // rest of the file into one long literal.
    const runs = paint(`fn f<'a>(s: &'a str) -> &'a str { s }`, "rust");
    expect(kinds(runs, "string")).toEqual([]);
    expect(kinds(runs, "keyword")).toContain("fn");
    expect(kinds(runs, "keyword")).toContain("str");
  });

  it("lets a template literal cross a line and an ordinary quote not", () => {
    const template = paint("const a = `one\ntwo`;\nconst b = 2;", "ts");
    expect(kinds(template, "string")).toEqual(["`one\ntwo`"]);
    // The unbalanced quote is punctuation, so `const` on the next line is
    // still a keyword rather than the inside of a string.
    const broken = paint(`const a = "one\nconst b = 2;`, "ts");
    expect(kinds(broken, "keyword")).toEqual(["const", "const"]);
  });

  it("does not tint a keyword inside a string or a comment", () => {
    const runs = paint(`// return early\nlet x = "return";`, "rust");
    expect(kinds(runs, "keyword")).toEqual(["let"]);
  });

  it("does not tint a comment opener inside a string", () => {
    const runs = paint(`let url = "https://example.com"; // real`, "rust");
    expect(kinds(runs, "string")).toEqual(['"https://example.com"']);
    expect(kinds(runs, "comment")).toEqual(["// real"]);
  });

  it("reads a doubled quote as an escape where the language says so", () => {
    // A shell single-quote takes no backslash escape at all, so `'\''` is two
    // literals with a backslash between them rather than one literal — which
    // is what actually happens when the shell reads it.
    const runs = paint(`echo 'it''s'`, "shell");
    expect(kinds(runs, "string")).toEqual(["'it''s'"]);
  });

  it("reads a triple quote before a single one", () => {
    const runs = paint(`x = """a ' b"""`, "python");
    expect(kinds(runs, "string")).toEqual([`"""a ' b"""`]);
  });

  it("stops a number before a method call", () => {
    // `[0-9._]` would eat the dot and the start of the name after it, which
    // is the whole reason a number is scanned rather than matched.
    const runs = paint("let n = 1.max(2);", "rust");
    expect(kinds(runs, "number")).toEqual(["1", "2"]);
    expect(kinds(runs, "fn")).toEqual(["max"]);
  });

  it("reads a radix prefix and an exponent as one number", () => {
    const runs = paint("let a = 0xdead_beef; let b = 1.5e-9;", "rust");
    expect(kinds(runs, "number")).toEqual(["0xdead_beef", "1.5e-9"]);
  });

  it("calls a word a call only when it is being called", () => {
    const runs = paint("let count = tally();", "rust");
    expect(kinds(runs, "fn")).toEqual(["tally"]);
    // Joined, because neighbouring runs of one kind are merged — `count` and
    // the spaces around it are one plain run, which is the point of merging.
    expect(kinds(runs, "plain").join("")).toContain("count");
  });

  it("merges neighbouring runs of the same kind", () => {
    // Not cosmetic: without it a line of code is one span per character of
    // indentation, and the transcript draws a great many lines of code.
    const runs = paint("        return;", "rust");
    expect(runs.map((run) => run.kind)).toEqual(["plain", "keyword", "punct"]);
  });
});

describe("what a hint means", () => {
  it("reads a fence label, an extension, and a whole path", () => {
    expect(grammarFor("rust")).toBe("rust");
    expect(grammarFor("TSX")).toBe("ts");
    expect(grammarFor("src/lib/ink.ts")).toBe("ts");
    expect(grammarFor("crates/taurus-core/src/agent.rs")).toBe("rust");
    expect(grammarFor("C:\\work\\main.py")).toBe("python");
  });

  it("names no language for a file that names none", () => {
    // A dotfile is not an extension, and neither is a directory that has a dot
    // in it. Both would otherwise be looked up and, occasionally, found.
    expect(grammarFor(".gitignore")).toBe(null);
    expect(grammarFor("Makefile")).toBe(null);
    expect(grammarFor("a.b/Dockerfile")).toBe(null);
    expect(grammarFor("notes.txt")).toBe(null);
    expect(grammarFor(null)).toBe(null);
    expect(grammarFor("")).toBe(null);
  });
});

describe("marking a span inside painted runs", () => {
  it("splits a run at the edges of the span", () => {
    const runs = paint("let value = 1;", "rust");
    // "value" alone: offsets 4 to 9.
    const marked = mark(runs, { from: 4, to: 9 });
    expect(text(marked)).toBe("let value = 1;");
    expect(marked.filter((run) => run.changed).map((run) => run.text)).toEqual(["value"]);
    // And the syntax underneath is unchanged by having been cut.
    expect(marked.find((run) => run.text === "let")?.kind).toBe("keyword");
  });

  it("marks a span that crosses more than one run", () => {
    const marked = mark(paint("let a = 1;", "rust"), { from: 0, to: 5 });
    expect(marked.filter((run) => run.changed).map((run) => run.text).join("")).toBe("let a");
    expect(text(marked)).toBe("let a = 1;");
  });

  it("marks nothing for no span and for an empty one", () => {
    const runs = paint("let a = 1;", "rust");
    for (const span of [null, { from: 3, to: 3 }, { from: 5, to: 2 }]) {
      const marked = mark(runs, span);
      expect(text(marked)).toBe("let a = 1;");
      expect(marked.some((run) => run.changed)).toBe(false);
    }
  });
});

describe("markdown", () => {
  /** The property every painter here holds. A dropped character would slide the
   *  colour off the text under a textarea and keep sliding. */
  const exact = (source: string) =>
    expect(
      paint(source, "markdown")
        .map((run) => run.text)
        .join(""),
    ).toBe(source);

  it("gives back exactly what it was handed", () => {
    exact("# Title\n\nSome **bold** and `code`.\n\n- one\n- two\n");
    exact("```rust\nfn main() {}\n```\n");
    exact("> quoted\n\n---\n\n1. first\n");
    exact("");
    exact("\n\n\n");
    exact("no trailing newline");
  });

  it("finds a language from a filename", () => {
    expect(grammarFor("README.md")).toBe("markdown");
    expect(grammarFor("docs/known-gaps.md")).toBe("markdown");
    expect(grammarFor("md")).toBe("markdown");
  });

  it("tints a heading whole, marker and all", () => {
    const runs = paint("## Why this exists\n", "markdown");
    expect(runs[0]).toEqual({ text: "## Why this exists", kind: "keyword" });
  });

  /** A fence flips what the lines after it mean, and flips back. */
  it("keeps a fenced block one colour and stops at the closer", () => {
    const runs = paint("```\n# inside\n```\n# outside\n", "markdown");
    expect(runs.find((r) => r.text.includes("inside"))?.kind).toBe("string");
    expect(runs.find((r) => r.text.includes("outside"))).toEqual({
      text: "# outside",
      kind: "keyword",
    });
  });

  /** The reason the alternatives are one expression and code is listed first. */
  it("lets a code span win over the emphasis inside it", () => {
    const runs = paint("a `**not bold**` b", "markdown");
    expect(runs.find((r) => r.text.startsWith("`"))).toEqual({
      text: "`**not bold**`",
      kind: "string",
    });
  });

  it("separates a list marker from what follows it", () => {
    const runs = paint("- [ ] do the thing\n", "markdown");
    expect(runs[0].kind).toBe("punct");
    expect(runs[0].text).toBe("- [ ] ");
  });

  it("paints a link as one run", () => {
    const runs = paint("see [the docs](docs/x.md) for more", "markdown");
    expect(runs.find((r) => r.text.startsWith("["))).toEqual({
      text: "[the docs](docs/x.md)",
      kind: "fn",
    });
  });
});
