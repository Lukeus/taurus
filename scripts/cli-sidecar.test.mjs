// `tauri.bundle-cli.conf.json` replaces the app's `beforeBuildCommand` to add
// the CLI build, because a merged config can only replace a string, not append
// to it. So it restates the original, and this keeps the two from drifting: a
// step added to `tauri.conf.json` and not here would be silently skipped by
// every release build.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const tauri = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri");
const read = (name) => JSON.parse(readFileSync(join(tauri, name), "utf8"));

describe("the bundle config that ships the CLI", () => {
  it("runs everything the app's build runs, then builds the CLI", () => {
    const base = read("tauri.conf.json").build.beforeBuildCommand;
    const bundled = read("tauri.bundle-cli.conf.json").build.beforeBuildCommand;
    expect(bundled).toBe(`${base} && node scripts/cli-sidecar.mjs`);
  });

  it("names the sidecar the script writes", () => {
    expect(read("tauri.bundle-cli.conf.json").bundle.externalBin).toEqual(["binaries/taurus-cli"]);
  });
});
