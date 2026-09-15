/**
 * Enough Markdown to finish a line, to carry a list on to the next one, and to
 * find the handful of things in a note the pane acts on.
 *
 * The note editor's half of what `sql.ts` is to the query box: pure, no DOM,
 * and not a parser. Everything here answers a question about the text around
 * a position — what could go at the caret, what Enter should put on the next
 * line, which note a link names, where a task's box is — and each stays useful
 * when the answer is approximate.
 *
 * # What the menu offers, and where
 *
 * Four places, each a thing somebody writing a note would otherwise have to
 * remember the syntax for:
 *
 * - **`/` at the start of a line**: the blocks — headings, lists, a task, a
 *   quote, a code block, a table, a divider, two diagram templates — then every
 *   sketch and every other note in the note's notebook. The composer opens its
 *   command list on the same key, so it is the one people already reach for.
 * - **After a fence's backticks**: the languages the app colours, and
 *   `mermaid`, which it draws. When the fence is being opened rather than
 *   relabelled, taking one writes the closing fence too.
 * - **Inside `![`**: the notebook's sketches, written as the embed line. This
 *   is **Copy embed** without leaving the note.
 * - **Inside a link's address, `[text](`**: the notebook's other notes.
 *
 * Nothing opens inside a fenced block. A `/usr/bin` or a `![` in a code sample
 * is the sample's text, and a menu over it is in the way.
 */

import { grammarFor } from "./ink";

/** One thing the menu can put where the caret is. */
export type Choice = {
  /** What the row reads as. */
  label: string;
  /** The dimmer half of the row: the syntax it writes, or what it is. */
  note: string;
  kind: "block" | "diagram" | "language" | "sketch" | "note";
  /** What replaces the trigger that was typed. */
  insert: string;
  /** Where the caret lands, as an offset into `insert`. The end when absent. */
  caret?: number;
  /** A stretch of `insert` left selected, as offsets into it: a placeholder
   *  to type straight over. Wins over `caret`. */
  select?: [number, number];
  /** Other words the row answers to, so `/todo` finds a task. */
  also?: string[];
};

/** What the menu offers, and the stretch of text a choice replaces. */
export type Offer = { from: number; to: number; items: Choice[] };

/**
 * What else is in the note's notebook, by name: what the menu can embed and
 * what it can link to. The note being written is left out of `notes` by the
 * pane — a link to the page you are on is a link nobody wanted offered.
 */
export type Names = { sketches: readonly string[]; notes: readonly string[] };

/**
 * A change Enter makes instead of its plain newline — see `carry`.
 *
 * Offsets into the text before the change, except `caret`, which is where the
 * caret belongs afterwards.
 */
export type Edit = { from: number; to: number; insert: string; caret: number };

/**
 * How many rows the menu shows. Every block fits with a sketch or two below
 * it, and past that the answer is a letter more, not a longer list.
 */
const MAX_CHOICES = 14;

/** A fence line, the same test the painter uses — see `inFence`. */
const FENCE = /^\s*(```|~~~)/;

/**
 * Whether the line starting at `lineStart` is inside a fenced block.
 *
 * Every fence line flips it, which is the painter's rule in `ink.ts` and on
 * purpose the same one: a menu that thought it was in prose where the editor
 * had painted code — or the other way round — would be two answers on one
 * screen.
 */
function inFence(text: string, lineStart: number): boolean {
  let open = false;
  for (const line of text.slice(0, lineStart).split("\n")) {
    if (FENCE.test(line)) open = !open;
  }
  return open;
}

/** Where the caret's line starts and ends. */
function lineAround(text: string, caret: number): { start: number; end: number } {
  const start = text.lastIndexOf("\n", caret - 1) + 1;
  const newline = text.indexOf("\n", caret);
  return { start, end: newline === -1 ? text.length : newline };
}

/**
 * The line that shows a sketch in a note.
 *
 * The same shape as the notebook's `embedFor`, which the test holds them to.
 * Written out here rather than imported, because this is a library module and
 * that one is the notebook's state.
 */
export function embedLine(name: string): string {
  return `![${name}](<${name}.excalidraw>)`;
}

/**
 * The line that links to another note in the same notebook.
 *
 * An ordinary relative link to the file, the way `embedLine` is an ordinary
 * image — so GitHub, or any viewer that knows the folder, follows it to the
 * same note this pane opens.
 */
export function linkLine(name: string): string {
  return `[${name}](<${name}.md>)`;
}

/**
 * The note a link's address names, or `null` for any other link.
 *
 * `sketchName`'s rules for the other kind of file: a `.md` file beside the
 * note, which Markdown hands over percent-encoded, with nothing in it that
 * would reach outside the notebook. A web address is a web address, and a
 * `#section` after the name is dropped — the note opens at its top.
 */
export function noteName(href: string | undefined): string | null {
  if (!href) return null;
  const bare = href.replace(/#.*$/, "");
  if (/^[a-z][a-z0-9+.-]*:/i.test(bare)) return null;
  let path: string;
  try {
    path = decodeURIComponent(bare);
  } catch {
    return null;
  }
  path = path.replace(/^\.\//, "");
  if (!/\.md$/i.test(path) || /[\\/]/.test(path)) return null;
  const name = path.slice(0, -".md".length);
  return name === "" ? null : name;
}

/** A task's marker from the start of its list item: the bullet or number, the
 *  gap, and the box, whose one character is the group. */
const TASK = /^(?:[-*+]|\d{1,9}[.)])[ \t]+\[([ xX])\]/;

/**
 * The note with one task ticked or unticked, or `null` if there is no task
 * where `at` says one starts.
 *
 * `at` is where the list item begins in the text — its bullet — which is the
 * position the Markdown parser reports for it. Refused rather than guessed at
 * when the marker is not there: the text moved under the click, and flipping
 * whichever box happens to be nearby would tick the wrong item.
 */
export function toggleTask(text: string, at: number): string | null {
  const marker = TASK.exec(text.slice(at, at + 64));
  if (!marker) return null;
  const box = at + marker[0].length - 2;
  const next = marker[1] === " " ? "x" : " ";
  return `${text.slice(0, box)}${next}${text.slice(box + 1)}`;
}

/**
 * Where each heading line starts, outside fenced blocks.
 *
 * The landmarks `lib/scrollMap.ts` lines the editor and the rendered note up
 * by. `#` headings only: an underlined heading, or one inside a quote, renders
 * as a heading this does not count — and the pane notices the two counts
 * disagree and falls back to proportion, rather than lining up the wrong ones.
 */
export function headingOffsets(text: string): number[] {
  const out: number[] = [];
  let open = false;
  let at = 0;
  for (const line of text.split("\n")) {
    if (FENCE.test(line)) open = !open;
    else if (!open && /^ {0,3}#{1,6}(\s|$)/.test(line)) out.push(at);
    at += line.length + 1;
  }
  return out;
}

const FLOWCHART = "```mermaid\nflowchart LR\n  start[Start] --> finish[Finish]\n```";
const SEQUENCE =
  "```mermaid\nsequenceDiagram\n  Client->>Server: Request\n  Server-->>Client: Response\n```";
const TABLE = "| Column | Column |\n| --- | --- |\n|  |  |";

/** The part of `whole` that is `part`, as offsets — for a placeholder. */
function around(whole: string, part: string): [number, number] {
  const at = whole.indexOf(part);
  return [at, at + part.length];
}

/**
 * The blocks, in the order the menu lists them with nothing typed.
 *
 * Ordered by how often a note wants one, not alphabetically — a heading and a
 * list are most of what anybody writes, and a divider is rare.
 */
export const BLOCKS: readonly Choice[] = [
  { label: "Heading 1", note: "#", kind: "block", insert: "# ", also: ["h1", "title"] },
  { label: "Heading 2", note: "##", kind: "block", insert: "## ", also: ["h2"] },
  { label: "Heading 3", note: "###", kind: "block", insert: "### ", also: ["h3"] },
  { label: "Bulleted list", note: "-", kind: "block", insert: "- ", also: ["ul", "bullet"] },
  { label: "Numbered list", note: "1.", kind: "block", insert: "1. ", also: ["ol", "ordered"] },
  { label: "Task", note: "- [ ]", kind: "block", insert: "- [ ] ", also: ["todo", "checkbox"] },
  { label: "Quote", note: ">", kind: "block", insert: "> ", also: ["blockquote"] },
  // The caret on the opening fence, where the language goes — which opens the
  // language list the moment this one is taken.
  { label: "Code block", note: "```", kind: "block", insert: "```\n\n```", caret: 3, also: ["fence", "snippet"] },
  {
    label: "Flowchart",
    note: "mermaid",
    kind: "diagram",
    insert: FLOWCHART,
    select: around(FLOWCHART, "Start"),
    also: ["diagram", "mermaid", "graph"],
  },
  {
    label: "Sequence diagram",
    note: "mermaid",
    kind: "diagram",
    insert: SEQUENCE,
    select: around(SEQUENCE, "Request"),
    also: ["mermaid"],
  },
  { label: "Table", note: "| |", kind: "block", insert: TABLE, select: around(TABLE, "Column"), also: ["grid"] },
  { label: "Divider", note: "---", kind: "block", insert: "---\n", also: ["rule", "hr", "line"] },
];

/**
 * The fence labels offered, beyond `mermaid`.
 *
 * The ones people actually write, not every alias `ink.ts` knows. Each is
 * marked with whether the app colours it, which is worth knowing while the
 * choice is being made, and the rest are still perfectly good labels.
 */
const LANGUAGES = [
  "ts",
  "tsx",
  "js",
  "rust",
  "python",
  "go",
  "bash",
  "sh",
  "json",
  "yaml",
  "toml",
  "markdown",
  "sql",
  "diff",
  "text",
];

/**
 * What the menu offers at `caret`, or `null` when nothing does.
 *
 * `asked` is ⌃Space, which opens the block list at the caret on a line with
 * nothing on it yet — the `/` without typing one. On a line that already has
 * words on it there is no block to offer, and asking changes nothing.
 */
export function suggest(
  text: string,
  caret: number,
  names: Names,
  asked = false,
): Offer | null {
  const { sketches, notes } = names;
  const { start } = lineAround(text, caret);
  const before = text.slice(start, caret);
  // Every trigger is cheap to test and the fence walk is not — it reads every
  // line above the caret — so it runs only once there is a trigger to refuse.
  const triggered =
    /^ {0,3}(`{3,}|~{3,})|\[|^\s*\/[\w-]*$/.test(before) || (asked && before.trim() === "");
  if (!triggered || inFence(text, start)) return null;

  const fence = /^( {0,3})(`{3,}|~{3,})([\w+#.-]*)$/.exec(before);
  if (fence) return languages(text, caret, fence[1] + fence[2], fence[3]);

  // Inside an image's address: `![caption](<Na` — the caption is kept.
  const address = /!\[[^\]\n]*\]\((<?)([^()<>\n]*)$/.exec(before);
  if (address) {
    const typed = address[2];
    const from = caret - typed.length - address[1].length;
    // A closing bracket already there is taken with the rest, rather than
    // doubled.
    const tail = text.startsWith(">)", caret) ? 2 : text.startsWith(")", caret) ? 1 : 0;
    const items = narrow(
      sketches.map((name) => ({
        label: name,
        note: "sketch",
        kind: "sketch" as const,
        insert: `<${name}.excalidraw>)`,
      })),
      typed,
    );
    return offer(from, caret + tail, items, typed);
  }

  // Inside a link's address: `[the store](<To` — any link that is not an
  // image. Only the address, never the brackets alone: `[` is typed all the
  // time for things that are not links, a task's box among them.
  const link = /(?:^|[^!])\[[^\]\n]*\]\((<?)([^()<>\n]*)$/.exec(before);
  if (link) {
    const typed = link[2];
    const from = caret - typed.length - link[1].length;
    const tail = text.startsWith(">)", caret) ? 2 : text.startsWith(")", caret) ? 1 : 0;
    const items = narrow(
      notes.map((name) => ({
        label: name,
        note: "note",
        kind: "note" as const,
        insert: `<${name}.md>)`,
      })),
      typed,
    );
    return offer(from, caret + tail, items, typed);
  }

  // Inside an image's caption: `![Au`. The whole embed line is written.
  const caption = /!\[([^\]\n]*)$/.exec(before);
  if (caption) {
    const typed = caption[1];
    return offer(
      caret - typed.length - 2,
      caret,
      narrow(sketches.map(sketchChoice), typed),
      typed,
    );
  }

  const slash = /^\s*\/([\w-]*)$/.exec(before);
  if (slash) {
    const typed = slash[1];
    return offer(caret - typed.length - 1, caret, blocks(text, start, names, typed), typed);
  }

  if (asked && before.trim() === "") {
    return offer(caret, caret, blocks(text, start, names, ""), "");
  }
  return null;
}

function sketchChoice(name: string): Choice {
  return { label: name, note: "sketch", kind: "sketch", insert: embedLine(name) };
}

function noteChoice(name: string): Choice {
  return { label: name, note: "link", kind: "note", insert: linkLine(name) };
}

/** The block list, narrowed to what was typed after the `/`. */
function blocks(text: string, lineStart: number, names: Names, typed: string): Choice[] {
  // A `---` straight under a line of text is not a divider — it turns that
  // line into a heading. So a divider there is given the blank line it needs.
  const above = text.slice(0, Math.max(0, lineStart - 1));
  const previous = above.slice(above.lastIndexOf("\n") + 1);
  const all = BLOCKS.map((block) =>
    block.label === "Divider" && lineStart > 0 && previous.trim() !== ""
      ? { ...block, insert: `\n${block.insert}` }
      : block,
  );
  return narrow(
    [...all, ...names.sketches.map(sketchChoice), ...names.notes.map(noteChoice)],
    typed,
  );
}

/**
 * The fence labels, after an opening fence's backticks.
 *
 * A fence being *opened* gets its closing line written too, with the caret on
 * the empty line between — so choosing a language is also making the block.
 * Being opened rather than relabelled is read from the fence lines below: an
 * even number of them pair up among themselves and this one has no closer yet,
 * an odd number means one of them is its closer already. The same counting
 * `inFence` does, and wrong in the same rare documents.
 */
function languages(text: string, caret: number, run: string, typed: string): Offer | null {
  const { end } = lineAround(text, caret);
  const restOfLine = text.slice(caret, end);
  const below = text.slice(end + 1).split("\n").filter((line) => FENCE.test(line));
  const opening = restOfLine.trim() === "" && below.length % 2 === 0;
  // At the opener's own indentation, so the pair lines up.
  const closer = `\n\n${run}`;

  const label = (name: string, note: string, kind: Choice["kind"] = "language"): Choice =>
    opening
      ? { label: name, note, kind, insert: `${name}${closer}`, caret: name.length + 1 }
      : { label: name, note, kind, insert: name };

  const items: Choice[] = [label("mermaid", "draws as a diagram")];
  if (opening) {
    // The two diagram templates, minus the fence this line has already
    // written — which is why they are only offered when there is no closer:
    // a template dropped onto a fence that already has one would be a block
    // with two ends.
    for (const block of BLOCKS.filter((b) => b.kind === "diagram")) {
      const body = block.insert.slice("```".length);
      const withRun = `${body.slice(0, body.lastIndexOf("\n") + 1)}${run}`;
      const skipped = "```".length;
      items.push({
        label: `mermaid · ${block.label.toLowerCase()}`,
        note: "template",
        kind: "diagram",
        insert: withRun,
        select: block.select && [block.select[0] - skipped, block.select[1] - skipped],
        also: ["mermaid", ...(block.also ?? [])],
      });
    }
  }
  for (const name of LANGUAGES) {
    items.push(label(name, grammarFor(name) ? "colored" : "plain text"));
  }
  return offer(caret - typed.length, caret, narrow(items, typed), typed);
}

/**
 * Keeps what matches what was typed, prefix matches first, and otherwise the
 * order the list was built in — which is deliberate everywhere it is built.
 */
function narrow(items: Choice[], typed: string): Choice[] {
  const want = typed.toLowerCase();
  if (want === "") return items;
  return items
    .map((item, i) => ({ item, i, rank: rank(item, want) }))
    .filter(({ rank }) => rank >= 0)
    .sort((a, b) => a.rank - b.rank || a.i - b.i)
    .map(({ item }) => item);
}

/** Lower is better; `-1` is no match. A word of the label, or one of the
 *  words it answers to, starting with what was typed beats the label merely
 *  containing it. */
function rank(item: Choice, want: string): number {
  const label = item.label.toLowerCase();
  const words = [label, ...label.split(/[\s·]+/), ...(item.also ?? [])];
  if (words.some((word) => word.startsWith(want))) return 0;
  if (label.includes(want)) return 1;
  return -1;
}

function offer(from: number, to: number, items: Choice[], typed: string): Offer | null {
  if (items.length === 0) return null;
  // One choice that would write exactly what is already there is the finished
  // case. A menu hovering over it would make Enter a no-op instead of a newline.
  if (items.length === 1 && items[0].insert === typed) return null;
  return { from, to, items: items.slice(0, MAX_CHOICES) };
}

/** A list item's marker: indentation, the bullet or number, the gap, a task box. */
const ITEM = /^(\s*)(?:([-*+])|(\d{1,9})([.)]))( +)(\[[ xX]\] +)?/;
/** A quote's markers, however deep. */
const QUOTE = /^\s*(?:> ?)+/;
/** A horizontal rule, which starts like a list item and is not one. */
const RULE = /^\s*([-*_])(\s*\1){2,}\s*$/;

/**
 * What Enter does on a list item or in a quote, or `null` for a plain newline.
 *
 * The next line starts with the same marker — the next number, an unticked
 * box, the same depth of quote — so a list is written by typing items rather
 * than by typing markers. On an item with nothing in it Enter takes the marker
 * away instead, which is how the list ends: the way every editor that does the
 * first half also does the second, and the only way out that does not need a
 * key nobody would guess.
 *
 * Not inside a fenced block, where a `- ` is code, and not with the caret in
 * the marker itself, which is somebody editing the marker.
 */
export function carry(text: string, caret: number): Edit | null {
  const { start, end } = lineAround(text, caret);
  const line = text.slice(start, end);
  if (RULE.test(line)) return null;

  const item = ITEM.exec(line);
  const quote = item ? null : QUOTE.exec(line);
  const marker = item?.[0] ?? quote?.[0];
  if (!marker) return null;
  if (caret - start < marker.length) return null;
  // Last, because it reads every line above: most Enters are not on an item.
  if (inFence(text, start)) return null;

  if (line.slice(marker.length).trim() === "") {
    return { from: start, to: end, insert: "", caret: start };
  }

  let next: string;
  if (item) {
    const [, indent, bullet, number, delim, gap, task] = item;
    next = `${indent}${bullet ?? `${Number(number) + 1}${delim}`}${gap}${task ? "[ ] " : ""}`;
  } else {
    next = `${marker.trimEnd()} `;
  }
  const insert = `\n${next}`;
  return { from: caret, to: caret, insert, caret: caret + insert.length };
}
