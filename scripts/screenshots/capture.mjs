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
  // What a data turn leaves in the transcript: two references into the pane
  // and a query that can be taken back out of it. The query card is the point
  // — it is the one card here that is neither a result nor a pointer but an
  // offer to ask again.
  { name: "query", shot: "query", theme: "dark" },
  // Where that button lands: the same query, asked again at full width, with
  // the offer to keep it. Captured by pressing the card rather than by opening
  // the tab, so the image is of the trip and not of the destination.
  { name: "query-run", shot: "query-run", theme: "dark" },
  // The box mid-join: the query painted, the schema of both files open under
  // it, and the completion list showing one table's columns after its alias.
  // The join marks are why two tables are loaded — a column both files have is
  // a column they can be joined on, and that is said in both places at once.
  { name: "query-complete", shot: "query-complete", theme: "dark" },
  // A turn in flight. A still image cannot show motion, which is exactly why
  // this is worth taking: it is the only check that the waveform renders where
  // it should, that the running row wears the treatment its category calls
  // for, and that none of it has landed on top of something else.
  { name: "motion", shot: "motion", theme: "dark" },
  // One box over the whole window, mid-search. One word that reaches all three
  // groups, which is the only way to photograph the ordering: a panel matched
  // by name, a conversation matched by its title, and — underneath, because it
  // arrives last and must not push the others down — two matched by what was
  // said in them, with the word marked in the line it was on.
  { name: "palette", shot: "palette", theme: "dark" },
  // Where the context went. A conversation whose reads are 82% of its tool
  // spend, three calls that repeated an earlier one exactly, and the fixed
  // cost of every request below it — which is the half that is not in the
  // transcript at all.
  { name: "context", shot: "context", theme: "dark" },
  // Where the time went, with a turn's waterfall open. The turn chosen has a
  // shape worth photographing: a four-second command, a tool call that failed,
  // and a `spawn` with a delegate's whole turn indented underneath it. A frame
  // of even bars would be a picture of the styling.
  { name: "traces", shot: "traces", theme: "dark" },
  // The rail with a section folded. An open section looks like the plain list
  // it replaced, so the shut one is the only state that shows the feature.
  { name: "rail", shot: "rail", theme: "dark" },
  // A background command, on screen while it is still the model's to read.
  // The tab strip is the whole feature — a test run that failed, a dev server
  // that will not finish on its own, and the shell they sit beside.
  { name: "background", shot: "background", theme: "dark" },
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
