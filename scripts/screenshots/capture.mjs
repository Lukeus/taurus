// Regenerates the screenshots the README shows.
//
// The app is a Tauri window, and a Tauri window cannot be captured from a
// script: it needs a desktop session, and on macOS a screen-recording grant
// that CI will never have. So the frontend is served on its own and driven in
// headless Chrome, with the IPC bridge answering fixtures — see
// `main.tsx`, which explains why that is the whole of the stubbing.
//
// What this produces is therefore the real interface at a fixed size with fixed
// data, and not a photograph of a running desktop app: there is no window
// chrome, and the conversation is canned. That is the trade worth making for
// images that can be regenerated, because the failure mode of hand-taken
// screenshots is that they quietly describe a version of the app that no longer
// exists.
//
//   pnpm screenshots
//
// Chrome is located rather than depended on. A machine without it gets a clear
// message and no images, which is correct: this is a documentation chore, not
// part of the build.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const out = join(root, "docs", "screenshots");
const PORT = 5177;

/** The window the design is drawn at. */
const WIDTH = 1280;
const HEIGHT = 840;

// One image per thing worth seeing, and the two palettes split across them
// rather than shown twice: a second copy of the same frame in the other theme
// costs a reader a scroll and tells them nothing the first did not.
const SHOTS = [
  { name: "app-dark", shot: "top", theme: "dark" },
  { name: "app-light", shot: "chart", theme: "light" },
  { name: "questions", shot: "questions", theme: "dark" },
  { name: "permission-diff", shot: "permission", theme: "dark" },
  // Shown with two servers working, one that cannot find its program, and one
  // switched off — because the panel exists for the ones that are not working,
  // and a frame of four green rows would say nothing about what it is for.
  { name: "mcp", shot: "mcp", theme: "dark" },
  // A profile rather than a page of rows, and a file with real problems in it:
  // one column 42% missing, one with too many distinct values to rank. A grid
  // of clean numbers would be a picture of a spreadsheet.
  { name: "data", shot: "data", theme: "dark" },
  // A recipe mid-report rather than sitting still. The delta column is the
  // whole reason a run is reported per step, and a frame of four rows all
  // saying "done" would say nothing about what it is for.
  { name: "recipe", shot: "recipes", theme: "dark" },
];

const CHROME = [
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  process.env.CHROME_PATH,
].filter((path) => path && existsSync(path));

if (CHROME.length === 0) {
  console.error(
    "no Chrome or Chromium found. Install one, or set CHROME_PATH to its binary.",
  );
  process.exit(1);
}
const chrome = CHROME[0];

mkdirSync(out, { recursive: true });

// No shell, so a repository path containing a space survives the trip — the
// same reason `scripts/bindings.mjs` spawns cargo directly.
const vite = spawn(
  "npx",
  ["vite", "--config", "scripts/screenshots/vite.config.ts", "--port", String(PORT), "--strictPort"],
  { cwd: root, stdio: ["ignore", "pipe", "inherit"] },
);
vite.on("error", (error) => {
  console.error(`could not start vite: ${error.message}`);
  process.exit(1);
});

const stop = () => vite.kill();
process.on("exit", stop);
process.on("SIGINT", () => {
  stop();
  process.exit(130);
});

await waitForServer(`http://localhost:${PORT}/`);

for (const { name, shot, theme } of SHOTS) {
  const file = join(out, `${name}.png`);
  await run(chrome, [
    "--headless",
    "--disable-gpu",
    "--hide-scrollbars",
    // Deterministic text rendering, so a rerun on the same machine produces an
    // identical file and git does not see a diff in every image every time.
    "--force-device-scale-factor=2",
    "--font-render-hinting=none",
    `--window-size=${WIDTH},${HEIGHT}`,
    `--screenshot=${file}`,
    // Generous: the page marks itself ready after its startup round trips, and
    // virtual time runs far faster than the wall clock.
    "--virtual-time-budget=8000",
    `http://localhost:${PORT}/?shot=${shot}&theme=${theme}`,
  ]);
  console.log(`wrote docs/screenshots/${name}.png`);
}

stop();

/** Resolves once the dev server answers, or gives up rather than hanging. */
async function waitForServer(url) {
  for (let attempt = 0; attempt < 60; attempt++) {
    try {
      await fetch(url);
      return;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  stop();
  console.error(`vite did not come up on ${url}`);
  process.exit(1);
}

function run(command, args) {
  return new Promise((resolve, reject) => {
    // Chrome writes its own noise to stderr on every run — a GPU warning, a
    // sandbox note — none of which is an error here. Only the exit code is.
    const child = spawn(command, args, { stdio: ["ignore", "ignore", "ignore"] });
    child.on("error", reject);
    child.on("close", (code) =>
      code === 0 ? resolve() : reject(new Error(`${command} exited ${code}`)),
    );
  });
}
