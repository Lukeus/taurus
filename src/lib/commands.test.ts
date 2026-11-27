import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { COMMANDS } from "./commands";

/*
 * The menu bar is built in Rust and run in TypeScript, and the only thing
 * joining the two is a string. An id added to one and not the other is a menu
 * item that does nothing, or a command nothing can send — and neither fails a
 * build. So this reads `menu.rs` and holds the two lists to each other.
 *
 * In the node environment, where `import.meta.url` is a file URL; what needs a
 * DOM is in `commands.mount.test.ts`.
 */
const rust = readFileSync(new URL("../../src-tauri/src/menu.rs", import.meta.url), "utf8");

describe("the menu bar's commands", () => {
  it("are the same ids on both sides", () => {
    const tuples = [...rust.matchAll(/\("([a-z]+)", "[^"]+", (?:Some|None)/g)].map(([, id]) => id);
    const texts = [...rust.matchAll(/\.text\("([a-z]+)"/g)].map(([, id]) => id);
    expect(tuples.length).toBeGreaterThan(5);
    expect(new Set([...tuples, ...texts])).toEqual(new Set(COMMANDS));
  });
});
