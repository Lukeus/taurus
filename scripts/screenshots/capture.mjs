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
// It is a build, served under the app's own CSP, and the run fails if the page
// reports a single refusal. A refused font draws in a fallback face and a
// refused image draws as nothing, and a picture of either still looks like a
// picture: sketch embeds drew in a serif in the packaged app for as long as
// these were taken without the policy. The dev server cannot be put under it —
// its hot-reload preamble is an inline script — which is why this builds.
//
// Chrome is driven over the DevTools protocol, in real time: each picture is
// taken once the page says its scene is finished (`data-ready` on the body),
// not when a clock runs out. The previous way, `--screenshot` with
// `--virtual-time-budget`, could not take pictures of a build at all: virtual
// time raced ahead while a large lazily loaded chunk compiled off the main
// thread, and a quarter to a third of the shots were of the moment before their
// scene, a different set every run.
//
//   pnpm screenshots            every shot
//   pnpm screenshots notes mcp  just these, by name
//
// Chrome is located rather than depended on. A machine without it gets a clear
// message and no images, which is correct: this is a documentation chore, not
// part of the build.

import { spawn } from "node:child_process";
import { createReadStream, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, extname, join, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const out = join(root, "docs", "screenshots");
const PORT = 5177;

/** The window the design is drawn at. */
const WIDTH = 1280;
const HEIGHT = 840;

/** How long a scene gets to report itself finished before the run fails on it. */
const SCENE_TIMEOUT_MS = 30_000;

// One image per thing worth seeing, and the two palettes split across them
// rather than shown twice: a second copy of the same frame in the other theme
// costs a reader a scroll and tells them nothing the first did not.
const SHOTS = [
  { name: "app-dark", shot: "top", theme: "dark" },
  { name: "app-light", shot: "chart", theme: "light" },
  { name: "questions", shot: "questions", theme: "dark" },
  { name: "permission-diff", shot: "permission", theme: "dark" },
  // Changes beside the conversation rather than over it, and unfolded to the
  // one diff that spans the whole conversation — because the turn list above
  // it is the half that already existed, and a frame of that alone would not
  // show what moved.
  { name: "changes", shot: "changes", theme: "dark" },
  // A conversation whose files are set aside because a fork of it has the
  // workspace: the fork's row in the rail, the banner offering the switch
  // back, and Fork beside Rewind on each turn. The only picture of the three
  // together, which is how somebody meets them.
  { name: "fork", shot: "fork", theme: "dark" },
  // Shown with two servers working, one that cannot find its program, and one
  // switched off — because the panel exists for the ones that are not working,
  // and a frame of four green rows would say nothing about what it is for.
  { name: "mcp", shot: "mcp", theme: "dark" },
  // The catalogue, searched for something it cannot offer — which is half of
  // what a curated list is for, and the half a registry search cannot do.
  { name: "mcp-catalog", shot: "catalog", theme: "dark" },
  // Appearance, with two themes on disk and one of them the workspace's own —
  // because a repository carrying its own branding is the half of this that a
  // picture of one global theme would not say exists.
  { name: "appearance", shot: "appearance", theme: "dark" },
  // The providers tab with a card open, which is where nearly all of the
  // Settings form lives — the fields, the per-model overrides, and the row a
  // key is stored from.
  { name: "providers", shot: "providers", theme: "dark" },
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
  // The canvas: a Markdown file open beside the conversation that asked for
  // it, with the passage the model pointed at selected. Two things only a
  // photograph can check — that the split is a split, and that a line number
  // lands on the line it counts.
  { name: "canvas", shot: "canvas", theme: "dark" },
  // The notebook: both scopes in the list, one note open in the editor that
  // wraps. The only picture of the pane, and the only check of the prose editor
  // there is — jsdom measures nothing, so a mount test can prove the text is in
  // the box and nothing about where its lines break.
  { name: "notes", shot: "notes", theme: "dark" },
  // The same note in Read, with its Mermaid fence drawn. The only picture of the
  // reader, and the only check that a diagram read out of a fence lands where
  // its own arrows say — the boxes and the stages are what the unit tests can
  // hold, and the geometry is not.
  { name: "notes-diagram", shot: "notes-diagram", theme: "dark" },
  // A sketch in the editor. The only check that Excalidraw's fonts arrive from
  // this origin and that none of the app's element rules reach its toolbar —
  // both are failures that look like nothing at all to a test.
  { name: "sketch", shot: "sketch", theme: "dark" },
  // A sketch drawn into a note being read, which is the embed's only check.
  { name: "notes-sketch", shot: "notes-sketch", theme: "dark" },
  // The note editor's block list, open under a slash mid-note. The only check
  // of where it lands — the editor wraps, so the caret is found by layout, and
  // jsdom has none.
  { name: "notes-complete", shot: "notes-complete", theme: "dark" },
  // A note in Write and Read side by side, with a ticked task and a note link
  // in the rendered half.
  { name: "notes-split", shot: "notes-split", theme: "dark" },
  // Read, reached with ⌘E from an editor scrolled down to the diagram: the
  // check that the two views line up by their headings rather than both
  // opening at the top.
  { name: "notes-kept", shot: "notes-kept", theme: "dark" },
  // A sketch with the list folded: the check that folding it is enough to take
  // Excalidraw out of its compact layout and give it back its zoom controls.
  // The one shot taken narrower than the rest. At 840 tall the canvas is 574,
  // so only Excalidraw's width rule applies: compact under 730. The canvas is
  // the window less 520 beside the list and less 304 with it folded, so a
  // 1200 window is compact before the fold (680) and not after it (896) —
  // which is the check. A 1280 one is never compact at all.
  { name: "sketch-wide", shot: "sketch-wide", theme: "dark", width: 1200 },
  // The moment the two writers meet: Taurus wrote the file while there was
  // typing in it, so both versions exist and neither has been chosen. The only
  // picture of the rule the whole write slice is built around.
  { name: "canvas-conflict", shot: "canvas-conflict", theme: "dark" },
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

const wanted = process.argv.slice(2);
const unknown = wanted.filter((name) => !SHOTS.some((s) => s.name === name));
if (unknown.length > 0) {
  console.error(`no such shot: ${unknown.join(", ")}`);
  process.exit(1);
}
const shots = wanted.length > 0 ? SHOTS.filter((s) => wanted.includes(s.name)) : SHOTS;

mkdirSync(out, { recursive: true });

// Kept out of the repository, and rebuilt every run so a picture is never of a
// stale build.
const dist = join(root, "node_modules", ".cache", "taurus-screenshots");

// No shell, so a repository path containing a space survives the trip — the
// same reason `scripts/bindings.mjs` spawns cargo directly.
await run(
  "npx",
  [
    "vite", "build",
    "--config", "scripts/screenshots/vite.config.ts",
    "--outDir", dist, "--emptyOutDir",
    "--sourcemap", "false",
    "--logLevel", "warn",
  ],
  { cwd: root, stdio: ["ignore", "inherit", "inherit"] },
);

/**
 * The policy the app ships with, read from the file it ships in rather than
 * copied here, so the two cannot drift. Tauri adds hashes for inline scripts
 * and styles to it at runtime; this page has neither kind of script, so the
 * string as written is the stricter of the two and the right one to test under.
 */
const CSP = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8")).app
  .security.csp;

/** Where the page posts a refusal — see `csp.ts` beside this file. */
const REPORT = "/__csp-violation";

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".ttf": "font/ttf",
  ".wasm": "application/wasm",
};

/** What the page reported as refused, by the shot being taken at the time. */
const refused = new Map();
let taking = "";

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  if (req.method === "POST" && url.pathname === REPORT) {
    let body = "";
    for await (const chunk of req) body += chunk;
    if (!refused.has(taking)) refused.set(taking, new Set());
    refused.get(taking).add(body);
    res.writeHead(204).end();
    return;
  }
  let file = join(dist, decodeURIComponent(url.pathname));
  if (!file.startsWith(dist + sep) || !(await isFile(file))) {
    // A path with no extension is the app's own page, the way a Tauri window
    // has it. Anything else that is missing is a 404 rather than that page —
    // a font answered with HTML fails to parse instead of plainly being
    // absent, which is what Xiaolai is meant to be.
    if (extname(url.pathname) !== "") {
      res.writeHead(404, { "Content-Security-Policy": CSP }).end();
      return;
    }
    file = join(dist, "index.html");
  }
  res.writeHead(200, {
    "Content-Type": TYPES[extname(file)] ?? "application/octet-stream",
    "Content-Security-Policy": CSP,
  });
  createReadStream(file).pipe(res);
});
await new Promise((resolve) => server.listen(PORT, resolve));

// A throwaway profile, so nothing of the person's own Chrome — extensions,
// flags, a remembered zoom — reaches a picture.
const profile = mkdtempSync(join(tmpdir(), "taurus-screenshots-"));
const browser = spawn(
  chrome,
  [
    "--headless=new",
    "--disable-gpu",
    "--hide-scrollbars",
    // Deterministic text rendering, so a rerun on the same machine produces an
    // identical file and git does not see a diff in every image every time.
    "--font-render-hinting=none",
    "--no-first-run",
    "--no-default-browser-check",
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    "about:blank",
  ],
  { stdio: ["ignore", "ignore", "pipe"] },
);

const exited = new Promise((resolve) => browser.on("exit", resolve));
let stopped = false;
/**
 * Ends Chrome and the server, and removes the profile once Chrome has let go
 * of it — removing it while Chrome is still writing its last files fails.
 */
const stop = async () => {
  if (stopped) return;
  stopped = true;
  browser.kill();
  server.close();
  await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 5_000))]);
  rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
};
process.on("SIGINT", () => {
  void stop().finally(() => process.exit(130));
});

/** Shots whose scene never reported itself finished, with what the page said. */
const unfinished = [];
let cdp;

// In a `finally`, so a failure part way through doesn't leave a headless Chrome
// running with nothing to end it.
try {
  cdp = await connect(await devtoolsUrl(browser));
  for (const { name, shot, theme, width = WIDTH } of shots) {
    taking = name;
    const result = await take(`http://localhost:${PORT}/?shot=${shot}&theme=${theme}`, width);
    if (result.ready) {
      writeFileSync(join(out, `${name}.png`), Buffer.from(result.png, "base64"));
      console.log(`wrote docs/screenshots/${name}.png`);
    } else {
      unfinished.push({ name, errors: result.errors });
    }
  }
} finally {
  await stop();
}

if (unfinished.length > 0) {
  console.error("\nthese scenes never reported themselves finished, so no picture was taken:");
  for (const { name, errors } of unfinished) {
    console.error(`  ${name}${errors.length > 0 ? `: ${errors.join("; ")}` : ""}`);
  }
}

// Reported after every picture rather than at the first refusal, so one run
// shows all of them — and the images are still written, since a refused font
// is easier to understand beside the picture it spoiled.
const spoiled = [...refused].filter(([, seen]) => seen.size > 0);
if (spoiled.length > 0) {
  console.error("\nthe app's CSP refused something the page asked for:");
  for (const [name, seen] of spoiled) {
    for (const what of seen) console.error(`  ${name}: ${what}`);
  }
}
if (spoiled.length > 0 || unfinished.length > 0) process.exit(1);

/**
 * One picture: a fresh browser context (so no shot inherits another's local
 * storage), the window at its size, and the screenshot once the page has said
 * its scene is finished.
 */
async function take(url, width) {
  const { browserContextId } = await cdp.send("Target.createBrowserContext");
  const { targetId } = await cdp.send("Target.createTarget", {
    url: "about:blank",
    browserContextId,
  });
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
  const page = (method, params) => cdp.send(method, params, sessionId);

  // What the page threw, so a scene that never finishes says why.
  const errors = [];
  const unlisten = cdp.on(sessionId, "Runtime.exceptionThrown", ({ exceptionDetails }) => {
    errors.push(exceptionDetails.exception?.description?.split("\n")[0] ?? exceptionDetails.text);
  });

  try {
    await page("Runtime.enable");
    await page("Emulation.setDeviceMetricsOverride", {
      width,
      height: HEIGHT,
      deviceScaleFactor: 2,
      mobile: false,
    });
    // Asked of the page as well as by flag: an overlay scrollbar caught part
    // way through fading out was the one thing that differed between runs.
    await page("Emulation.setScrollbarsHidden", { hidden: true });
    await page("Page.navigate", { url });

    const deadline = Date.now() + SCENE_TIMEOUT_MS;
    let ready = false;
    while (!ready && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 100));
      const { result } = await page("Runtime.evaluate", {
        expression: "document.body?.dataset.ready === 'true'",
        returnByValue: true,
      });
      ready = result.value === true;
    }
    if (!ready) return { ready, errors };

    const { data } = await page("Page.captureScreenshot", { format: "png" });
    return { ready, png: data, errors };
  } finally {
    unlisten();
    await cdp.send("Target.closeTarget", { targetId });
    await cdp.send("Target.disposeBrowserContext", { browserContextId });
  }
}

/** The browser's DevTools endpoint, from the line Chrome prints when it is up. */
function devtoolsUrl(child) {
  return new Promise((resolve, reject) => {
    let seen = "";
    const timer = setTimeout(() => reject(new Error("Chrome did not start its DevTools server")), 15_000);
    child.on("error", reject);
    child.stderr.on("data", (chunk) => {
      seen += chunk;
      const found = seen.match(/DevTools listening on (ws:\/\/\S+)/);
      if (found) {
        clearTimeout(timer);
        resolve(found[1]);
      }
    });
  });
}

/**
 * The smallest DevTools client this needs: commands answered by id, and events
 * handed to whoever is listening for that session. Node's own `WebSocket`, so
 * there is nothing to install.
 */
async function connect(url) {
  const socket = new WebSocket(url);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  let next = 0;
  const pending = new Map();
  const listeners = new Set();
  socket.addEventListener("message", ({ data }) => {
    const message = JSON.parse(data);
    if (message.id !== undefined) {
      const waiting = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) waiting?.reject(new Error(`${waiting.method}: ${message.error.message}`));
      else waiting?.resolve(message.result);
      return;
    }
    for (const listener of listeners) listener(message);
  });
  return {
    send(method, params = {}, sessionId) {
      const id = ++next;
      socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
      return new Promise((resolve, reject) => pending.set(id, { resolve, reject, method }));
    },
    on(sessionId, method, handle) {
      const listener = (message) => {
        if (message.sessionId === sessionId && message.method === method) handle(message.params);
      };
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

/** Whether a path is a file that exists. */
async function isFile(path) {
  try {
    return (await stat(path)).isFile();
  } catch {
    return false;
  }
}

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: ["ignore", "ignore", "ignore"], ...options });
    child.on("error", reject);
    child.on("close", (code) =>
      code === 0 ? resolve() : reject(new Error(`${command} exited ${code}`)),
    );
  });
}
