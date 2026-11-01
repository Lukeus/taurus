// The versions a release can be cut at. Run as the script runs, through `valid`,
// which is the check `set` makes and writes nothing.
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const script = join(dirname(fileURLToPath(import.meta.url)), "version.mjs");
const valid = (version) => spawnSync("node", [script, "valid", version]).status === 0;

describe("which versions can be set", () => {
  it("takes what the Windows installer can number", () => {
    for (const version of ["0.3.0", "1.0.0-0", "1.0.0-1", "1.0.0-65535"]) {
      expect(valid(version), version).toBe(true);
    }
  });

  it("refuses a pre-release the Windows installer would fail on", () => {
    for (const version of ["1.0.0-rc.1", "1.0.0-rc", "1.0.0-65536", "1.0.0-01", "1.0.0-1.2"]) {
      expect(valid(version), version).toBe(false);
    }
  });

  it("refuses what isn't a version at all", () => {
    for (const version of ["v0.2.0", "0.2", ""]) {
      expect(valid(version), version).toBe(false);
    }
  });
});
