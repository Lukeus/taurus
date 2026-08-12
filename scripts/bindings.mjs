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
//
// Generation is authoritative: what comes out is the whole set of types Rust
// exports, and nothing else. ts-rs only ever writes, so a type deleted or
// renamed on the Rust side would otherwise leave its `.ts` file behind, still
// importable and still type-checking, describing a payload that no longer
// crosses the boundary. That is the failure worth engineering against here —
// stale bindings do not announce themselves, they just quietly stay correct
// enough to compile.
//
// The staging directory is what keeps that from costing anything on a failed
// run: a build that does not compile leaves the working tree exactly as it was,
// rather than deleting every type in the app and burying the Rust error under
// editor noise. It lives under `target/` so the swap is a rename within one
// filesystem.

import { spawn } from "node:child_process";
import { renameSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const dest = join(root, "src", "bindings");
const staging = join(root, "target", "ts-bindings");

rmSync(staging, { recursive: true, force: true });

// No shell: `cargo` is a real executable, so this needs no quoting, which is
// what makes a workspace path containing a space survive the trip.
const cargo = spawn(
  "cargo",
  ["test", "--workspace", "export_bindings", ...process.argv.slice(2)],
  {
    cwd: root,
    env: { ...process.env, TS_RS_EXPORT_DIR: staging },
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
  const status = code ?? (signal ? 1 : 0);
  if (status !== 0) {
    rmSync(staging, { recursive: true, force: true });
    process.exit(status);
  }

  try {
    rmSync(dest, { recursive: true, force: true });
    renameSync(staging, dest);
  } catch (error) {
    // Worth naming rather than letting a raw EPERM stand: on Windows this is
    // usually a running dev server or an editor holding a file open, and the
    // generated types are sitting in `staging` intact either way.
    console.error(
      `generated the bindings but could not move them into ${dest}: ${error.message}\n` +
        `they are in ${staging}; close anything holding those files open and rerun`,
    );
    process.exit(1);
  }
});
