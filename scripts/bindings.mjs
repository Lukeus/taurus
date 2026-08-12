// Regenerates the TypeScript types the frontend imports from `src/bindings`.
//
// This exists instead of a one-line npm script because the one-line version was
// `TS_RS_EXPORT_DIR="$PWD/src/bindings" cargo test ...`, which is shell syntax
// no Windows shell has: pnpm runs scripts through `cmd.exe` there, where an
// environment prefix is not a command and `$PWD` is not a variable. A Windows
// contributor's first `pnpm bindings` failed, and `pnpm build` then failed on
// every `Cannot find module "../bindings/…"` in turn — with nothing in either
// message pointing back at the script that did not run.
//
// The path has to be absolute. With `TS_RS_EXPORT_DIR` unset or relative, ts-rs
// resolves it per crate, so each crate writes into its own directory and the app
// imports something that was never written.

import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

// No shell: `cargo` is a real executable, so this needs no quoting, which is
// what makes a workspace path containing a space survive the trip.
const cargo = spawn(
  "cargo",
  ["test", "--workspace", "export_bindings", ...process.argv.slice(2)],
  {
    cwd: root,
    env: { ...process.env, TS_RS_EXPORT_DIR: join(root, "src", "bindings") },
    stdio: "inherit",
  },
);

cargo.on("error", (error) => {
  console.error(`could not run cargo: ${error.message}`);
  process.exit(1);
});

// A signal death is not an exit code; reporting it as 0 would let CI march on
// to a type check against bindings that were never finished.
cargo.on("close", (code, signal) => {
  process.exit(code ?? (signal ? 1 : 0));
});
