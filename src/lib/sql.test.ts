import { describe, expect, it } from "vitest";

import type { DataTable } from "./api";
import { aliases, ink, quote, spot, suggest } from "./sql";

const table = (name: string, columns: [string, string][]): DataTable => ({
  name,
  path: `data/${name}.csv`,
  rows: null,
  columns: columns.map(([column, type_name]) => ({
    name: column,
    kind: type_name === "Int64" || type_name === "Float64" ? "number" : "text",
    type_name,
    nullable: true,
  })),
});

const EVENTS = table("events", [
  ["user_id", "Int64"],
  ["event", "Utf8"],
  ["Price_Per_Unit", "Float64"],
]);

const USERS = table("users", [
  ["user_id", "Int64"],
  ["country", "Utf8"],
]);

describe("painting SQL", () => {
  /*
   * The property everything else rests on. The painted `<pre>` sits under a
   * transparent textarea, so a scanner that dropped a space or doubled a
   * character would slide the colour off the text and keep sliding for the
   * rest of the query. Colours can be wrong and be merely ugly; this cannot be
   * wrong at all.
   */
  it("gives back exactly what it was handed, character for character", () => {
    const tricky = [
      "",
      "SELECT 1",
      "select * from events where event = 'it''s fine'",
      "-- a comment\nSELECT 1",
      "/* block\n   comment */ SELECT 1",
      'SELECT "Price Per Unit" FROM t',
      "SELECT count(*) AS n, 1.5, 1_000 FROM t\n\n  WHERE x <> 2",
      "SELECT 'unterminated",
      'SELECT "unterminated',
      "SELECT ünïcode FROM t -- ünïcode\n",
      "\n\n\t\t  \n",
    ];
    for (const sql of tricky) {
      expect(ink(sql).map((run) => run.text).join("")).toBe(sql);
    }
  });

  it("does not tint a keyword that is inside a string", () => {
    // The failure the old comment in the stylesheet predicted for a regex, and
    // the reason this is a scanner instead.
    const painted = ink("WHERE label = 'select from where'");
    const string = painted.find((run) => run.kind === "string");
    expect(string?.text).toBe("'select from where'");
    expect(painted.filter((run) => run.kind === "keyword").map((r) => r.text)).toEqual([
      "WHERE",
    ]);
  });

  it("runs a line comment to the newline and no further", () => {
    const painted = ink("SELECT 1 -- not SELECT\nSELECT 2");
    expect(painted.filter((r) => r.kind === "comment").map((r) => r.text)).toEqual([
      "-- not SELECT",
    ]);
    expect(painted.filter((r) => r.kind === "keyword").map((r) => r.text)).toEqual([
      "SELECT",
      "SELECT",
    ]);
  });

  it("tints a function only where it is being called", () => {
    // `count` is a perfectly ordinary column name, and colouring the column
    // would be the box saying something false about the file.
    expect(ink("count(*)").find((r) => r.kind === "fn")?.text).toBe("count");
    expect(ink("SELECT count FROM t").some((r) => r.kind === "fn")).toBe(false);
  });

  it("draws a quoted identifier as an identifier and a string as a string", () => {
    // The distinction that matters in this dialect: `"Material"` is the column
    // and `'Material'` is the word.
    const painted = ink(`SELECT "Material", 'Material'`);
    expect(painted.find((r) => r.kind === "quoted")?.text).toBe('"Material"');
    expect(painted.find((r) => r.kind === "string")?.text).toBe("'Material'");
  });
});

describe("where the caret is", () => {
  it("reads a half-typed word and where it starts", () => {
    const at = spot("SELECT use", 10);
    expect(at.prefix).toBe("use");
    expect(at.from).toBe(7);
    expect(at.qualifier).toBeNull();
  });

  it("knows a table name goes here, with or without a letter typed", () => {
    expect(spot("SELECT * FROM ", 14).naming).toBe(true);
    expect(spot("SELECT * FROM ev", 16).naming).toBe(true);
    expect(spot("SELECT * FROM events JOIN ", 26).naming).toBe(true);
    // Past the table name, a keyword goes here and not another table.
    expect(spot("SELECT * FROM events WHERE ", 27).naming).toBe(false);
  });

  it("reads a qualifier off the dot before the caret", () => {
    expect(spot("SELECT e.", 9)).toMatchObject({ qualifier: "e", prefix: "" });
    expect(spot("SELECT e.us", 11)).toMatchObject({ qualifier: "e", prefix: "us" });
  });

  it("stops treating it as qualified once the word has moved on", () => {
    // `e. ` with a space is past the column; offering columns there would put
    // them where a keyword goes.
    expect(spot("SELECT e.id ", 12).qualifier).toBeNull();
  });
});

describe("what a name in a FROM clause is called afterwards", () => {
  it("takes an alias with or without AS, and the table's own name too", () => {
    const named = aliases("SELECT * FROM events e JOIN users AS u ON e.id = u.id");
    expect(named.get("e")).toBe("events");
    expect(named.get("u")).toBe("users");
    expect(named.get("events")).toBe("events");
    expect(named.get("users")).toBe("users");
  });

  it("does not mistake the next keyword for an alias", () => {
    // `WHERE` sits exactly where an alias would, which is the one case worth
    // guarding in something this approximate.
    const named = aliases("SELECT * FROM events WHERE x = 1");
    expect(named.get("where")).toBeUndefined();
    expect(named.get("events")).toBe("events");
  });
});

describe("what the box offers", () => {
  const tables = [EVENTS, USERS];

  it("offers only tables where a table name goes", () => {
    const { items } = suggest("SELECT * FROM ", 14, tables);
    expect(items.map((i) => i.label)).toEqual(["events", "users"]);
  });

  it("offers one table's columns after its alias and a dot", () => {
    const { items } = suggest(
      "SELECT u. FROM events e JOIN users u ON e.user_id = u.user_id",
      9,
      tables,
    );
    expect(items.map((i) => i.label)).toEqual(["country", "user_id"]);
  });

  it("falls back to every column when the qualifier means nothing yet", () => {
    // Half-typed aliases land here. An empty list would look like the box knew
    // something it does not.
    const { items } = suggest("SELECT zz.", 10, tables);
    expect(items.map((i) => i.label).sort()).toContain("country");
    expect(items.map((i) => i.label).sort()).toContain("event");
  });

  /*
   * The feature this was built for. Two files that share a column are two
   * files that can be joined on it, and the completion list is where that is
   * cheapest to notice — while writing the join, rather than by reading two
   * profiles side by side.
   */
  it("marks a column more than one table has", () => {
    const { items } = suggest("SELECT user", 11, tables);
    const shared = items.filter((i) => i.label === "user_id");
    expect(shared).toHaveLength(2);
    expect(shared.every((i) => i.shared)).toBe(true);
    expect(shared.map((i) => i.note)).toEqual([
      "events · Int64",
      "users · Int64",
    ]);

    const alone = suggest("SELECT coun", 11, tables).items.find(
      (i) => i.label === "country",
    );
    expect(alone?.shared).toBe(false);
  });

  it("puts identifiers above keywords, because keywords are the known half", () => {
    const { items } = suggest("SELECT ev", 9, tables);
    expect(items[0].label).toBe("events");
    // `event` the column beats `ELSE`/`END`/`EXISTS`, none of which anybody
    // opened a completion list to be reminded of.
    expect(items[1].label).toBe("event");
  });

  it("offers no keywords at all until something has been typed", () => {
    // A list opened on an empty word that led with ALL, AND, AS is a list
    // nobody reads twice.
    const { items } = suggest("SELECT ", 7, tables);
    expect(items.every((i) => i.kind !== "keyword")).toBe(true);
  });

  it("inserts a spreadsheet's column name in a form that parses", () => {
    // Taurus turns identifier normalization off, so `Price_Per_Unit` needs
    // nothing. A header with a space in it does, and getting that wrong is the
    // exact failure the case-sensitivity note in the docs is about.
    const { items } = suggest("SELECT Price", 12, tables);
    expect(items[0].insert).toBe("Price_Per_Unit");

    const spaced = suggest("SELECT Pri", 10, [
      table("t", [["Price Per Unit", "Float64"]]),
    ]);
    expect(spaced.items[0].insert).toBe('"Price Per Unit"');
    // Shown as the profile spells it, inserted as SQL needs it.
    expect(spaced.items[0].label).toBe("Price Per Unit");
  });
});

describe("quoting a name", () => {
  it("leaves a plain word alone, whatever its case", () => {
    expect(quote("material")).toBe("material");
    expect(quote("Material")).toBe("Material");
    expect(quote("_x9")).toBe("_x9");
  });

  it("quotes anything a bare identifier cannot be", () => {
    expect(quote("Price Per Unit")).toBe('"Price Per Unit"');
    expect(quote("2024")).toBe('"2024"');
    expect(quote("total-cost")).toBe('"total-cost"');
  });

  it("quotes a name the grammar has already spoken for", () => {
    expect(quote("order")).toBe('"order"');
    expect(quote("select")).toBe('"select"');
  });

  it("doubles a quote inside the name rather than ending the identifier", () => {
    expect(quote('od"d')).toBe('"od""d"');
  });
});
