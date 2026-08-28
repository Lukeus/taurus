/**
 * Enough of a language to colour it.
 *
 * Not a parser, and it must not become one — the same rule the SQL box already
 * follows, for the same reason. Everything here answers one question: what
 * colour is this run of characters. That answer stays useful when it is
 * approximate, because nothing downstream of it executes anything. A grammar
 * good enough to be right about a fenced block would be a second compiler per
 * language to keep in step with the first, and the payoff would be the same
 * eight colours.
 *
 * Eight is the whole palette, and it is the palette the query box already has
 * CSS for. The categories a grammar distinguishes are more numerous than the
 * colours a reader can tell apart, so the split is by what a run *does* —
 * vocabulary, value, name, aside — rather than by what the standard calls it.
 *
 * One scanner serves every language. What differs between them is a handful of
 * facts — how a comment opens, which delimiters quote a string and whether the
 * quote may cross a line, which words are the vocabulary — so those are data
 * and the walk over the characters is not. A language nobody has written a
 * `Grammar` for comes back as one plain run, which renders exactly as it does
 * today; the label above the block still says what it is.
 */

/** What a run of characters is, for the one purpose of tinting it. */
export type InkKind =
  | "keyword"
  | "fn"
  | "string"
  | "quoted"
  | "number"
  | "comment"
  | "punct"
  | "plain";

export type Ink = { text: string; kind: InkKind };

/**
 * One way to open a literal.
 *
 * `kind` is separate from the delimiter because a double quote is not the same
 * thing in every language: in SQL it names a column and in Rust it holds text,
 * and the colours say so.
 */
type Quote = {
  delim: string;
  kind: "string" | "quoted";
  /** How the delimiter is escaped inside the literal: `''` or `\'`. */
  escape: "double" | "backslash";
  /**
   * How much the literal may hold.
   *
   * Set to `"one"` for the character literal that Rust, Go, and C all spell with a
   * single quote: one character or one escape, and then the closing quote —
   * never a run of text. Saying so is what tells a lifetime from a string.
   * `&'a str` has a quote and, later on the same line, another one, and a
   * scanner that only asks "is there a partner" answers yes and paints the
   * signature red. Asking "is there a partner *one character along*" answers
   * no, and the quote falls through to punctuation, which is what it is.
   *
   * Left off everywhere else, which is most places: a delimiter that holds
   * whatever is between it and its partner needs nothing said about it.
   */
  width?: "one";
  /**
   * Whether the literal may cross a line ending.
   *
   * False is the useful default and the reason Rust lifetimes are not one
   * long red smear: `'a` opens nothing, so a quote with no partner before the
   * newline is punctuation rather than the start of a string that swallows the
   * rest of the file. True is for the two cases where crossing is ordinary —
   * a template literal, and a query still being typed.
   */
  spans: boolean;
};

type Grammar = {
  /** Openers that comment out the rest of the line. */
  line: string[];
  /** Opener/closer pairs that comment out everything between them. */
  block: [string, string][];
  /** Tried in order, so a triple quote has to be listed before a single one. */
  quotes: Quote[];
  keywords: Set<string>;
  /**
   * Words tinted as keywords though a grammar would call them something else —
   * the primitive types, mostly. `let x: u32` reads as one thought, and
   * colouring `u32` differently from `let` splits it into two.
   */
  types: Set<string>;
  /**
   * Which identifiers before a `(` are a call.
   *
   * `"any"` for languages where that is simply what the syntax means. A set,
   * for SQL, where `count` is both a function and a perfectly ordinary column
   * name — tinting the column would be saying something false about it.
   */
  calls: Set<string> | "any";
  /** Whether the vocabulary is matched without regard to case. SQL only. */
  fold: boolean;
};

const words = (source: string) => new Set(source.split(/\s+/).filter(Boolean));

/** The delimiters every C-descendant here agrees about. */
const C_QUOTES: Quote[] = [
  { delim: '"', kind: "string", escape: "backslash", spans: false },
  { delim: "'", kind: "string", escape: "backslash", spans: false },
];

/** The same pair, for the languages where a single quote holds one character. */
const CHAR_QUOTES: Quote[] = [
  { delim: '"', kind: "string", escape: "backslash", spans: false },
  { delim: "'", kind: "string", escape: "backslash", spans: false, width: "one" },
];

const GRAMMARS: Record<string, Grammar> = {
  rust: {
    line: ["//"],
    block: [["/*", "*/"]],
    // A lifetime is a `'` that no character-literal shape fits, which
    // `width: "one"` turns into punctuation. Byte and raw strings open with a
    // letter and so are read as one identifier followed by an ordinary
    // literal — wrong about the prefix, right about the text, and cheap.
    quotes: CHAR_QUOTES,
    keywords: words(`as async await break const continue crate dyn else enum extern false fn for
      if impl in let loop match mod move mut pub ref return self Self static struct super trait
      true type unsafe use where while union`),
    types: words(`bool char str u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize f32 f64
      String Vec Option Some None Result Ok Err Box Arc Rc HashMap HashSet BTreeMap`),
    calls: "any",
    fold: false,
  },
  ts: {
    line: ["//"],
    block: [["/*", "*/"]],
    quotes: [
      // Template literals cross lines by design, which is the one place
      // `spans` earns its keep outside SQL.
      { delim: "`", kind: "string", escape: "backslash", spans: true },
      ...C_QUOTES,
    ],
    keywords: words(`abstract as async await break case catch class const continue debugger
      declare default delete do else enum export extends false finally for from function get
      if implements import in instanceof interface keyof let new null of package private
      protected public readonly return satisfies set static super switch this throw true try
      type typeof undefined var void while with yield`),
    types: words(`any bigint boolean never number object string symbol unknown Array Promise
      Record Partial Readonly Map Set`),
    calls: "any",
    fold: false,
  },
  python: {
    line: ["#"],
    block: [],
    quotes: [
      { delim: '"""', kind: "string", escape: "backslash", spans: true },
      { delim: "'''", kind: "string", escape: "backslash", spans: true },
      ...C_QUOTES,
    ],
    keywords: words(`and as assert async await break class continue def del elif else except
      False finally for from global if import in is lambda None nonlocal not or pass raise
      return True try while with yield match case`),
    types: words(`bool bytes dict float int list object set str tuple Any Optional Callable`),
    calls: "any",
    fold: false,
  },
  go: {
    line: ["//"],
    block: [["/*", "*/"]],
    quotes: [
      { delim: "`", kind: "string", escape: "backslash", spans: true },
      ...CHAR_QUOTES,
    ],
    keywords: words(`break case chan const continue default defer else fallthrough for func go
      goto if import interface map package range return select struct switch type var nil true
      false`),
    types: words(`bool byte complex64 complex128 error float32 float64 int int8 int16 int32
      int64 rune string uint uint8 uint16 uint32 uint64 uintptr any`),
    calls: "any",
    fold: false,
  },
  shell: {
    line: ["#"],
    block: [],
    quotes: [
      // Single quotes take no escape at all in a shell, and `escape: "double"`
      // is the closest thing to that: `'\''` is not one literal, and reading
      // it as two is what actually happens.
      { delim: "'", kind: "string", escape: "double", spans: false },
      { delim: '"', kind: "string", escape: "backslash", spans: false },
    ],
    keywords: words(`if then elif else fi for while until do done case esac in function select
      time coproc return break continue local export readonly declare unset shift source exit
      trap set`),
    types: new Set<string>(),
    // Nothing. A shell has no call syntax to speak of — `foo(` is rarer than
    // `$(`, and tinting the latter's contents as a call would be noise.
    calls: new Set<string>(),
    fold: false,
  },
  json: {
    line: [],
    block: [],
    quotes: [{ delim: '"', kind: "string", escape: "backslash", spans: false }],
    keywords: words(`true false null`),
    types: new Set<string>(),
    calls: new Set<string>(),
    fold: false,
  },
  yaml: {
    line: ["#"],
    block: [],
    quotes: [
      { delim: '"', kind: "string", escape: "backslash", spans: false },
      { delim: "'", kind: "string", escape: "double", spans: false },
    ],
    keywords: words(`true false null yes no on off`),
    types: new Set<string>(),
    calls: new Set<string>(),
    fold: false,
  },
  toml: {
    line: ["#"],
    block: [],
    quotes: [
      { delim: '"""', kind: "string", escape: "backslash", spans: true },
      { delim: "'''", kind: "string", escape: "double", spans: true },
      { delim: '"', kind: "string", escape: "backslash", spans: false },
      { delim: "'", kind: "string", escape: "double", spans: false },
    ],
    keywords: words(`true false`),
    types: new Set<string>(),
    calls: new Set<string>(),
    fold: false,
  },
};

/**
 * What a fence label or a file extension means.
 *
 * The keys are what people actually type above a block and what actually ends
 * a filename, which is not one list — `sh`, `bash`, and `zsh` are three names
 * for the colours here, and so are `ts` and `tsx`.
 */
const ALIASES: Record<string, string> = {
  rs: "rust",
  rust: "rust",
  ts: "ts",
  tsx: "ts",
  js: "ts",
  jsx: "ts",
  mjs: "ts",
  cjs: "ts",
  mts: "ts",
  cts: "ts",
  javascript: "ts",
  typescript: "ts",
  py: "python",
  python: "python",
  go: "go",
  golang: "go",
  sh: "shell",
  bash: "shell",
  zsh: "shell",
  shell: "shell",
  console: "shell",
  json: "json",
  jsonc: "json",
  yaml: "yaml",
  yml: "yaml",
  toml: "toml",
  md: "markdown",
  markdown: "markdown",
  mdx: "markdown",
};

/**
 * The grammar for a fence label, a file extension, or a whole path.
 *
 * `null` for everything else, which is most things, and that is the honest
 * answer rather than a failure — see the note at the top of the file about
 * what an unknown language renders as.
 */
export function grammarFor(hint: string | null | undefined): string | null {
  if (!hint) return null;
  const cleaned = hint.toLowerCase().trim();
  if (cleaned in ALIASES) return ALIASES[cleaned];
  // A path, then: the part after the last dot, and only if there was a dot
  // after the last separator — `.gitignore` and `Makefile` name no language.
  const name = cleaned.split(/[\\/]/).pop() ?? "";
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return null;
  const extension = name.slice(dot + 1);
  return extension in ALIASES ? ALIASES[extension] : null;
}

/**
 * Splits source into runs to tint.
 *
 * Every character of the input comes back exactly once, in order. That is the
 * property the tests hold rather than any particular colour, and it is the one
 * that matters: the runs are concatenated back into a line of code, so a
 * scanner that dropped or doubled a character would be showing something the
 * file does not say.
 */
export function paint(source: string, grammar: string | null): Ink[] {
  // Markdown is not a `Grammar` and could not be made into one honestly. Every
  // entry above describes a language whose structure is comments, literals and
  // words; Markdown's is headings, fences and emphasis, and there is no set of
  // keywords that would say anything true about it. Rather than bend the shape
  // until it fits, it gets its own walk — and gives back the same `Ink[]`, so
  // the editor, the transcript and the diff paint it with the palette they
  // already have.
  if (grammar === "markdown") return markdown(source);
  const g = grammar ? GRAMMARS[grammar] : undefined;
  if (!g) return source.length ? [{ text: source, kind: "plain" }] : [];
  return scan(source, g);
}

/**
 * Markdown source, tinted line by line.
 *
 * Line by line because that is how Markdown is structured: a heading is a line,
 * a fence toggles a mode until the next fence, and a list marker is only a list
 * marker at the start of one. What is *inside* a line — code spans, emphasis,
 * links — is found by a second, smaller walk that runs only on the lines where
 * it can matter.
 *
 * Deliberately shallow. This is colour for somebody editing prose, not a
 * parser: it does not track reference links, nested emphasis, or setext
 * headings, and it does not need to. What it does hold is the property every
 * painter here holds — every character of the input comes back exactly once,
 * in order — because the runs are laid under a textarea and one dropped
 * character slides the rest of the file out of line.
 */
function markdown(source: string): Ink[] {
  const out: Ink[] = [];
  const push = (text: string, kind: InkKind) => {
    if (!text) return;
    // Runs of the same kind are merged as they are added rather than in a pass
    // afterwards. A paragraph is otherwise one run per word.
    const last = out[out.length - 1];
    if (last && last.kind === kind) last.text += text;
    else out.push({ text, kind });
  };

  let fenced = false;
  // `split` on the newline rather than a line iterator, so the newlines
  // themselves are still ours to put back — a painter that dropped them would
  // paint the whole file on one line.
  const lines = source.split("\n");
  lines.forEach((line, i) => {
    const newline = i < lines.length - 1 ? "\n" : "";

    // A fence is its own line, and it flips what the lines after it mean. The
    // opener and closer both tint as the code they delimit.
    if (/^\s*(```|~~~)/.test(line)) {
      fenced = !fenced;
      push(line, "string");
      push(newline, "plain");
      return;
    }
    if (fenced) {
      // Not painted as the language it declares. Inside an editor the fence's
      // contents are still the file's text, and switching palettes mid-document
      // is a rainbow rather than a reading aid.
      push(line, "string");
      push(newline, "plain");
      return;
    }

    const heading = /^(#{1,6}\s)(.*)$/.exec(line);
    if (heading) {
      // The whole line, marker included. A heading is a heading because of how
      // it reads, and tinting only the hashes would be colouring the syntax
      // and not the thing.
      push(heading[1] + heading[2], "keyword");
      push(newline, "plain");
      return;
    }

    // A block quote or a horizontal rule: the marker carries the meaning and
    // the rest is ordinary prose.
    const quoted = /^(\s*>+\s?)(.*)$/.exec(line);
    if (quoted) {
      push(quoted[1], "comment");
      inline(quoted[2], push);
      push(newline, "plain");
      return;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      push(line, "punct");
      push(newline, "plain");
      return;
    }

    // A list marker, numbered or not, and a task box after it.
    const list = /^(\s*(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?)(.*)$/.exec(line);
    if (list) {
      push(list[1], "punct");
      inline(list[2], push);
      push(newline, "plain");
      return;
    }

    inline(line, push);
    push(newline, "plain");
  });

  return out;
}

/**
 * What can appear inside one line of Markdown prose.
 *
 * One regular expression with alternatives rather than four passes, because
 * they compete: the `` ` `` in `` `**not bold**` `` wins, and only a single
 * left-to-right walk gets that right. Code spans are listed first for exactly
 * that reason.
 */
function inline(line: string, push: (text: string, kind: InkKind) => void) {
  const pattern =
    /(`[^`]*`)|(\*\*[^*]+\*\*|__[^_]+__)|(\*[^*\s][^*]*\*|_[^_\s][^_]*_)|(\[[^\]]*\]\([^)]*\))/g;
  let at = 0;
  for (const m of line.matchAll(pattern)) {
    const start = m.index ?? 0;
    push(line.slice(at, start), "plain");
    // Code spans read as code; emphasis and links read as marked-up prose. Two
    // colours rather than four, because a line wearing four is harder to read
    // than the one it replaced.
    push(m[0], m[1] ? "string" : m[4] ? "fn" : "keyword");
    at = start + m[0].length;
  }
  push(line.slice(at), "plain");
}

/** The grammar for SQL is assembled by `sql.ts`, which owns the vocabulary. */
export function scanWith(source: string, g: Grammar): Ink[] {
  return scan(source, g);
}

export type { Grammar, Quote };
export { words };

function scan(source: string, g: Grammar): Ink[] {
  const out: Ink[] = [];
  let i = 0;

  const push = (text: string, kind: InkKind) => {
    if (!text) return;
    // Runs of the same kind are merged, which keeps the painted DOM to a few
    // spans per line instead of one per character of whitespace.
    const last = out[out.length - 1];
    if (last && last.kind === kind) last.text += text;
    else out.push({ text, kind });
  };

  while (i < source.length) {
    const c = source[i];

    const line = g.line.find((open) => source.startsWith(open, i));
    if (line) {
      const end = source.indexOf("\n", i);
      const stop = end === -1 ? source.length : end;
      push(source.slice(i, stop), "comment");
      i = stop;
      continue;
    }

    const block = g.block.find(([open]) => source.startsWith(open, i));
    if (block) {
      const [open, close] = block;
      const end = source.indexOf(close, i + open.length);
      const stop = end === -1 ? source.length : end + close.length;
      push(source.slice(i, stop), "comment");
      i = stop;
      continue;
    }

    const quote = g.quotes.find((q) => source.startsWith(q.delim, i));
    if (quote) {
      const end = closes(source, i, quote);
      if (end === null) {
        // No partner before the line ended and the literal may not cross one:
        // this is an apostrophe or a lifetime, not an opening.
        push(quote.delim, "punct");
        i += quote.delim.length;
      } else {
        push(source.slice(i, end), quote.kind);
        i = end;
      }
      continue;
    }

    if (/[0-9]/.test(c)) {
      const end = number(source, i);
      push(source.slice(i, end), "number");
      i = end;
      continue;
    }

    if (/[A-Za-z_$]/.test(c)) {
      let j = i;
      while (j < source.length && /[A-Za-z0-9_$]/.test(source[j])) j += 1;
      const word = source.slice(i, j);
      const key = g.fold ? word.toLowerCase() : word;
      push(word, kindOfWord(g, key, source, j));
      i = j;
      continue;
    }

    if (/[(){}[\],;.:*=<>+\-/%|&!?~^@#\\]/.test(c)) {
      push(c, "punct");
      i += 1;
      continue;
    }

    push(c, "plain");
    i += 1;
  }

  return out;
}

function kindOfWord(
  g: Grammar,
  key: string,
  source: string,
  after: number,
): InkKind {
  if (g.keywords.has(key) || g.types.has(key)) return "keyword";
  // Only when it is actually being called, in either language: the `(` may be
  // preceded by whitespace but not by anything else.
  const called = source.slice(after).trimStart().startsWith("(");
  if (!called) return "plain";
  if (g.calls === "any") return "fn";
  return g.calls.has(key) ? "fn" : "plain";
}

/**
 * Where a literal opened at `start` ends, or `null` if it never does.
 *
 * The walk is what handles a doubled delimiter — `''` inside a `'…'` is an
 * escaped quote rather than an empty literal followed by whatever comes next —
 * and a backslash, which takes the character after it whatever that is. A
 * literal that runs off the end of the input closes there when it is allowed
 * to span, because the last line of a fenced block is often mid-string.
 */
function closes(source: string, start: number, quote: Quote): number | null {
  const { delim } = quote;
  if (quote.width === "one") {
    const found = CHAR_LITERAL.exec(source.slice(start));
    return found ? start + found[0].length : null;
  }
  let j = start + delim.length;
  while (j < source.length) {
    if (!quote.spans && source[j] === "\n") return null;
    if (quote.escape === "backslash" && source[j] === "\\") {
      j += 2;
      continue;
    }
    if (source.startsWith(delim, j)) {
      if (quote.escape === "double" && source.startsWith(delim, j + delim.length)) {
        j += delim.length * 2;
        continue;
      }
      return j + delim.length;
    }
    j += 1;
  }
  return quote.spans ? source.length : null;
}

/**
 * One character between single quotes, and the escapes that count as one.
 *
 * The named escapes are spelled out rather than covered by `\\.` alone
 * because `'\\u{1F600}'` is one character and `\\.` would stop at the `u`,
 * leaving the brace to close nothing.
 */
const CHAR_LITERAL = /^'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\}|.)|[^'\\\n])'/;

/**
 * Where a number starting at `start` ends.
 *
 * A radix prefix takes every letter that follows, because `0xdeadbeef` is one
 * number. Everything else takes digits and separators, and a dot only when a
 * digit follows it — which is what keeps `1.max()` from swallowing the method
 * it is being called on.
 */
function number(source: string, start: number): number {
  let j = start;
  if (source[j] === "0" && /[xXoObB]/.test(source[j + 1] ?? "")) {
    j += 2;
    while (j < source.length && /[0-9A-Fa-f_]/.test(source[j])) j += 1;
    return j;
  }
  while (j < source.length) {
    if (/[0-9_]/.test(source[j])) {
      j += 1;
      continue;
    }
    if (source[j] === "." && /[0-9]/.test(source[j + 1] ?? "")) {
      j += 1;
      continue;
    }
    // An exponent, and the sign that may follow it.
    if (/[eE]/.test(source[j]) && /[0-9+-]/.test(source[j + 1] ?? "")) {
      j += 2;
      continue;
    }
    break;
  }
  return j;
}

/** A painted run, and whether it falls inside a region worth marking. */
export type Marked = Ink & { changed: boolean };

/**
 * Splits painted runs at a character range, so a region can be marked without
 * losing the colours underneath it.
 *
 * The two answers are independent — what a run *is* comes from the grammar,
 * whether it *changed* comes from the diff — and they are drawn on the same
 * characters, so one of them has to be expressed as a split. Colour is the one
 * that cannot be: a run cut in half is still the same colour, and a background
 * cut in half is a different mark.
 *
 * Offsets are into the concatenated text, which is the same string the runs
 * were painted from. Passing `null` marks nothing and still splits nothing,
 * which is the ordinary case and stays free.
 */
export function mark(runs: Ink[], span: { from: number; to: number } | null): Marked[] {
  if (!span || span.to <= span.from) return runs.map((run) => ({ ...run, changed: false }));

  const out: Marked[] = [];
  let at = 0;
  for (const run of runs) {
    const start = at;
    const end = at + run.text.length;
    at = end;
    // The three pieces a run can be cut into: before the span, inside it,
    // after it. Each is emitted only if it has characters.
    const cuts: [number, number, boolean][] = [
      [start, Math.min(end, span.from), false],
      [Math.max(start, span.from), Math.min(end, span.to), true],
      [Math.max(start, span.to), end, false],
    ];
    for (const [from, to, changed] of cuts) {
      if (to <= from) continue;
      out.push({ text: run.text.slice(from - start, to - start), kind: run.kind, changed });
    }
  }
  return out;
}
