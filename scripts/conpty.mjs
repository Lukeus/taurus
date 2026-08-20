// Fetches the ConPTY runtime that the Windows bundle sideloads.
//
// # Why there is a download in the build at all
//
// `run_command` with `pty: true` runs under a ConPTY, and a ConPTY is a real
// console host process. Taken from `kernel32`, that host is the system's own
// `conhost.exe`, which shows a window when the process asking for it has none —
// and a release build of Taurus has none, so every pty command opened a console
// window for as long as it ran.
//
// `portable-pty` looks for a `conpty.dll` beside the executable before falling
// back to `kernel32`, so shipping Microsoft's redistributable ConPTY next to
// `Taurus.exe` replaces the system host with `OpenConsole.exe`, which is
// headless. That is the whole fix: two files in the right places.
//
// # Why fetched rather than committed
//
// It is 2.2 MB of binaries that are not ours, that we do not build, and that
// change on Microsoft's schedule rather than this project's. Committing them
// puts a new copy in git history on every bump. The trade is a network fetch in
// the Windows build, paid once per machine — and pinned by hash, so a fetch that
// returns something other than the reviewed bytes fails the build rather than
// shipping.
//
// Off Windows this does nothing at all: there is no ConPTY to sideload, and the
// build calls it unconditionally so that nobody on Windows has to know it needs
// calling.

import { createHash } from "node:crypto";
import { inflateRawSync } from "node:zlib";
import { mkdirSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const dest = join(root, "src-tauri", "conpty");

// Pinned exactly. A floating version would mean the bytes in a release were
// chosen by whatever was newest on the day it was built, which is not a thing
// anyone could review after the fact.
const VERSION = "1.24.260710001";
const PACKAGE = "microsoft.windows.console.conpty";
const NUPKG_SHA256 =
  "175640566a3b59c4b132070ee96c2c77e5ab7edd2e92732a5eb3610bbf63d90e";

// Where each file sits inside the package, where it has to land beside the
// executable, and the hash it must have.
//
// The layout is Microsoft's, taken from the package's own MSBuild targets
// rather than guessed: `conpty.dll` sits beside the binary, and each
// `OpenConsole.exe` sits in a directory named for its architecture, because
// that is where `conpty.dll` looks for it.
//
// An x64 build ships the ARM64 host as well as its own. An x64 binary runs on
// ARM64 Windows under emulation, and there the console host has to be native —
// so the second one is not redundancy, it is the only thing standing between an
// ARM64 user and a pty that cannot start.
const FILES = [
  {
    from: "runtimes/win-x64/native/conpty.dll",
    to: "conpty.dll",
    sha256: "39fba2713e2495117b1591ae8c32a3b904bea7aa66069cf7815e2844c76d75d8",
  },
  {
    from: "build/native/runtimes/x64/OpenConsole.exe",
    to: "x64/OpenConsole.exe",
    sha256: "b7fd936c2668b87b9ecf7b3366dc6568afc1c6f981874cba3e955a1c35cf8160",
  },
  {
    from: "build/native/runtimes/arm64/OpenConsole.exe",
    to: "arm64/OpenConsole.exe",
    sha256: "ed7622fd0d3bedc9ab9f122f5e58edf0def9e7999224f52dd395ba9f54edbe09",
  },
];

const sha256 = (buffer) => createHash("sha256").update(buffer).digest("hex");

/**
 * Reads named entries out of a zip in memory.
 *
 * A `.nupkg` is a zip, and this is the only reason the script would otherwise
 * need a tool or a dependency to run. Written out instead — it is a central
 * directory walk and one `inflateRaw` — because the alternatives both cost more
 * than they look: a package to unzip three files is a supply-chain edge on the
 * one script whose whole job is verifying bytes, and shelling out to PowerShell
 * makes the download path impossible to exercise anywhere but Windows, which is
 * the one platform this cannot be tested on before it ships.
 *
 * Handles what NuGet actually produces: stored and deflated entries, no
 * encryption, no zip64. Anything else is an error rather than a guess.
 */
function readZipEntries(zip, wanted) {
  const END_SIG = 0x06054b50;
  const CENTRAL_SIG = 0x02014b50;
  const LOCAL_SIG = 0x04034b50;

  // The end-of-central-directory record is last, after a comment of unknown
  // length, so it is found by scanning backwards for its signature.
  let end = -1;
  for (let i = zip.length - 22; i >= 0; i--) {
    if (zip.readUInt32LE(i) === END_SIG) {
      end = i;
      break;
    }
  }
  if (end < 0) throw new Error("not a zip archive: no end-of-central-directory record");

  const count = zip.readUInt16LE(end + 10);
  let offset = zip.readUInt32LE(end + 16);
  const found = new Map();

  for (let i = 0; i < count; i++) {
    if (zip.readUInt32LE(offset) !== CENTRAL_SIG) {
      throw new Error(`corrupt central directory at entry ${i}`);
    }
    const method = zip.readUInt16LE(offset + 10);
    const compressedSize = zip.readUInt32LE(offset + 20);
    const nameLength = zip.readUInt16LE(offset + 28);
    const extraLength = zip.readUInt16LE(offset + 30);
    const commentLength = zip.readUInt16LE(offset + 32);
    const localOffset = zip.readUInt32LE(offset + 42);
    const name = zip.toString("utf8", offset + 46, offset + 46 + nameLength);

    if (wanted.includes(name)) {
      // The local header repeats the name and extra fields, and its extra
      // length routinely differs from the central one — so the data offset has
      // to be read from the local header rather than computed from this one.
      if (zip.readUInt32LE(localOffset) !== LOCAL_SIG) {
        throw new Error(`corrupt local header for ${name}`);
      }
      const localNameLength = zip.readUInt16LE(localOffset + 26);
      const localExtraLength = zip.readUInt16LE(localOffset + 28);
      const start = localOffset + 30 + localNameLength + localExtraLength;
      const raw = zip.subarray(start, start + compressedSize);

      if (method === 0) {
        found.set(name, Buffer.from(raw));
      } else if (method === 8) {
        found.set(name, inflateRawSync(raw));
      } else {
        throw new Error(`${name} uses unsupported zip compression method ${method}`);
      }
    }

    offset += 46 + nameLength + extraLength + commentLength;
  }

  for (const name of wanted) {
    if (!found.has(name)) throw new Error(`${name} is not in the package`);
  }
  return found;
}

/** Whether every file is already there with the right bytes. */
function alreadyFetched() {
  return FILES.every((file) => {
    const path = join(dest, file.to);
    if (!existsSync(path)) return false;
    // Checked rather than assumed present: a half-extracted directory from an
    // interrupted build would otherwise be taken for a finished one, and the
    // failure would surface as a bundle that silently lacks the fix.
    return sha256(readFileSync(path)) === file.sha256;
  });
}

async function main() {
  // Windows only — there is no ConPTY anywhere else. `TAURUS_CONPTY_FORCE`
  // runs it anyway, which is how the download, the hash checks and the
  // extraction get exercised on the machines this is actually developed on:
  // the one platform that needs this is the one platform that cannot test it
  // before a release.
  if (process.platform !== "win32" && !process.env.TAURUS_CONPTY_FORCE) {
    return;
  }

  if (alreadyFetched()) {
    console.log("conpty: already present");
    return;
  }

  const url = `https://api.nuget.org/v3-flatcontainer/${PACKAGE}/${VERSION}/${PACKAGE}.${VERSION}.nupkg`;
  console.log(`conpty: fetching ${VERSION}`);

  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`could not download ConPTY ${VERSION}: HTTP ${response.status}`);
  }
  const nupkg = Buffer.from(await response.arrayBuffer());

  const actual = sha256(nupkg);
  if (actual !== NUPKG_SHA256) {
    // Hard failure rather than a warning. The point of pinning is that bytes
    // nobody reviewed do not reach a release.
    throw new Error(
      `ConPTY package hash mismatch.\n  expected ${NUPKG_SHA256}\n  got      ${actual}\n` +
        "Refusing to use it. If Microsoft republished the package, review the new " +
        "bytes and update NUPKG_SHA256 and the per-file hashes in this script.",
    );
  }

  const entries = readZipEntries(
    nupkg,
    FILES.map((file) => file.from),
  );

  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dest, { recursive: true });

  for (const file of FILES) {
    const bytes = entries.get(file.from);
    const got = sha256(bytes);
    if (got !== file.sha256) {
      throw new Error(
        `ConPTY file ${file.from} hash mismatch.\n  expected ${file.sha256}\n  got      ${got}`,
      );
    }
    const target = join(dest, ...file.to.split("/"));
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, bytes);
    console.log(`conpty: ${file.to} (${bytes.length} bytes)`);
  }

  // The license travels with the binaries. They are MIT, which permits
  // redistribution and requires the notice go with it — so it is written beside
  // them rather than left to a reader to go and find.
  writeFileSync(
    join(dest, "LICENSE-ConPTY.txt"),
    [
      "Microsoft.Windows.Console.ConPTY",
      `version ${VERSION}`,
      "https://github.com/microsoft/terminal",
      "",
      "Copyright (c) Microsoft Corporation.",
      "",
      "MIT License",
      "",
      "Permission is hereby granted, free of charge, to any person obtaining a copy",
      'of this software and associated documentation files (the "Software"), to deal',
      "in the Software without restriction, including without limitation the rights",
      "to use, copy, modify, merge, publish, distribute, sublicense, and/or sell",
      "copies of the Software, and to permit persons to whom the Software is",
      "furnished to do so, subject to the following conditions:",
      "",
      "The above copyright notice and this permission notice shall be included in all",
      "copies or substantial portions of the Software.",
      "",
      'THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR',
      "IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,",
      "FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE",
      "AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER",
      "LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,",
      "OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE",
      "SOFTWARE.",
      "",
    ].join("\n"),
  );

  console.log("conpty: ready");
}

main().catch((error) => {
  console.error(`conpty: ${error.message}`);
  process.exit(1);
});
