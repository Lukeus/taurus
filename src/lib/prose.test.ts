import { describe, expect, it } from "vitest";

import { embedFor } from "../state/notebook";
import { read } from "./mermaid";
import {
  BLOCKS,
  carry,
  embedLine,
  headingOffsets,
  linkLine,
  noteName,
  suggest,
  toggleTask,
  type Offer,
} from "./prose";

/** Text written with a `|` where the caret is. */
function at(marked: string): { text: string; caret: number } {
  return { text: marked.replace("|", ""), caret: marked.indexOf("|") };
}

function offered(
  marked: string,
  sketches: string[] = [],
  asked = false,
  notes: string[] = [],
): Offer | null {
  const { text, caret } = at(marked);
  return suggest(text, caret, { sketches, notes }, asked);
}

const labels = (marked: string, sketches: string[] = [], notes: string[] = []) =>
  offered(marked, sketches, false, notes)?.items.map((item) => item.label) ?? null;

/** Takes the choice called `label` the way the editor does, and gives the text
 *  back with the caret (or the start of the selection) marked. */
function take(marked: string, label: string, sketches: string[] = [], notes: string[] = []): string {
  const { text, caret } = at(marked);
  const offer = suggest(text, caret, { sketches, notes });
  const choice = offer?.items.find((item) => item.label === label);
  if (!offer || !choice) throw new Error(`nothing called "${label}" is offered at ${marked}`);
  const out = text.slice(0, offer.from) + choice.insert + text.slice(offer.to);
  const cursor = offer.from + (choice.select?.[0] ?? choice.caret ?? choice.insert.length);
  return `${out.slice(0, cursor)}|${out.slice(cursor)}`;
}

/** Presses Enter the way the editor does, and gives the text back marked. */
function enter(marked: string): string | null {
  const { text, caret } = at(marked);
  const edit = carry(text, caret);
  if (!edit) return null;
  const out = text.slice(0, edit.from) + edit.insert + text.slice(edit.to);
  return `${out.slice(0, edit.caret)}|${out.slice(edit.caret)}`;
}

describe("the block list", () => {
  it("opens on a slash at the start of a line, in the order a note uses them", () => {
    expect(labels("/|")?.slice(0, 4)).toEqual(["Heading 1", "Heading 2", "Heading 3", "Bulleted list"]);
    expect(labels("/|")).toContain("Divider");
    // Indented is still the start of a line — a block inside a list item.
    expect(labels("  /|")).toContain("Task");
  });

  it("does not open on a slash in the middle of a sentence", () => {
    // `and/or`, a path, a date: all ordinary prose.
    expect(offered("either and/|")).toBeNull();
    expect(offered("see /|")).toBeNull();
  });

  it("narrows on any word of the name, or a word it answers to", () => {
    expect(labels("/todo|")).toEqual(["Task"]);
    expect(labels("/h2|")).toEqual(["Heading 2"]);
    expect(labels("/list|")).toEqual(["Bulleted list", "Numbered list"]);
    expect(labels("/mermaid|")).toEqual(["Flowchart", "Sequence diagram"]);
  });

  it("closes once nothing matches, so a path typed at the start of a line is just a path", () => {
    expect(offered("/usr|")).toBeNull();
  });

  it("writes the syntax in place of the slash", () => {
    expect(take("/h1|", "Heading 1")).toBe("# |");
    expect(take("Intro\n/ta|", "Task")).toBe("Intro\n- [ ] |");
    expect(take("  /|", "Quote")).toBe("  > |");
  });

  it("leaves a table's first header selected, to be typed over", () => {
    const { text, caret } = at("/|");
    const offer = suggest(text, caret, { sketches: [], notes: [] })!;
    const table = offer.items.find((item) => item.label === "Table")!;
    const [from, to] = table.select!;
    expect(table.insert.slice(from, to)).toBe("Column");
    expect(table.insert.split("\n")).toHaveLength(3);
  });

  it("gives a divider under a line of text the blank line it needs", () => {
    // Straight under a paragraph, `---` would turn the paragraph into a heading.
    expect(take("A paragraph.\n/|", "Divider")).toBe("A paragraph.\n\n---\n|");
    expect(take("/|", "Divider")).toBe("---\n|");
    expect(take("A paragraph.\n\n/|", "Divider")).toBe("A paragraph.\n\n---\n|");
  });

  it("lists the notebook's sketches after the blocks, as embed lines", () => {
    const got = labels("/|", ["Auth flow"]);
    expect(got?.[got.length - 1]).toBe("Auth flow");
    expect(take("/auth|", "Auth flow", ["Auth flow"])).toBe("![Auth flow](<Auth flow.excalidraw>)|");
  });

  it("opens with ⌃Space on an empty line, and not on a line with words on it", () => {
    const offer = offered("Intro\n|", [], true);
    expect(offer?.items[0].label).toBe("Heading 1");
    expect(offer && [offer.from, offer.to]).toEqual([6, 6]);
    expect(offered("some words |", [], true)).toBeNull();
  });
});

describe("the diagram templates", () => {
  /*
   * A template that the note's own reader refused would be the menu writing
   * something broken into the note. So each is read by the same scanner that
   * draws a fence in Read, and must come back as a drawing.
   */
  it("are diagrams the reader draws", () => {
    const diagrams = BLOCKS.filter((block) => block.kind === "diagram");
    expect(diagrams).toHaveLength(2);
    for (const block of diagrams) {
      const body = block.insert.split("\n").slice(1, -1).join("\n");
      const drawn = read(body);
      expect(drawn.kind, block.label).not.toBe("refused");
      expect(drawn.kind !== "refused" && drawn.skipped, block.label).toEqual([]);
    }
  });

  it("select the first label, so the first thing typed replaces it", () => {
    expect(take("/flow|", "Flowchart")).toBe(
      "```mermaid\nflowchart LR\n  start[|Start] --> finish[Finish]\n```",
    );
  });
});

describe("a fence's language", () => {
  it("offers mermaid first, then what the app colours", () => {
    const got = labels("```|");
    expect(got?.[0]).toBe("mermaid");
    expect(got).toContain("rust");
    const notes = Object.fromEntries(offered("```|")!.items.map((item) => [item.label, item.note]));
    expect(notes.mermaid).toBe("draws as a diagram");
    expect(notes.ts).toBe("colored");
    // Past the rows shown with nothing typed, the way the query box caps its
    // list: one letter more and it is there.
    expect(notes.sql).toBeUndefined();
    expect(offered("```sq|")!.items.map((item) => [item.label, item.note])).toEqual([
      ["sql", "plain text"],
    ]);
  });

  it("writes the closing fence when the block is being opened", () => {
    expect(take("```|", "mermaid")).toBe("```mermaid\n|\n```");
    expect(take("~~~|", "rust")).toBe("~~~rust\n|\n~~~");
    // At the opener's own indentation.
    expect(take("  ```|", "ts")).toBe("  ```ts\n|\n  ```");
    // A complete block further down is not this one's closer.
    expect(take("```|\nprose\n```ts\nx\n```", "go")).toBe("```go\n|\n```\nprose\n```ts\nx\n```");
  });

  it("only relabels a fence that already has its closer", () => {
    expect(take("```|\ncode\n```", "ts")).toBe("```ts|\ncode\n```");
    // And offers no template, which would give the block a second end.
    expect(labels("```|\ncode\n```")).not.toContain("mermaid · flowchart");
  });

  it("offers the templates on a fence being opened", () => {
    expect(take("```|", "mermaid · sequence diagram")).toBe(
      "```mermaid\nsequenceDiagram\n  Client->>Server: |Request\n  Server-->>Client: Response\n```",
    );
  });

  it("chains from the block list: a code block's fence asks for its language", () => {
    const made = take("/code|", "Code block");
    expect(made).toBe("```|\n\n```");
    expect(labels(made)?.[0]).toBe("mermaid");
    // Its closer is already there, so the language is only a label.
    expect(take(made, "rust")).toBe("```rust|\n\n```");
  });

  it("gets out of the way once the label is finished, so Enter is a newline", () => {
    expect(offered("```rust|\nlet x = 1;\n```")).toBeNull();
  });

  it("does not open on a closing fence", () => {
    expect(offered("```js\nx\n```|")).toBeNull();
  });
});

describe("a sketch's embed line", () => {
  const SKETCHES = ["Auth flow", "Token map"];

  it("offers the notebook's sketches inside an image's brackets", () => {
    expect(labels("![|", SKETCHES)).toEqual(SKETCHES);
    expect(labels("See ![tok|", SKETCHES)).toEqual(["Token map"]);
    expect(take("See ![|", "Auth flow", SKETCHES)).toBe("See ![Auth flow](<Auth flow.excalidraw>)|");
  });

  it("keeps a caption already written, and completes the address", () => {
    expect(take("![How it goes](|", "Auth flow", SKETCHES)).toBe(
      "![How it goes](<Auth flow.excalidraw>)|",
    );
    // A closing bracket already there is used rather than doubled.
    expect(take("![x](<Au|)", "Auth flow", SKETCHES)).toBe("![x](<Auth flow.excalidraw>)|");
  });

  it("offers nothing in a notebook with no sketches", () => {
    expect(offered("![|", [])).toBeNull();
  });

  it("writes the same line Copy embed copies", () => {
    for (const name of ["flow", "Auth flow", "a.b"]) expect(embedLine(name)).toBe(embedFor(name));
  });
});

describe("a link to another note", () => {
  const NOTES = ["Token store", "Release checklist"];

  it("offers the notebook's other notes inside a link's address", () => {
    expect(labels("See [the store](|", [], NOTES)).toEqual(NOTES);
    expect(take("See [the store](|", "Token store", [], NOTES)).toBe(
      "See [the store](<Token store.md>)|",
    );
    // A closing bracket already there is used rather than doubled.
    expect(take("[x](<Rel|)", "Release checklist", [], NOTES)).toBe("[x](<Release checklist.md>)|");
  });

  it("does not open on a bracket alone, which is typed for plenty that is not a link", () => {
    expect(offered("- [|", [], false, NOTES)).toBeNull();
    expect(offered("see note [1|", [], false, NOTES)).toBeNull();
  });

  it("leaves an image's address to the sketches", () => {
    expect(labels("![x](|", ["Flow"], NOTES)).toEqual(["Flow"]);
  });

  it("lists the notes after the sketches in the block list, as link lines", () => {
    expect(labels("/|", ["Flow"], ["Token store"])?.slice(-2)).toEqual(["Flow", "Token store"]);
    expect(take("/tok|", "Token store", [], ["Token store"])).toBe("[Token store](<Token store.md>)|");
  });
});

describe("which links in a note are notes", () => {
  it("reads the name out of either spelling", () => {
    // `<Token store.md>` reaches the link percent-encoded.
    expect(noteName("Token%20store.md")).toBe("Token store");
    expect(noteName("./plan.md")).toBe("plan");
    expect(noteName("Plan.MD")).toBe("Plan");
    // A section is dropped, and the note opens at its top.
    expect(noteName("plan.md#rollout")).toBe("plan");
  });

  it("leaves every other link alone", () => {
    for (const href of [
      undefined,
      "",
      ".md",
      "https://example.com/plan.md",
      "mailto:someone@example.md",
      "notes.txt",
      "sub/plan.md",
      "../plan.md",
      "..%2Fplan.md",
      "%E0%A4%A.md",
    ]) {
      expect(noteName(href), String(href)).toBeNull();
    }
  });

  it("writes a link a note reads back as the same note", () => {
    expect(linkLine("Token store")).toBe("[Token store](<Token store.md>)");
    expect(noteName(encodeURI("Token store.md"))).toBe("Token store");
  });
});

describe("ticking a task", () => {
  const TASKS = "- [ ] one\n- [x] two\n";

  it("flips the box of the item that starts where it is told", () => {
    expect(toggleTask(TASKS, 0)).toBe("- [x] one\n- [x] two\n");
    expect(toggleTask(TASKS, 10)).toBe("- [ ] one\n- [ ] two\n");
    expect(toggleTask("3. [X] done", 0)).toBe("3. [ ] done");
  });

  it("refuses a position that is not a task, rather than ticking whatever is near", () => {
    expect(toggleTask(TASKS, 2)).toBeNull();
    expect(toggleTask("- a plain item", 0)).toBeNull();
  });
});

describe("the headings a note is lined up by", () => {
  it("finds each one, and none inside a code block", () => {
    const text = "# One\ntext\n## Two\n```\n# not a heading\n```\n### Three";
    expect(headingOffsets(text)).toEqual([0, 11, text.indexOf("### Three")]);
    expect(headingOffsets("#tag is not a heading")).toEqual([]);
  });
});

describe("inside a code block", () => {
  it("opens nothing, because what is typed there is the sample's own text", () => {
    expect(offered("```\n/|\n```")).toBeNull();
    expect(offered("```sh\ncd /|\n```")).toBeNull();
    expect(offered("```\n![|\n```", ["Auth flow"])).toBeNull();
    // And after the block is closed, prose again.
    expect(labels("```\ncode\n```\n/h1|")).toEqual(["Heading 1"]);
  });
});

describe("Enter on a list", () => {
  it("starts the next item with the same marker", () => {
    expect(enter("- one|")).toBe("- one\n- |");
    expect(enter("* one|")).toBe("* one\n* |");
    expect(enter("  + deep|")).toBe("  + deep\n  + |");
  });

  it("counts on, in the list's own style", () => {
    expect(enter("3. three|")).toBe("3. three\n4. |");
    expect(enter("9) nine|")).toBe("9) nine\n10) |");
  });

  it("starts an unticked task after a ticked one", () => {
    expect(enter("- [x] done|")).toBe("- [x] done\n- [ ] |");
    expect(enter("- [ ] todo|")).toBe("- [ ] todo\n- [ ] |");
  });

  it("carries a quote at its depth", () => {
    expect(enter("> said|")).toBe("> said\n> |");
    expect(enter(">> deeper|")).toBe(">> deeper\n>> |");
  });

  it("ends the list on an empty item, taking the marker away", () => {
    expect(enter("- one\n- |")).toBe("- one\n|");
    expect(enter("1. a\n2. |")).toBe("1. a\n|");
    expect(enter("- [ ] |")).toBe("|");
    expect(enter("> |")).toBe("|");
  });

  it("splits an item at the caret, like any other line", () => {
    expect(enter("- one| two")).toBe("- one\n- | two");
  });

  it("is a plain newline anywhere else", () => {
    expect(enter("A sentence.|")).toBeNull();
    expect(enter("1.5 is a number|")).toBeNull();
    // A rule starts like an item and is not one.
    expect(enter("* * *|")).toBeNull();
    // The caret in the marker is somebody editing the marker.
    expect(enter("-| one")).toBeNull();
    // And inside a code block, a `- ` is code.
    expect(enter("```\n- a|\n```")).toBeNull();
  });
});
