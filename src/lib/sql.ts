/**
 * Enough SQL to colour it and to finish a word.
 *
 * Not a parser, and it must not become one. Everything here answers one of two
 * questions — what colour is this run of characters, and what could the word
 * under the caret be — and both stay useful when the answer is approximate,
 * because the engine is the thing that actually reads the query. A parser would
 * be a second dialect to keep in step with DataFusion's, which is the mistake
 * the query box already declines to make about read-only checking.
 *
 * Both halves are pure and live here rather than in the component, because
 * both are worth testing on their own and neither needs a DOM.
 */

import type { DataTable } from "./api";

import { scanWith, words, type Grammar, type Ink, type InkKind } from "./ink";

// The colours and the scanner are shared with every other language the app
// draws — see `ink.ts` for why one walk serves all of them. What stays here is
// the vocabulary, because the completion menu is built out of the same two
// sets and a second copy of them is a copy that can disagree.
export type { Ink, InkKind };

/*
 * The words, split by what they do rather than by what the standard calls
 * them. `keyword` is the shape of the statement — the parts you would read
 * aloud — and `fn` is anything that takes arguments and gives a value back.
 * Two colours is what a reader can use; the eight categories a grammar
 * distinguishes are eight colours nobody can tell apart.
 */
const KEYWORDS = words(`select from where group by having order limit offset as join inner left right
   full outer cross on using union all except intersect distinct case when then
   else end and or not in is null like ilike between exists asc desc with
   recursive over partition rows range unbounded preceding following current row
   cast filter nulls first last values`);

const FUNCTIONS = words(`count sum avg min max median stddev variance array_agg string_agg
   coalesce nullif greatest least abs ceil floor round trunc sign power sqrt
   exp ln log mod random
   length lower upper trim ltrim rtrim substr substring replace concat
   split_part starts_with regexp_match regexp_replace left right lpad rpad
   date_trunc date_part extract now to_timestamp to_date make_date age
   row_number rank dense_rank lag lead first_value last_value ntile
   unnest struct arrow_cast try_cast`);

/** Types a `CAST` names, tinted as keywords so a cast reads as one thing. */
const TYPES = words(`int integer bigint smallint tinyint float double decimal numeric real
   varchar text string char boolean bool date timestamp time interval bytea`);

/**
 * SQL as the scanner needs to be told about it.
 *
 * Two facts here are this dialect's alone. A double quote names a column
 * rather than holding text, which is the distinction the query box exists to
 * make visible — `"Material"` is the column and `'Material'` is the word. And
 * a literal with no closing quote runs to the end of the input rather than
 * ending at the newline, because in a box someone is typing in, an unfinished
 * string is the normal state of an unfinished query.
 */
const SQL: Grammar = {
  line: ["--"],
  block: [["/*", "*/"]],
  quotes: [
    { delim: "'", kind: "string", escape: "double", spans: true },
    { delim: '"', kind: "quoted", escape: "double", spans: true },
  ],
  keywords: KEYWORDS,
  types: TYPES,
  calls: FUNCTIONS,
  fold: true,
};

/**
 * Splits SQL into runs to tint.
 *
 * Every character of the input comes back exactly once, in order — the caller
 * paints these behind a transparent textarea, so a scanner that dropped or
 * added one would slide the whole rest of the query out of alignment. The
 * tests hold that property directly rather than checking colours.
 */
export function ink(sql: string): Ink[] {
  return scanWith(sql, SQL);
}

/** One thing the box offers to finish the word with. */
export type Suggestion = {
  /** What gets inserted, already quoted if it has to be. */
  insert: string;
  /** What the row reads as. The bare name, so a quoted insert is still
   *  recognisable as the column the profile listed. */
  label: string;
  /** The dimmer half of the row: which table, or what type. */
  note: string;
  kind: "table" | "column" | "keyword" | "fn";
  /** Whether more than one table has this column — which is to say, whether
   *  it is a key two of them could be joined on. */
  shared?: boolean;
};

/** Where the caret is, and what is around it. */
export type Spot = {
  /** The partial word being completed, possibly empty. */
  prefix: string;
  /** Where that word starts, so an accepted suggestion knows what to replace. */
  from: number;
  /** The table or alias before a `.`, when the caret is after one. */
  qualifier: string | null;
  /** Whether the word sits where a table name goes — after FROM or JOIN. */
  naming: boolean;
};

// The word is optional in both of these, and that is the case that matters:
// the caret sitting just after `FROM ` or just after `i.` is exactly when
// somebody wants to be told what the choices are. No `\s*` around the dot —
// `i. ` with a space has moved on to the next word, and treating it as still
// qualified would offer columns where a keyword goes.
const AFTER_TABLE = /\b(?:from|join)\s+([A-Za-z_][\w]*)?$/i;
const QUALIFIED = /([A-Za-z_][\w]*)\.([A-Za-z_][\w]*)?$/;
const WORD = /[A-Za-z_][\w]*$/;

/**
 * Reads the caret's surroundings out of the text before it.
 *
 * Before it and nothing after, deliberately. What comes after the caret is
 * whatever was there before this word started being typed, and letting it
 * decide the suggestions makes the list change as the cursor is walked back
 * through a finished query — which is the opposite of what a completion list
 * is for.
 */
export function spot(sql: string, caret: number): Spot {
  const before = sql.slice(0, caret);

  const qualified = QUALIFIED.exec(before);
  if (qualified) {
    const partial = qualified[2] ?? "";
    return {
      prefix: partial,
      from: caret - partial.length,
      qualifier: qualified[1],
      naming: false,
    };
  }

  const word = WORD.exec(before);
  const prefix = word ? word[0] : "";
  return {
    prefix,
    from: caret - prefix.length,
    qualifier: null,
    naming: AFTER_TABLE.test(before),
  };
}

/**
 * What each name in a `FROM`/`JOIN` clause is called for the rest of the query.
 *
 * `FROM interactions i JOIN items AS m` — both spellings, because both are
 * written. The table's own name maps to itself as well, since `interactions.id`
 * is legal whether or not an alias was given.
 *
 * Approximate on purpose. The one case it gets wrong is `FROM events WHERE`,
 * where `WHERE` sits exactly where an alias would; the keyword check is what
 * stops that, and anything subtler than that belongs in a parser this file is
 * not going to grow into.
 */
export function aliases(sql: string): Map<string, string> {
  const found = new Map<string, string>();
  const pattern = /\b(?:from|join)\s+([A-Za-z_][\w]*)(?:\s+(?:as\s+)?([A-Za-z_][\w]*))?/gi;
  for (const match of sql.matchAll(pattern)) {
    const table = match[1];
    found.set(table.toLowerCase(), table);
    const alias = match[2];
    if (alias && !KEYWORDS.has(alias.toLowerCase())) {
      found.set(alias.toLowerCase(), table);
    }
  }
  return found;
}

/** How many rows the menu shows before it stops. Beyond about this many the
 *  list is faster to scroll past than to read, and the answer is to type
 *  another letter. */
const MAX_SUGGESTIONS = 10;

/**
 * What could come next, best first.
 *
 * The ordering is the whole design. A prefix match beats a match in the middle
 * of a word, because that is what typing three letters meant. Within that,
 * identifiers beat keywords: `SELECT` is known by heart and `Price_Per_Unit`
 * is not, and the list exists for the half nobody can remember.
 *
 * A column that appears in more than one table is marked, and that is the
 * feature this was built for. Two files that share a `user_id` are two files
 * that can be joined on it, and the completion list is where that fact is
 * cheapest to notice — you find the join key while writing the join rather
 * than by opening two profiles side by side.
 */
export function suggest(
  sql: string,
  caret: number,
  tables: DataTable[],
): { at: Spot; items: Suggestion[] } {
  const at = spot(sql, caret);
  const named = aliases(sql);
  const shared = sharedColumns(tables);
  const candidates: Suggestion[] = [];

  // `qualified` only changes what the note says. What gets inserted is the
  // bare column either way — after `i.` the table is already written, and
  // before it, inserting `items.price` would finish a word nobody started.
  const columnsOf = (table: DataTable, qualified: boolean) =>
    table.columns.map((column) => ({
      insert: quote(column.name),
      label: column.name,
      note: qualified ? column.type_name : `${table.name} · ${column.type_name}`,
      kind: "column" as const,
      shared: shared.has(column.name),
    }));

  if (at.qualifier) {
    // A qualifier nothing recognises still gets every column, rather than an
    // empty list. Half-typed aliases and tables loaded since the query was
    // written both land here, and offering nothing would look like the box
    // knows something it does not.
    const table = named.get(at.qualifier.toLowerCase()) ?? at.qualifier;
    const match = tables.find((t) => t.name.toLowerCase() === table.toLowerCase());
    candidates.push(...(match ? columnsOf(match, true) : tables.flatMap((t) => columnsOf(t, false))));
  } else if (at.naming) {
    candidates.push(...tables.map(asTable));
  } else {
    candidates.push(...tables.map(asTable));
    for (const table of tables) candidates.push(...columnsOf(table, false));
    for (const word of KEYWORDS) {
      candidates.push({
        insert: word.toUpperCase(),
        label: word.toUpperCase(),
        note: "",
        kind: "keyword",
      });
    }
    // With the open bracket, because a function is never wanted without one
    // and the caret lands where the argument goes. Lowercase, which is how
    // every one of these is written even by people who shout their keywords.
    for (const word of FUNCTIONS) {
      candidates.push({
        insert: `${word}(`,
        label: `${word}()`,
        note: "",
        kind: "fn",
      });
    }
  }

  const wanted = at.prefix.toLowerCase();
  const scored = candidates
    .map((item) => ({ item, rank: rank(item, wanted) }))
    .filter(({ rank }) => rank >= 0)
    .sort((a, b) => a.rank - b.rank || a.item.label.localeCompare(b.item.label));

  // Deduplicated on what would be inserted, not on the label: the same column
  // name in two tables is two rows worth showing, and the same keyword reached
  // twice is not.
  const seen = new Set<string>();
  const items: Suggestion[] = [];
  for (const { item } of scored) {
    const key = `${item.kind}:${item.note}:${item.insert}`;
    if (seen.has(key)) continue;
    seen.add(key);
    items.push(item);
    if (items.length === MAX_SUGGESTIONS) break;
  }
  return { at, items };
}

function asTable(table: DataTable): Suggestion {
  return {
    insert: quote(table.name),
    label: table.name,
    note:
      table.rows === null
        ? `${table.columns.length} columns`
        : `${table.rows.toLocaleString()} rows · ${table.columns.length} columns`,
    kind: "table",
  };
}

/** Lower is better. `-1` means it does not match at all. */
function rank(item: Suggestion, wanted: string): number {
  if (wanted === "") {
    // Nothing typed yet: what is in *this* workspace, and nothing that would
    // be the same list in every other one. A menu opened on an empty word that
    // led with ALL, AND, AS is a menu nobody reads twice.
    if (item.kind === "keyword" || item.kind === "fn") return -1;
    return item.kind === "table" ? 0 : 1;
  }
  const label = item.label.toLowerCase();
  // Identifiers above the vocabulary, because the vocabulary is the half
  // everybody already knows. Tables above columns: a table name is typed once
  // and its columns many times, so the one time it is typed is worth putting
  // first.
  const weight =
    item.kind === "table" ? 0 : item.kind === "column" ? 2 : item.kind === "fn" ? 4 : 6;
  if (label.startsWith(wanted)) return weight;
  if (label.includes(wanted)) return weight + 1;
  return -1;
}

/** Column names that more than one loaded table has — the join keys. */
function sharedColumns(tables: DataTable[]): Set<string> {
  const seen = new Map<string, number>();
  for (const table of tables) {
    // Per table, so a file with the same column twice does not look shared.
    for (const name of new Set(table.columns.map((c) => c.name))) {
      seen.set(name, (seen.get(name) ?? 0) + 1);
    }
  }
  return new Set([...seen].filter(([, n]) => n > 1).map(([name]) => name));
}

/**
 * A name as SQL has to spell it.
 *
 * Taurus turns identifier normalization off — a column reported as `Material`
 * is written as `Material` — so a name that is already a plain word needs
 * nothing. What does need quoting is a name a spreadsheet header produced:
 * spaces, punctuation, a leading digit, or a word the grammar has already
 * spoken for. Getting this wrong is the exact failure the case-sensitivity
 * note in the docs is about, which is why the box does it rather than leaving
 * it to be discovered.
 */
export function quote(name: string): string {
  const bare = /^[A-Za-z_][A-Za-z0-9_$]*$/.test(name);
  if (bare && !KEYWORDS.has(name.toLowerCase())) return name;
  return `"${name.replace(/"/g, '""')}"`;
}
