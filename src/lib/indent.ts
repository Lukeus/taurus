/**
 * Tab and Enter in a file of code.
 *
 * Pure, working out what to change and leaving the change to `replaceText`, so
 * the arithmetic can be tested without a browser.
 */

/** A stretch of text to replace, and where the selection goes afterwards. */
export type Edit = { from: number; to: number; insert: string; start: number; end: number };

/** How far to look for how a file indents. A file doesn't change its mind. */
const SAMPLE_LINES = 400;

/**
 * One level of indentation, as this file already writes it.
 *
 * A tab when tabs are what its indented lines start with. Otherwise the smallest
 * indent of two or more spaces, which is the step: a deeper line is several of
 * them, and one space is alignment. With nothing to go on, tabs for the two
 * languages that want them and four spaces for the rest.
 */
export function indentOf(text: string, grammar: string | null): string {
  let smallest = Infinity;
  for (const line of text.split("\n", SAMPLE_LINES)) {
    if (line[0] === "\t") return "\t";
    if (line[0] !== " " || !line.trim()) continue;
    const width = line.length - line.trimStart().length;
    if (width >= 2 && width < smallest) smallest = width;
  }
  if (smallest !== Infinity) return " ".repeat(Math.min(smallest, 8));
  return grammar === "go" || grammar === "makefile" ? "\t" : "    ";
}

const lineStart = (text: string, at: number) => text.lastIndexOf("\n", at - 1) + 1;

/**
 * What Tab does: a level at the caret, or a level on every line selected.
 *
 * Any selection indents its lines and never replaces them, because Tab typed over
 * a highlighted word that vanishes is a surprise nobody asks for.
 */
export function tabEdit(text: string, start: number, end: number, unit: string): Edit {
  if (start === end) {
    return { from: start, to: end, insert: unit, start: start + unit.length, end: end + unit.length };
  }
  return eachLine(text, start, end, (line) => (line ? unit + line : line));
}

/** What Shift-Tab does: a level off every line the caret or selection touches. */
export function outdentEdit(text: string, start: number, end: number, unit: string): Edit {
  return eachLine(text, start, end, (line) => {
    if (line.startsWith(unit)) return line.slice(unit.length);
    if (line[0] === "\t") return line.slice(1);
    const spaces = line.length - line.replace(/^ +/, "").length;
    return line.slice(Math.min(spaces, unit === "\t" ? 4 : unit.length));
  });
}

function eachLine(text: string, start: number, end: number, change: (line: string) => string): Edit {
  const from = lineStart(text, start);
  // A selection ending at the start of a line hasn't selected any of that line.
  const last = end > start && text[end - 1] === "\n" ? end - 1 : end;
  const next = text.indexOf("\n", last);
  const to = next === -1 ? text.length : next;

  const lines = text.slice(from, to).split("\n");
  const changed = lines.map(change);
  const insert = changed.join("\n");

  // The caret stays on its character, and never slides back onto the line before.
  const moved = changed[0].length - lines[0].length;
  const newStart = Math.max(from, start + moved);
  if (start === end) return { from, to, insert, start: newStart, end: newStart };
  return { from, to, insert, start: newStart, end: Math.max(newStart, end + insert.length - (to - from)) };
}

const CLOSER: Record<string, string> = { "{": "}", "[": "]", "(": ")" };

/**
 * What Enter does: a new line at the indent of this one.
 *
 * One level deeper after an opening bracket, or after a colon in Python. Between
 * a pair of brackets the closer gets a line of its own. Only in code: `grammar` is
 * `null` or `"markdown"` for prose, where a line ending in `(` opens nothing.
 */
export function enterEdit(
  text: string,
  start: number,
  end: number,
  unit: string,
  grammar: string | null,
): Edit {
  const line = text.slice(lineStart(text, start), start);
  const indent = line.slice(0, line.length - line.trimStart().length);
  const code = grammar !== null && grammar !== "markdown";
  const opener = line.trimEnd().slice(-1);
  const opens = code && (opener in CLOSER || (grammar === "python" && opener === ":"));

  if (opens && CLOSER[opener] && text[end] === CLOSER[opener]) {
    const before = `\n${indent}${unit}`;
    const caret = start + before.length;
    return { from: start, to: end, insert: `${before}\n${indent}`, start: caret, end: caret };
  }
  const insert = `\n${indent}${opens ? unit : ""}`;
  const caret = start + insert.length;
  return { from: start, to: end, insert, start: caret, end: caret };
}

/**
 * Replaces a stretch of a textarea the way typing would.
 *
 * `insertText` is the one route into the browser's own undo history, so it is
 * tried first; a ⌘Z afterwards takes back that edit rather than the whole history
 * with it. Where it is missing — jsdom, and in principle any engine that finally
 * drops a deprecated command — the text goes in directly and an `input` is sent,
 * which is what the component listens to either way.
 */
export function replaceText(area: HTMLTextAreaElement, from: number, to: number, insert: string) {
  area.focus();
  area.setSelectionRange(from, to);
  const exec = typeof document.execCommand === "function" ? document.execCommand.bind(document) : null;
  // An empty insert is a deletion, which `insertText` does not reliably do.
  const done = exec
    ? insert === ""
      ? from === to || exec("delete", false)
      : exec("insertText", false, insert)
    : false;
  if (done) return;
  area.setRangeText(insert, from, to, "end");
  area.dispatchEvent(new Event("input", { bubbles: true }));
}
