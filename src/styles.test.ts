import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { compile } from "tailwindcss";

/*
 * Comments are stripped first. Without that, `[^{]*` runs straight through a
 * comment block and the "selector" it captures is the comment plus the real
 * selector — which is how the first version of this test passed while the
 * duplicate it was written to catch was still in the file.
 */
const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  "",
);

/** The selector group of every top-level rule. */
function rules(): string[] {
  // A rule that starts at column 0 — anything indented is inside a block.
  return [...css.matchAll(/^([^\s@}][^{]*)\{/gm)].map(([, group]) =>
    group.trim().replace(/\s+/g, " "),
  );
}

describe("the stylesheet", () => {
  it("finds the rules it is meant to be checking", () => {
    // The guard on the guard. A regex that matched nothing would make the test
    // below pass forever, which is exactly how it failed the first time.
    const found = rules();
    expect(found.length).toBeGreaterThan(250);
    expect(found).toContain(".chip");
    expect(found).toContain(".thinking-body");
  });

  it("gives each bare class a rule of its own exactly once", () => {
    /*
     * The bug this exists for: `.chip` was the "N files changed" button in the
     * header, and a component added later declared `.chip` again for its own
     * tool pills. The second rule won, the header button silently became 10px
     * mono, and nothing failed — not the typecheck, not a test, not the build.
     *
     * Only rules whose whole selector is one bare class count. Sharing a class
     * across grouped rules is how this file layers on purpose — `.tool-output,
     * .proposal-body { }` narrowing a block those two share is fine. A second
     * `.chip { }` is a name taken twice.
     */
    const owned = rules().filter((r) => /^\.[a-z][a-z0-9-]*$/i.test(r));
    const counts = new Map<string, number>();
    for (const r of owned) counts.set(r, (counts.get(r) ?? 0) + 1);

    const taken = [...counts].filter(([, n]) => n > 1).map(([s]) => s);
    expect(taken).toEqual([]);
  });

  /** The body of the first top-level rule whose selector group contains `sel`. */
  function block(sel: string): string | undefined {
    for (const [, group, body] of css.matchAll(/^([^\s@}][^{]*)\{([^}]*)\}/gm)) {
      if (group.includes(sel)) return body;
    }
  }

  /** The `outline-offset` a rule declares, in px. */
  function offset(body: string | undefined): number | undefined {
    const px = body?.match(/outline-offset:\s*(-?[\d.]+)px/)?.[1];
    return px === undefined ? undefined : Number(px);
  }

  it("gives every kind the scanner can produce a colour", () => {
    // The scanner and the stylesheet are the two halves of one decision, and
    // they are in different files and different languages. A kind added to
    // `InkKind` with no rule here is invisible in the worst way: the run
    // renders, inherits whatever it is sitting in, and looks like a colour
    // somebody chose. Read out of the type rather than restated, so the list
    // cannot be half-updated.
    const source = readFileSync(new URL("./lib/ink.ts", import.meta.url), "utf8");
    const declared = source
      .slice(source.indexOf("export type InkKind ="), source.indexOf("export type Ink ="))
      .match(/"([a-z]+)"/g)
      ?.map((quoted) => quoted.replaceAll('"', ""));
    expect(declared?.length).toBeGreaterThan(0);
    for (const kind of declared ?? []) {
      expect(css, kind).toMatch(new RegExp(`\\.ink-${kind}[,\\s{]`));
    }
  });

  it("offsets the focus ring far enough off a control to read as a ring", () => {
    /*
     * At the 1px this shipped with, a ring in --accent sitting off a button
     * *filled* with --accent reads as a slightly thicker border rather than as
     * a state. The gap is what makes it a ring.
     */
    expect(offset(block(":focus-visible"))).toBeGreaterThanOrEqual(2);
  });

  it("turns the ring inward on the controls whose parent clips", () => {
    /*
     * The bug this exists for: an outline is painted outside the border box, so
     * a control filling a parent that carries `overflow: hidden` has its ring
     * clipped away completely rather than merely trimmed. `.run-head` — the
     * fold control on every tool-call card — was keyboard-focusable with
     * nothing on screen to say so, and rendered pixel-identical to an unfocused
     * card. Nothing caught it: the ring was declared, it just never reached a
     * pixel. A positive offset on any of these is that bug coming back.
     */
    for (const control of [".run-head", ".run-row-head", ".table-sort"]) {
      const body = block(`${control}:focus-visible`);
      expect(body, `${control} has no :focus-visible rule`).toBeDefined();
      expect(offset(body), `${control} would have its ring clipped away`).toBeLessThan(0);
    }
  });

  it("does not fade a filled button into an unreadable one", () => {
    /*
     * `opacity` composites fill and label together, so a disabled primary took
     * its own label down with it: 1.65:1 on dark, 1.29:1 on light, both
     * measured. The filled variant has to drop its fill instead.
     */
    const body = block("button.primary:disabled");
    expect(body, "no disabled rule for the filled variant").toBeDefined();
    expect(body).toMatch(/opacity:\s*1\b/);
    expect(body).toMatch(/color:/);
  });

  it("paints the window in the same two inks the stylesheet does", () => {
    /*
     * A webview holds its host's default ground — white — until the document
     * first paints, so the window itself has to be told the palette before
     * there is a stylesheet to read it from. That means these two values exist
     * twice, in `src-tauri`, and nothing in either file can derive the other.
     *
     * This is the thing that notices when the palette moves and the window
     * does not: the symptom otherwise is a one-frame flash of the *old* theme
     * on every launch, which nobody reports and nobody can screenshot.
     */
    const ink = (body: string | undefined) =>
      body?.match(/--lk-ink:\s*(#[0-9a-f]{6})/i)?.[1].toLowerCase();

    const dark = ink(block(":root"));
    const light = ink(block(':root[data-theme="light"]'));
    expect(dark, "the dark palette declares no --lk-ink").toBeDefined();
    expect(light, "the light palette declares no --lk-ink").toBeDefined();

    const rust = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
    const constant = (name: string) => {
      const hex = rust
        .match(new RegExp(`${name}: tauri::window::Color = [^;]+;`))?.[0]
        .match(/0x([0-9a-f]{2}), 0x([0-9a-f]{2}), 0x([0-9a-f]{2})/i);
      return hex && `#${hex[1]}${hex[2]}${hex[3]}`.toLowerCase();
    };
    expect(constant("DARK"), "the window's dark ground has drifted").toBe(dark);
    expect(constant("LIGHT"), "the window's light ground has drifted").toBe(light);

    // The config value is what the window is created with, before any of the
    // above runs; it is the dark one because that is what the app defaults to.
    const conf = JSON.parse(
      readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
    );
    expect(conf.app.windows[0].backgroundColor?.toLowerCase()).toBe(dark);
  });

  it("lets the browser skip a turn that is scrolled off", () => {
    /*
     * The transcript has no virtualiser: every turn ever rendered stays in the
     * document, so layout and paint cost grow with conversation length however
     * well the React memos hold. `content-visibility` is what buys that back,
     * and it is only worth having with an intrinsic size beside it — without
     * one every off-screen turn collapses to zero height and the scrollbar
     * jumps. The `auto` keyword is the part that stops a turn already measured
     * from being guessed at again.
     */
    const body = block(".turn");
    expect(body).toBeDefined();
    expect(body, "off-screen turns still cost a full layout").toMatch(
      /content-visibility:\s*auto/,
    );
    expect(body, "a skipped turn would collapse to nothing").toMatch(
      /contain-intrinsic-size:\s*auto\s+[\d.]+px/,
    );
  });

  it("hangs the whole centre column off one gutter", () => {
    /*
     * The bug this exists for: `.topbar`, `.pane-switch` and `.data-pane` had
     * drifted to 18px and 20px while the transcript, the composer, the plan
     * panel and a banner all sat at 32px. Nothing failed — each rule was
     * internally sensible — but the conversation's first sentence, the tab that
     * opens it and the box you type into started on three different lines, and
     * switching from Data to Conversation slid the whole column sideways.
     *
     * These are the rules that span the centre column edge to edge. Whatever
     * --gutter is, all of them have to say so by name.
     */
    for (const sel of [
      ".topbar",
      ".pane-switch",
      ".data-pane",
      ".transcript",
      ".composer",
      ".proposals",
      ".banner",
      ".turn-strip",
    ]) {
      const body = block(sel);
      expect(body, `${sel} has no rule`).toBeDefined();
      expect(body, `${sel} sets its own horizontal inset instead of --gutter`).toMatch(
        /(padding|margin)[^;]*var\(--gutter\)/,
      );
    }
  });

  it("keeps every layout distance on the ladder", () => {
    /*
     * The file grew up with every integer from 1 to 14 in use as a padding or a
     * gap, which meant an 11px inset and a 12px one beside it were never a
     * decision anyone made. The ladder in `:root` is the set of distances this
     * UI has; a raw value that is not one of them is either a mistake or
     * something derived from a glyph column, and the derived ones say so with
     * `calc()` rather than with a number nobody can trace.
     *
     * Sub-4px values are exempt: a 1px hairline offset or a 2px nudge under a
     * tracked label is optical, not structural.
     */
    const LADDER = new Set([2, 4, 6, 8, 10, 12, 16, 20, 24, 32]);
    const strays: string[] = [];
    for (const [, group, body] of css.matchAll(/^([^\s@}][^{]*)\{([^}]*)\}/gm)) {
      for (const [, decl] of body.matchAll(
        /((?:padding|margin|gap|row-gap|column-gap)[a-z-]*:[^;]+);/g,
      )) {
        // A derivation names what it is made of, and is checked by the sum.
        if (decl.includes("calc(")) continue;
        for (const [, n] of decl.matchAll(/\b(\d+)px/g)) {
          const px = Number(n);
          if (px >= 4 && !LADDER.has(px)) {
            strays.push(`${group.trim().replace(/\s+/g, " ").slice(0, 40)} — ${decl.trim()}`);
          }
        }
      }
    }
    expect(strays).toEqual([]);
  });

  it("keeps empty, loading and failed as three different states", () => {
    /*
     * All three used to share `.drawer-empty`, so a drawer whose read failed
     * looked exactly like one that succeeded and found nothing.
     */
    const colour = (sel: string) => block(sel)?.match(/color:\s*([^;]+);/)?.[1].trim();
    const states = [".drawer-empty", ".drawer-loading", ".drawer-error"].map((s) => {
      const c = colour(s);
      expect(c, `${s} declares no colour`).toBeDefined();
      return c;
    });
    expect(new Set(states).size, "two of the states paint the same colour").toBe(3);
    expect(colour(".drawer-error")).toContain("--danger");
  });
});

/*
 * The same file, read as a set of rules that a class name can now override.
 *
 * Half the styling decisions in this app are moving out of `styles.css` and
 * into the markup, which is a change of syntax and must not become a change of
 * system. Every constraint above that is really a constraint on the *design* —
 * the ladder, most of all — has to keep holding when the distance is written
 * `p-3` instead of `padding: 12px`, or the discipline lasts exactly as long as
 * the conversion does.
 */
/**
 * Tailwind, compiled the way the Vite plugin compiles it.
 *
 * The two `@import`s are inlined by hand because this runs without the plugin
 * that would resolve them, and `@source` is dropped because the candidates are
 * handed in directly rather than scanned for.
 */
async function utilities(candidates: string[]): Promise<string> {
  const source = readFileSync(new URL("./tailwind.css", import.meta.url), "utf8")
    .replace(/@import "tailwindcss\/(\w+)\.css" layer\((\w+)\);/g, (_, file, layer) =>
      `@layer ${layer} {\n${readFileSync(
        new URL(`../node_modules/tailwindcss/${file}.css`, import.meta.url),
        "utf8",
      )}\n}`,
    )
    .replace(/@source[^;]*;/g, "");
  const compiler = await compile(source, { base: "." });
  return compiler.build(candidates);
}

describe("the utility layer", () => {
  /**
   * Every `className` written in the app's markup, one entry per element.
   *
   * Per element rather than per name, because some of what this file checks is
   * about a control rather than about a vocabulary — a target big enough to hit
   * is a fact about one button, and the two class names that carry it mean
   * nothing apart.
   */
  function classLists(): { file: string; names: string[] }[] {
    const out: { file: string; names: string[] }[] = [];
    const root = new URL("./", import.meta.url);
    const walk = (dir: URL) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const at = new URL(entry.name + (entry.isDirectory() ? "/" : ""), dir);
        if (entry.isDirectory()) walk(at);
        else if (entry.name.endsWith(".tsx") && !entry.name.includes(".test.")) {
          const file = entry.name;
          /*
           * Both spellings, and they need separate patterns rather than one
           * character class that accepts either delimiter. A template literal
           * routinely holds a ternary — `${armed ? "a" : "b"}` — and a pattern
           * that stops at the first quote of either kind ends the class list in
           * the middle of the interpolation, then reads the leftovers as class
           * names. Backticks do not nest, so the template form can be greedy
           * about everything up to its closing one.
           *
           * What is inside `${…}` is dropped either way: a class assembled at
           * runtime is a value rather than a name, and not a decision this test
           * can read.
           */
          const source = readFileSync(at, "utf8");

          /*
           * The class lists a file names rather than writes twice.
           *
           * A component with eight identical links pulls the list out into a
           * `const`, which is the same reason the stylesheet had a `.rail-link`
           * rule — and it would make this test blind to every element wearing
           * one. So the constants are resolved first, and `${…}` inside them
           * with them, before any element is read.
           */
          const named = new Map<string, string>();
          for (const [, name, value] of source.matchAll(
            /^const ([A-Z][A-Z0-9_]*) =\s*[`"]([^`"]*)[`"];$/gm,
          )) {
            named.set(name, value);
          }
          // Two passes is enough for `A` built out of `B` built out of `C`,
          // and a cycle cannot outlive them.
          const fill = (text: string) =>
            text.replace(/\$\{([A-Z][A-Z0-9_]*)\}/g, (whole, name) => named.get(name) ?? whole);
          for (const [name, value] of named) named.set(name, fill(fill(value)));

          const written = [
            ...source.matchAll(/className=\{?"([^"]*)"/g),
            ...source.matchAll(/className=\{`([^`]*)`/g),
            // `className={LINK}`, which is a whole list under one name.
            ...source.matchAll(/className=\{([A-Z][A-Z0-9_]*)\}/g),
          ];
          for (const [, quoted] of written) {
            const names = fill(named.get(quoted) ?? quoted)
              .replace(/\$\{[^}]*\}/g, " ")
              .split(/\s+/)
              .filter(Boolean);
            if (names.length > 0) out.push({ file, names });
          }
        }
      }
    };
    walk(root);
    return out;
  }

  /** The same, flattened, for the checks that are about a vocabulary. */
  function classNames(): { file: string; name: string }[] {
    return classLists().flatMap(({ file, names }) => names.map((name) => ({ file, name })));
  }

  it("finds the class names it is meant to be checking", () => {
    // The guard on the guard, for the same reason as the one above it: a regex
    // that matched nothing would make every test below pass forever.
    const found = classNames();
    expect(found.length).toBeGreaterThan(500);
    expect(found.map((c) => c.name)).toContain("rail");
  });

  it("keeps every layout distance on the ladder, spelled as a utility", () => {
    /*
     * `--spacing` is pinned at 4px, so a Tailwind step is its number times four
     * and the ladder is the set below. It is enforced here rather than by
     * clipping the scale in `tailwind.css`, because one scale drives padding
     * and sizing both: taking `p-7` away would take `size-12` with it, and a
     * width was never on the ladder — a control is as wide as what it holds.
     *
     * The named steps are the ones with a reason to exist off the ladder: the
     * two gutters that align the centre column, and the run indent that is
     * derived from a glyph column and says so in `calc()`.
     */
    const LADDER = new Set([
      "0", "0.5", "1", "1.5", "2", "2.5", "3", "4", "5", "6", "8",
      "gutter", "gutter-rail", "run-indent", "px",
      /*
       * `auto` is not a distance. `margin-left: auto` is what puts a count at
       * the far end of a row, and the answer is whatever is left over — the
       * rule above never flagged it either, because it only ever looked at
       * numbers.
       */
      "auto",
    ]);
    /*
     * `p-3`, `px-5`, `mt-1`, `gap-2` — and the two-dash forms, `gap-x-3` and
     * `space-y-2`, which the first version of this pattern read as a step
     * called "x-3" and reported as a stray. That failure is the right one to
     * have: a spelling this cannot parse is a distance nobody is checking, so
     * it fails rather than passing quietly. The guard below keeps it honest.
     */
    const SPACING = /^-?(p|m|gap|space)(?:-?[xytrbles])?-(.+)$/;

    const strays = classNames()
      .map(({ file, name }) => ({ file, name, step: name.replace(/^\w+:/, "").match(SPACING) }))
      .filter(({ step }) => {
        if (!step) return false;
        const value = step[2];
        // A sub-4px offset is optical rather than structural — a hairline nudge
        // under a tracked label — and is exempt here as it is in the CSS above.
        if (/^\[[0-3](?:\.\d+)?px\]$/.test(value)) return false;
        return !LADDER.has(value);
      })
      .map(({ file, name }) => `${file} — ${name}`);

    expect(strays).toEqual([]);

    // The guard on the guard, for the third time in this file: a pattern that
    // matched nothing would make the check above pass forever. The floor is
    // well under what the tree carries — its job is to catch zero, not to be a
    // count that has to be edited every time a component converts.
    const measured = classNames().filter(({ name }) => SPACING.test(name.replace(/^\w+:/, "")));
    expect(measured.length).toBeGreaterThan(40);
  });

  it("leaves no shared class name without a rule or a utility behind it", async () => {
    /*
     * The bug this exists for, and it is the one this migration produces most
     * easily: `.settings-problem` looked like Settings' own rule and was
     * deleted with the rest of them. Eleven other panels were still wearing it
     * — the MCP drawer, the agent editor, every drawer that can fail — and each
     * lost the colour and the size that said something had gone wrong. Nothing
     * failed. A thousand tests passed. The only witness was a screenshot that
     * happened to catch a drawer showing an error.
     *
     * So a class the markup writes has to be one of two things: a rule in the
     * stylesheet, or a utility Tailwind actually generates. A name that is
     * neither renders as nothing at all.
     *
     * Scoped to names used by more than one file, because a converted component
     * legitimately keeps a ruleless name of its own — `rail-row` and
     * `settings-provider` are what a test finds the element by, with every
     * declaration in the utilities beside them. One file wearing a name it
     * styles itself is that. Two files wearing it is a shape they agreed on,
     * and a shape they agreed on is the thing that must not vanish.
     */
    const IDENTITY = new Set([
      /*
       * The docked changes pane, worn by the drawer and by the placeholder
       * App shows while it loads. It has never had a rule — it marks the
       * element for a test that checks the panel is docked rather than
       * floating, and the width it is docked at comes from `.drawer`.
       */
      "changes-pane",
    ]);

    /*
     * Read out of the selector groups rather than the whole file, so a property
     * value like `font-size: 0.92em` is not mistaken for a class, and out of
     * the whole group rather than its start, so `.dot.warn` declares both
     * `dot` and `warn`.
     */
    const declared = new Set<string>();
    for (const [, group] of css.matchAll(/^([^\s@}][^{]*)\{/gm)) {
      for (const [, name] of group.matchAll(/\.([a-z][a-z0-9-]*)/gi)) declared.add(name);
    }

    const files = new Map<string, Set<string>>();
    for (const { file, names } of classLists()) {
      for (const name of names) {
        // A name assembled at runtime — `ink-${kind}` leaves `ink-` behind, and
        // a template holding a ternary leaves fragments of it. Neither is a
        // class anybody wrote.
        if (/[${}?]/.test(name) || name.endsWith("-")) continue;
        if (!files.has(name)) files.set(name, new Set());
        files.get(name)!.add(file);
      }
    }

    const unknown = [...files.keys()].filter(
      (name) => !declared.has(name) && !IDENTITY.has(name),
    );

    // Tailwind escapes everything outside `[A-Za-z0-9_-]` with a backslash, so
    // `bg-danger/10` is emitted as `.bg-danger\/10`. Asking for the escaped
    // spelling is asking whether a rule was generated for it at all.
    const generated = await utilities(unknown);
    const strays = unknown
      .filter(
        (name) =>
          !generated.includes("." + name.replace(/[^a-zA-Z0-9_-]/g, (c) => "\\" + c)),
      )
      .filter((name) => files.get(name)!.size > 1)
      .map((name) => `${name} — ${[...files.get(name)!].join(", ")}`);

    expect(strays).toEqual([]);
  });

  it("gives the one destructive control in the rail a pointer-sized target", () => {
    /*
     * Measured at 23×23 in the running app — the smallest thing in it, and the
     * only irreversible one, four pixels from the row that merely selects.
     *
     * Read out of the markup rather than out of the stylesheet, because that is
     * where the two numbers now are. The guarantee did not move with them: this
     * is the same 28px floor the CSS used to declare, checked in the file that
     * declares it, so a class list edited down to what "looks like enough"
     * still fails.
     */
    const arm = classLists().filter(({ names }) => names.includes("rail-delete"));
    expect(arm.length, "no rail-delete button found").toBeGreaterThan(0);
    for (const { file, names } of arm) {
      for (const axis of ["min-w", "min-h"]) {
        const step = names.find((n) => n.startsWith(`${axis}-`))?.slice(axis.length + 1);
        // `--spacing` is 4px, so a step is worth four of them.
        const px = Number(step) * 4;
        expect(px, `${file} — rail-delete declares no ${axis}`).toBeGreaterThanOrEqual(28);
      }
    }
  });

  it("gives no class name away to a utility of the same spelling", () => {
    /*
     * The bug this exists for, and it is the worst kind this migration can
     * produce: `.table-row { display: grid }` is a name Tailwind also generates
     * a utility for, because `table-row` is a CSS `display` value. A utility
     * outranks the stylesheet's layer by design, so every table in the app
     * quietly stopped being a grid and collapsed into a run of words — the
     * typecheck passed, a thousand tests passed, and the only thing that knew
     * was a screenshot.
     *
     * Checked by spelling rather than by compiling Tailwind, because the test
     * has to name what it is protecting: these are the CSS keywords a class
     * name can be, and a name that *is* one of them is a name the utility layer
     * has a prior claim on. Adding to the list is how a future collision gets
     * fixed; the fix is always to rename this side, since the other one cannot
     * be told not to answer.
     */
    const CLAIMED = new Set([
      // `display`
      "block", "inline", "flex", "grid", "contents", "hidden", "table",
      "table-row", "table-cell", "table-caption", "table-column", "flow-root",
      "inline-flex", "inline-grid", "inline-block", "inline-table", "list-item",
      // `position`
      "static", "fixed", "absolute", "relative", "sticky",
      // and the handful of one-word utilities that are not a keyword at all
      "container", "isolate", "truncate", "italic", "underline", "invisible",
      "collapse", "visible", "sr-only", "antialiased", "capitalize", "uppercase",
      "lowercase", "ordinal", "border", "outline", "ring", "shadow",
    ]);

    const sheet = css.matchAll(/(?:^|[\s,>+~])\.([a-z][a-z0-9-]*)/gim);
    const taken = [...new Set([...sheet].map(([, name]) => name))]
      .filter((name) => CLAIMED.has(name))
      .sort();

    expect(taken).toEqual([]);
  });

  it("writes colour in roles rather than in a second palette", () => {
    /*
     * `tailwind.css` clears Tailwind's own colours, so `bg-gray-800` compiles to
     * nothing and an element wearing it is transparent — which is a bug that
     * looks like a missing background rather than like a class name nobody
     * removed. This says which it is.
     *
     * The same for the sizes and radii the default theme would have supplied:
     * `text-sm` and `rounded-2xl` are names this app does not have, and reading
     * as "the text is the wrong size" is the slowest possible way to find out.
     */
    const GONE =
      /^(bg|text|border|ring|outline|fill|stroke|shadow|divide|from|via|to|accent|caret|decoration|placeholder)-(inherit|current|transparent|black|white|slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)(-\d{1,3})?$/;
    const SIZES = /^(text|rounded)-(xs|sm|base|md|lg|xl|\dxl)$/;

    const strays = classNames()
      .filter(({ name }) => {
        const bare = name.replace(/^\w+:/, "");
        // The app's own `rounded-base`/`-sm`/`-md`/`-lg`/`-xl` share their
        // spelling with the names Tailwind ships; only the type scale collides
        // in a way that matters, and it is numeric here.
        if (/^rounded-(sm|base|md|lg|xl)$/.test(bare)) return false;
        if (/^(text|bg|border)-(transparent|current)$/.test(bare)) return false;
        return GONE.test(bare) || SIZES.test(bare);
      })
      .map(({ file, name }) => `${file} — ${name}`);

    expect(strays).toEqual([]);
  });
});
