// Reads and writes the one version number that lives in four places.
//
// `package.json`, `[workspace.package]` in `Cargo.toml`, and `tauri.conf.json`
// each carry it independently, and the release tag is a fourth copy that no
// file records at all. Nothing forces them to agree, and the failure when they
// disagree is quiet in the worst way: `tauri-action` names the *release* from
// the tag but the *bundles* from `tauri.conf.json`, so tagging `v0.2.0` against
// an unbumped tree publishes a release called v0.2.0 whose assets are all named
// `Taurus_0.1.0_…`. Every download is mislabelled and the build that produced
// them was perfectly green.
//
//   node scripts/version.mjs check           # the files agree with each other
//   node scripts/version.mjs check v0.2.0    # …and with this tag
//   node scripts/version.mjs set 0.2.0       # write all of them, refresh the lock
//   node scripts/version.mjs valid 1.0.0-1   # whether this could be set; writes nothing
//
// `check` is what the release workflow runs before it builds anything, so a
// mismatch costs twenty seconds rather than three platforms' worth of compile.

import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

// Each entry finds the version in one file. The patterns are deliberately
// narrow rather than a parse-and-restringify: rewriting `package.json` through
// `JSON.stringify` would reformat the whole file, and `Cargo.toml` has a dozen
// other `version = "…"` lines under `[workspace.dependencies]` that a loose
// pattern would happily rewrite into nonsense.
const files = [
  {
    path: "package.json",
    // Top-level keys in this file are indented two spaces; a dependency named
    // "version" would be nested deeper and so cannot match.
    pattern: /^ {2}"version": "([^"]*)"/m,
    replace: (version) => `  "version": "${version}"`,
  },
  {
    path: "src-tauri/tauri.conf.json",
    pattern: /^ {2}"version": "([^"]*)"/m,
    replace: (version) => `  "version": "${version}"`,
  },
  {
    path: "Cargo.toml",
    // Anchored to the `[workspace.package]` header and stopped at the next
    // table, so this is the workspace's own version and never a dependency's.
    pattern: /(\[workspace\.package\][^[]*?\bversion = ")([^"]*)(")/,
    match: 2,
    replace: (version, m) => `${m[1]}${version}${m[3]}`,
  },
];

// Cargo and Tauri both want semver, and the Windows MSI bundler is narrower: a
// pre-release has to be one number no bigger than 65535, because WiX writes it as
// the fourth part of its version. `1.0.0-rc.1` is fine everywhere else and fails
// only on the Windows leg, hours into a release. Rejecting `v0.2.0` here is the
// point too: the tag carries the `v`, the files do not, and pasting one into the
// other is the obvious mistake to make.
const SEMVER = /^\d+\.\d+\.\d+(?:-(0|[1-9]\d*))?$/;
const accepted = (version) => {
  const m = SEMVER.exec(version);
  return m !== null && (m[1] === undefined || Number(m[1]) <= 65535);
};
const REFUSED = (version) =>
  `"${version}" is not a version Cargo, Tauri and the Windows installer will all accept\n` +
  `expected major.minor.patch, with no leading "v" and at most a numeric pre-release ` +
  `up to 65535: for example 0.2.0, or 1.0.0-1`;

function read(file) {
  const text = readFileSync(join(root, file.path), "utf8");
  const m = file.pattern.exec(text);
  if (!m) {
    // Worth naming the file and the expectation: the only way to get here is a
    // reformat that moved the version out from under the pattern, and a bare
    // "cannot read version" would send someone looking for a missing field
    // that is sitting right there.
    console.error(
      `could not find the version in ${file.path}\n` +
        `the pattern in scripts/version.mjs no longer matches it — if the file was ` +
        `reformatted, update the pattern to suit`,
    );
    process.exit(1);
  }
  return { text, match: m, version: m[file.match ?? 1] };
}

function check(tag) {
  const found = files.map((file) => ({ file, ...read(file) }));

  // Compared against the first file rather than against the tag, so that a
  // tagless `check` — what a dry run does — still catches a half-finished bump.
  const [first, ...rest] = found;
  const disagree = rest.filter((f) => f.version !== first.version);
  if (disagree.length > 0) {
    console.error(
      "the version is not the same in every file:\n" +
        found.map((f) => `  ${f.version}\t${f.file.path}`).join("\n") +
        `\n\nrun: node scripts/version.mjs set <version>`,
    );
    process.exit(1);
  }

  // A file edited by hand, or a version from before this rule, fails here rather
  // than on the Windows leg of the build this is about to start.
  if (!accepted(first.version)) {
    console.error(REFUSED(first.version));
    process.exit(1);
  }

  if (tag) {
    // The tag is the only copy with a `v`, by convention and by the workflow's
    // `tags: ['v*']` filter. Strip exactly that one leading character.
    const tagged = tag.startsWith("v") ? tag.slice(1) : tag;
    if (tagged !== first.version) {
      console.error(
        `the tag ${tag} does not match the version in the tree (${first.version})\n\n` +
          `nothing here can be fixed by rerunning. either the bump commit was not the ` +
          `one tagged, or the tag is wrong:\n` +
          `  node scripts/version.mjs set ${tagged}   # then commit, and move the tag\n` +
          `  git tag -d ${tag} && git push --delete origin ${tag}   # if the tag is the mistake`,
      );
      process.exit(1);
    }
    console.log(`${first.version} — tag ${tag}, and every file agrees`);
    return;
  }

  console.log(`${first.version} — every file agrees`);
}

function set(version) {
  if (!accepted(version)) {
    console.error(REFUSED(version));
    process.exit(1);
  }

  for (const file of files) {
    const { text, match } = read(file);
    const next =
      text.slice(0, match.index) +
      file.replace(version, match) +
      text.slice(match.index + match[0].length);
    writeFileSync(join(root, file.path), next);
  }

  // `Cargo.lock` pins each workspace member at its own version, so a bump that
  // stops at `Cargo.toml` leaves a lock that the next `cargo` command rewrites
  // — which is a dirty tree in the middle of a release build, and an outright
  // failure under `--locked`. `--workspace` touches only the members' entries,
  // so no third-party dependency moves as a side effect of a version bump.
  const cargo = spawnSync("cargo", ["update", "--workspace", "--offline"], {
    cwd: root,
    stdio: "inherit",
  });
  if (cargo.error || cargo.status !== 0) {
    console.error(
      `\nthe files are now ${version} but Cargo.lock was not refreshed` +
        (cargo.error ? `: ${cargo.error.message}` : "") +
        `\nrun \`cargo update --workspace\` before committing, or the first build ` +
        `after this will rewrite the lock on its own`,
    );
    process.exit(1);
  }

  console.log(
    `set to ${version} in ${files.map((f) => f.path).join(", ")}, and Cargo.lock`,
  );
}

const [command, argument] = process.argv.slice(2);

switch (command) {
  case "check":
    // The workflow passes the tag through an expression that is empty on a
    // dry run, so an empty argument means "no tag", not "a tag named nothing".
    check(argument || undefined);
    break;
  case "set":
    if (!argument) {
      console.error("usage: node scripts/version.mjs set <version>");
      process.exit(1);
    }
    set(argument);
    break;
  case "valid":
    if (!accepted(argument ?? "")) {
      console.error(REFUSED(argument ?? ""));
      process.exit(1);
    }
    console.log(`${argument} can be set`);
    break;
  default:
    console.error(
      "usage:\n" +
        "  node scripts/version.mjs check [tag]   verify the version is consistent\n" +
        "  node scripts/version.mjs set <version> write a new version everywhere\n" +
        "  node scripts/version.mjs valid <version> say whether it could be set, writing nothing",
    );
    process.exit(1);
}
