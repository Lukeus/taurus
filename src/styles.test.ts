import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

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
});
