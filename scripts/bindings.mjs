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
//
// # Two Rust types with one name
//
// ts-rs writes each type to `<Name>.ts`, and the name is global across the
// whole workspace while the Rust names it comes from are scoped per crate. So
// two crates can each define a `ColumnKind`, both write to `ColumnKind.ts`, and
// whichever ran last wins — leaving the other crate's payload described by the
// wrong type, with everything still compiling. That happened, and nothing
// caught it; `taurus_data::ColumnKind` silently replaced `taurus_tools`\''s.
//
// It is invisible in the directory listing, because the collision *is* one
// file. It is visible in the arithmetic: ts-rs generates exactly one
// `export_bindings_<name>` test per exported type and each writes exactly one
// file, so a run with more tests than files wrote something twice. `collided`
// below does that subtraction and refuses the run.
//
// Counting is the exact signal; naming the culprit is not, because the test
// name is the *Rust* type name and the collision is between *TypeScript* names.
// Two crates may each define a `SaveTarget` and be perfectly fine, because one
// of them is renamed on export — which is also the fix here:
// `#[ts(export, rename = "...")]` on one of the two.

import { spawn } from "node:child_process";
import { readdirSync, renameSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const dest = join(root, "src", "bindings");
const staging = join(root, "target", "ts-bindings");

rmSync(staging, { recursive: true, force: true });

// No shell: `cargo` is a real executable, so this needs no quoting, which is
// what makes a workspace path containing a space survive the trip.
// Piped rather than inherited so the test names can be read back — see the
// note on colliding names above. Everything is re-emitted as it arrives, so
// this still reads like `stdio: "inherit"` from the terminal.
const cargo = spawn(
  "cargo",
  ["test", "--workspace", "export_bindings", ...process.argv.slice(2)],
  {
    cwd: root,
    env: { ...process.env, TS_RS_EXPORT_DIR: staging },
    stdio: ["inherit", "pipe", "inherit"],
  },
);

let output = "";
cargo.stdout.on("data", (chunk) => {
  output += chunk;
  process.stdout.write(chunk);
});

/**
 * Every type that was exported, as `module::path` and Rust type name.
 */
function exported(text) {
  return [...text.matchAll(/^test (\S*?)export_bindings_(\w+) \.\.\. ok$/gm)].map(
    ([, path, name]) => ({ path, name }),
  );
}

/**
 * Whether two exports wrote the same file, and the shortlist to check.
 *
 * The shortlist is the Rust type names that appear in more than one crate. It
 * is where a collision almost always is, and it is a hint rather than the
 * answer — see the note at the top of this file.
 */
function collided(text, written) {
  const types = exported(text);
  if (types.length === 0 || types.length === written) return null;

  const byName = new Map();
  for (const { path, name } of types) {
    byName.set(name, [...(byName.get(name) ?? []), `${path}export_bindings_${name}`]);
  }
  return {
    types: types.length,
    written,
    shortlist: [...byName.entries()].filter(([, tests]) => tests.length > 1),
  };
}

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

  const clash = collided(output, readdirSync(staging).length);
  if (clash) {
    rmSync(staging, { recursive: true, force: true });
    console.error(
      `\n${clash.types} types were exported but only ${clash.written} files were written, ` +
        "so two of them share a TypeScript name and one overwrote the other.\n" +
        (clash.shortlist.length > 0
          ? "\nRust type names defined in more than one crate — start here:\n" +
            clash.shortlist
              .map(([name, tests]) => `  ${name}  <-  ${tests.join(", ")}`)
              .join("\n") +
            "\n"
          : "") +
        '\nRename one of the pair for export with #[ts(export, rename = "...")].\n',
    );
    process.exit(1);
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
