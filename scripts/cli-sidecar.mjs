// Builds the `taurus` CLI and puts it where the bundler takes an `externalBin`
// from, so the app ships the CLI that matches it.
//
// # Why
//
// The CLI and the app are separate binaries that share `~/.taurus`. A release
// used to carry only the app, so the CLI was whatever `cargo install` had built
// whenever someone last ran it, reading files a newer app had written. Bundled,
// every app update carries the matching CLI, and Settings › About can put it on
// the PATH. See `src-tauri/src/cli_install.rs`.
//
// # When it runs
//
// Only for a bundle: `tauri.bundle-cli.conf.json` makes it part of the
// `beforeBuildCommand` and declares the `externalBin`, and a release build
// merges that file with `--config`. It isn't in `tauri.conf.json` because
// `tauri-build` checks an `externalBin` exists at *compile* time, so every
// `cargo build`, `cargo test` and `pnpm tauri dev` would need the CLI built
// first, and would fail on a fresh clone with nothing saying why.
//
// # Where it lands
//
// `src-tauri/binaries/taurus-cli-<target triple>[.exe]`, the name the bundler
// looks for. `taurus-cli`, not `taurus`: `tauri-build` copies a sidecar into
// cargo's output folder, where `taurus` is the CLI's own build, and on Windows
// the app is `Taurus.exe`, the same name to a case-insensitive filesystem.
//
// The triple is `TAURI_ENV_TARGET_TRIPLE`, which `tauri build` sets for this
// hook. A universal macOS build is the exception: the hook is told the host's
// triple even then, so the release job says so with `TAURUS_CLI_UNIVERSAL=1`,
// and this builds both architectures and joins them with `lipo`. The bundler
// wants the joined one, and each architecture's half of the app build checks
// for its own, so all three are written.

import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "src-tauri", "binaries");
const targetDir = process.env.CARGO_TARGET_DIR || join(root, "target");
const debug = process.env.TAURI_ENV_DEBUG === "true";
const profile = debug ? "debug" : "release";

function run(command, args) {
  console.log(`> ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit" });
  if (result.error) throw new Error(`couldn't run ${command}: ${result.error.message}`);
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function hostTriple() {
  const result = spawnSync("rustc", ["-vV"], { encoding: "utf8" });
  const host = result.stdout?.match(/^host: (\S+)$/m)?.[1];
  if (!host) throw new Error("couldn't read the host triple from `rustc -vV`");
  return host;
}

const host = hostTriple();

// Built for `triple`, and the path cargo put it at. The host's own triple is
// built without `--target`, into the same directory the app's build uses, so
// the two share every dependency they have in common rather than compiling
// the workspace twice.
function build(triple) {
  const args = ["build", "-p", "taurus-cli", "--locked"];
  if (!debug) args.push("--release");
  if (triple !== host) args.push("--target", triple);
  run("cargo", args);
  const dir = triple === host ? join(targetDir, profile) : join(targetDir, triple, profile);
  return join(dir, triple.includes("windows") ? "taurus.exe" : "taurus");
}

const place = (from, triple) => {
  const to = join(out, `taurus-cli-${triple}${triple.includes("windows") ? ".exe" : ""}`);
  copyFileSync(from, to);
  console.log(`placed ${to}`);
  return to;
};

mkdirSync(out, { recursive: true });

if (process.env.TAURUS_CLI_UNIVERSAL === "1") {
  const halves = ["aarch64-apple-darwin", "x86_64-apple-darwin"].map((triple) =>
    place(build(triple), triple),
  );
  const universal = join(out, "taurus-cli-universal-apple-darwin");
  rmSync(universal, { force: true });
  run("lipo", ["-create", "-output", universal, ...halves]);
  console.log(`placed ${universal}`);
} else {
  const triple = process.env.TAURI_ENV_TARGET_TRIPLE || host;
  place(build(triple), triple);
}
