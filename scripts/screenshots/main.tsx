/**
 * The app, rendered in a browser for the README's images.
 *
 * The only thing stubbed is the one thing a browser cannot have: the Tauri IPC
 * bridge. `window.__TAURI_INTERNALS__` is the whole boundary — `invoke` goes
 * through it and nothing else does — so answering it with fixtures is enough to
 * run the real `App`, the real store, the real components, and the real
 * stylesheet. Nothing here is a mock-up of the UI; it *is* the UI, reading
 * canned answers instead of a live harness.
 *
 * This has to install before anything imports the app, which is why `App` is
 * pulled in dynamically at the bottom rather than with a top-level import that
 * would hoist above the assignment.
 */
import { createRoot } from "react-dom/client";

import { APPLE } from "../../src/lib/keys";

import {
  BACKGROUND_JOBS,
  BACKGROUND_OUTPUT,
  CHECKPOINTS,
  CONVERSATION_CHANGES,
  MCP_CATALOG,
  REPO,
  DATASETS,
  DATA_EVENTS,
  DATA_PROFILE,
  DATA_PROMPT,
  DATA_QUERY,
  DATA_TABLES,
  EVENTS,
  GLOBAL_PROVIDERS,
  DOCUMENT,
  DOCUMENT_EVENTS,
  DOCUMENT_PROMPT,
  MCP_ENVIRONMENT,
  MOTION_EVENTS,
  MCP_SERVERS,
  MODELS,
  KEY_STATUSES,
  PERMISSION,
  PERMISSION_RULES,
  PROMPT,
  RECIPES,
  RECIPE_RUN,
  SEARCH_HITS,
  SESSIONS,
  STATUS,
  USAGE,
  TRACES,
  THEMES,
} from "./fixtures";

/** Which part of the conversation to frame. See `capture.mjs` for the set. */
const shot = new URLSearchParams(location.search).get("shot") ?? "top";
const theme = new URLSearchParams(location.search).get("theme") ?? "dark";

/**
 * Polls until something is there, or gives up rather than hanging.
 *
 * On a timer rather than on `requestAnimationFrame`, which is the obvious
 * choice and the wrong one here: Chrome runs these shots under
 * `--virtual-time-budget`, and a frame loop that reschedules itself every
 * frame spends the whole budget without ever letting the fetch it is waiting
 * on land. A timer is fast-forwarded instead, so the wait costs nothing and
 * the work in between actually happens.
 */
function until<T>(look: () => T | null | undefined, tries = 200): Promise<T> {
  return new Promise((resolve, reject) => {
    const tick = (left: number) => {
      const found = look();
      if (found) return resolve(found);
      if (left <= 0) return reject(new Error("nothing turned up to click"));
      setTimeout(() => tick(left - 1), 20);
    };
    tick(tries);
  });
}

/** Waits for a button matching `text` to exist, and hands back its click. */
async function click(selector: string, text: (label: string) => boolean) {
  const button = await until(() =>
    [...document.querySelectorAll(selector)].find((element) =>
      text(element.textContent ?? ""),
    ),
  );
  return () => (button as HTMLButtonElement).click();
}

/**
 * The half-written join the completion shot is of.
 *
 * Ends on `i.` because that is the moment the list is worth a picture: two
 * files are in scope, one of them is aliased, and what comes next is a column
 * of that one specifically.
 */
const JOIN = `SELECT i.category, count(*) AS n
  FROM interactions x
  JOIN items i ON i.item_id = x.item_id
 WHERE x.event = 'add_to_cart'
 GROUP BY i.`;

/**
 * Types into a React-controlled text box, `<textarea>` or `<input>`.
 *
 * Setting `.value` alone is swallowed: React keeps a tracker on the node and
 * skips an event whose value it believes it already has. Going through the
 * prototype's setter writes past the tracker, which is what makes the `input`
 * that follows look like a keystroke. Which prototype comes off the element,
 * because the two do not share the property and the query box and the palette
 * are one of each.
 */
function typeInto(box: HTMLTextAreaElement | HTMLInputElement, text: string) {
  const write = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(box),
    "value",
  )!.set!;
  write.call(box, text);
  box.setSelectionRange(text.length, text.length);
  box.dispatchEvent(new Event("input", { bubbles: true }));
}

const ANSWERS: Record<string, unknown> = {
  get_status: { ...STATUS, settings: { ...STATUS.settings, theme } },
  list_sessions: SESSIONS,
  list_models: MODELS,
  list_checkpoints: CHECKPOINTS,
  repo_status: REPO,
  conversation_changes: CONVERSATION_CHANGES,
  list_skills: [],
  list_commands: [],
  list_agents: [],
  list_tools: [],
  agent_roster_cost: 0,
  list_datasets: DATASETS,
  dataset_profile: DATA_PROFILE,
  list_recipes: RECIPES,
  run_recipe: RECIPE_RUN,
  query_data: DATA_QUERY,
  dataset_tables: DATA_TABLES,
  // The canvas reads the file itself rather than being handed it by the tool
  // — see `taurus_host::document` — so the shot needs this as well as the
  // event that opened it.
  open_document: DOCUMENT,
  list_mcp_servers: MCP_SERVERS,
  // The catalogue is shipped in the binary, so the real one is what the shot
  // should show — read off disk at build time rather than restated here, which
  // would let a picture of the panel disagree with what the panel offers.
  mcp_catalog: MCP_CATALOG,
  // Only `npx` resolves in the fixture, so the uvx entries wear the warning —
  // which is the state worth photographing. A frame where everything is
  // reachable says nothing about what the check is for.
  programs_on_path: ["npx"],
  search_sessions: SEARCH_HITS,
  usage_report: USAGE,
  trace_report: TRACES,
  mcp_environment: MCP_ENVIRONMENT,
  list_themes: THEMES,
  themes_dir: "~/.taurus/themes",
  // Settings reads all four of these the moment it opens, whichever tab is
  // showing. Without them the providers tab draws an empty list — which is how
  // the drawer was photographed for a while, and why the form it is mostly
  // made of had no picture of it at all.
  list_global_providers: GLOBAL_PROVIDERS,
  list_key_statuses: KEY_STATUSES,
  keychain_available: true,
  list_permission_rules: PERMISSION_RULES,
  // The dock's shell. Nothing is ever written back down the channel, so the
  // Terminal tab is an empty emulator — which is the right picture, because
  // the shot is of the tab beside it.
  terminal_open: "shell-1",
  // Empty, because the turn below is replayed as live events instead. Resume
  // still runs — it is what binds the window to a session and fills the rail.
  resume_session: {
    id: "s1",
    model: "qwen3.6:27b",
    provider_id: "ollama",
    native_tools: true,
    context_length: 32_768,
    messages: [],
  },
};

window.__TAURI_INTERNALS__ = {
  invoke: async (cmd: string, args?: Record<string, unknown>) => {
    // The event plugin's listen/unlisten. Nothing is ever emitted here — a
    // permission prompt or a proposal card would be a different screenshot.
    if (cmd.startsWith("plugin:event|")) return 0;
    // The one answer that has to read what it was asked. A background command
    // is polled with a cursor, and the pane appends whatever comes back — so a
    // stub that handed over the same log every quarter second would draw it
    // again every quarter second. Honouring the cursor is not extra fidelity
    // here; it is the feature the shot is of.
    if (cmd === "background") {
      const first = !args?.cursor;
      return {
        jobs: BACKGROUND_JOBS,
        output: {
          id: 1,
          text: first ? BACKGROUND_OUTPUT : "",
          missed: 0,
          cursor: BACKGROUND_OUTPUT.length,
        },
      };
    }
    // The conflict shot needs a save that comes back refused, which is a
    // decision about *this* call rather than a fixed answer — so it is here
    // rather than in `ANSWERS`.
    if (cmd === "save_document") {
      return shot === "canvas-conflict"
        ? {
            type: "stale",
            current: {
              ...DOCUMENT,
              text: DOCUMENT.text.replace(
                "backs off exponentially",
                "backs off exponentially, with full jitter",
              ),
              fingerprint: "9999-9",
            },
          }
        : { type: "written", document: { ...DOCUMENT, fingerprint: "2-2" } };
    }
    if (cmd in ANSWERS) return ANSWERS[cmd];
    // Anything else is a command a screenshot does not need. Answering null
    // rather than throwing keeps one unmodelled call from blanking the window.
    console.warn(`unstubbed command: ${cmd}`);
    return null;
  },
  transformCallback: (callback: unknown) => {
    const id = Math.floor(Math.random() * 1e9);
    (window as Record<string, unknown>)[`_${id}`] = callback;
    return id;
  },
  unregisterCallback: () => {},
  convertFileSrc: (path: string) => path,
};

// Set before the app boots so the first paint is already in the right palette;
// App re-applies it from the settings above on mount.
document.documentElement.dataset.theme = theme;

const { default: App } = await import("../../src/App");
const { useStore, reduce } = await import("../../src/state/store");
await import("../../src/styles.css");

createRoot(document.getElementById("root")!).render(<App />);

// Chrome's screenshot fires on a timer, not on a signal from the page, so the
// marker is what tells `capture.mjs` the app has finished its startup round
// trips. Without it a slow first paint silently produces an empty window.
requestAnimationFrame(() => {
  setTimeout(() => {
    // The transcript pins itself to the bottom as entries arrive, so framing
    // has to happen after that has settled — otherwise every image is of the
    // last thing in the conversation.
    // Folded by the store's own reducer, so the transcript on screen is
    // assembled by the code a real turn runs and not by this file.
    useStore.setState({
      // The query shot is of a data conversation rather than the build-timing
      // one — there is nothing to load or query in that turn, and the cards
      // this is a picture of only exist in one that does.
      // A turn actually in flight, which is the only state the motion exists
      // for. Seeded here rather than faked in CSS: the waveform's shape comes
      // from the category of the call that is running, so the picture is only
      // honest if a call really is.
      busy: shot === "motion",
      entries: (
        shot === "motion"
          ? MOTION_EVENTS
          : shot.startsWith("query")
            ? DATA_EVENTS
            : shot.startsWith("canvas")
              ? DOCUMENT_EVENTS
              : EVENTS
      ).reduce(reduce, [
        {
          kind: "user",
          id: "u1",
          text: shot.startsWith("query")
            ? DATA_PROMPT
            : shot.startsWith("canvas")
              ? DOCUMENT_PROMPT
              : PROMPT,
        },
      ] as never),
      // The dialog is state, not a route, so seeding it is all it takes to
      // photograph the moment a write is actually approved.
      ...(shot === "permission" ? { permission: PERMISSION as never } : {}),
      // Seeded, and the reason is worth the line: the canvas opens off the
      // `opening` errand, which is set as a turn's events are folded inside
      // `send`. This harness folds the same events with the same reducer but
      // never runs `send`, so nothing sets it. That is the right behaviour
      // rather than a hole — a conversation *reopened* rehydrates its entries
      // the same way, and a week-old transcript flinging a file onto the
      // screen would be the app deciding where you are looking.
      ...(shot.startsWith("canvas")
        ? { opening: { path: DOCUMENT.path, lines: { from: 15, to: 19 }, at: 1 } as never }
        : {}),
      // Seeded rather than fetched, for the same reason the entries above are:
      // the list arrives from `refresh`, which the harness never calls, and
      // the switch this shot presses does not exist until the list is there.
      ...(shot === "data" || shot === "recipes" || shot.startsWith("query")
        ? { datasets: DATASETS as never }
        : {}),
      // The title is the open session's, and this shot puts it directly above
      // a conversation about something else — the transcript and the header
      // would visibly disagree. The other data shots show the pane, where
      // there is no transcript to disagree with, so they keep the default.
      ...(shot.startsWith("query")
        ? {
            sessions: SESSIONS.map((session, index) =>
              index === 0
                ? { ...session, title: "Which category people finish" }
                : session,
            ) as never,
          }
        : {}),
    });

    const transcript = document.querySelector(".transcript");
    const target = {
      top: () => transcript?.scrollTo({ top: 0 }),
      chart: () => scrollTo(transcript, document.querySelectorAll(".view-card")[1]),
      questions: () => scrollTo(transcript, document.querySelector(".questions")),
      // The dialog centres itself over the scrim, so there is nothing to scroll.
      permission: () => {},
      // The panel is opened by the rail, and which drawer is open is local
      // state in `App` rather than in the store — so this presses the button
      // instead of seeding it. That is also the more honest picture: the drawer
      // in the image is one that was opened the way a user opens it.
      // Pressed rather than seeded, like the MCP drawer below: which pane is
      // showing is local state in `App`, and the picture is more honest for
      // being of a tab somebody clicked. The profile it then fetches is
      // answered from the stub above, well inside Chrome's virtual-time
      // budget.
      // Settings, on the tab a theme is picked from. Pressed rather than
      // seeded, like the drawers below: which tab is showing is local state in
      // `Settings`, and the picture is more honest for being of one somebody
      // clicked. The theme list arrives from the stub above, which is why the
      // brand row has anything in it.
      appearance: async () => {
        (await click(".rail-link b", (b) => b === "Settings"))();
        (await click(".pill-row .pill", (b) => b === "Appearance"))();
      },
      // The providers tab, with a card unfolded — which is where nearly all of
      // this drawer's form is. Folded it is a header row, and a picture of two
      // header rows says nothing about the fields, the model list or the key
      // row underneath them. The card opens itself only when it has a problem
      // to show, so the shot presses the disclosure the way a reader would.
      providers: async () => {
        (await click(".rail-link b", (b) => b === "Settings"))();
        const folds = await until(() => {
          const found = document.querySelectorAll(".settings-provider-fold");
          return found.length > 1 ? found : null;
        });
        (folds[1] as HTMLButtonElement).click();
      },
      data: () => {
        const tab = [...document.querySelectorAll(".pane-switch .seg")].find(
          (button) => button.textContent?.startsWith("Data"),
        );
        (tab as HTMLButtonElement | undefined)?.click();
      },
      // Three clicks, so the picture is of a run somebody actually asked for
      // rather than of a report seeded into place. Each one has to wait for
      // what the last one fetched — the Recipes tab does not exist until the
      // pane is open, and the Run button does not exist until the recipe list
      // has arrived — which is why this is the one target that is async.
      recipes: async () => {
        (await click(".pane-switch .seg", (b) => b.startsWith("Data")))();
        (await click(".data-switch .seg", (b) => b === "Recipes"))();
        (await click(".recipe button.primary", (b) => b.startsWith("Run")))();
        // And the report itself, which arrives a round trip after the click.
        await until(() => document.querySelector(".recipe-run"));
      },
      // Nothing to press. The cards are what the shot is of, and they are in
      // the transcript the moment the entries land — so this only has to put
      // the frame on them rather than at the bottom, where the reply is.
      // Framed on the running row and the line under it, which is where all
      // of the motion is.
      motion: () => scrollTo(transcript, document.querySelector(".working")),
      query: () =>
        scrollTo(transcript, document.querySelector(".dataset-card")),
      // The one picture of the split, and the only check of most of the
      // editor there is: jsdom reports every measurement as zero, so the mount
      // tests can prove the gutter has the right numbers in it and nothing
      // about whether a number sits on the line it counts. Nothing is pressed
      // — the canvas opens because the turn's `open_file` call opened it,
      // which is the behaviour being photographed. Waited on rather than
      // assumed, because the file arrives a round trip after the event that
      // asked for it.
      // The conflict, taken by actually causing one: type into the editor,
      // then answer the save with a `stale` carrying a different file. Nothing
      // here is seeded into place — the bar in the picture is the one the app
      // draws when a real save comes back refused, which is the only way to
      // photograph a rule rather than a stylesheet.
      "canvas-conflict": async () => {
        const area = (await until(() =>
          window.document.querySelector(".doc-input"),
        )) as HTMLTextAreaElement;
        typeInto(area, DOCUMENT.text.replace("exponentially", "with a jittered backoff"));
        await until(() => window.document.querySelector(".canvas-conflict"), 400);
        scrollTo(transcript, window.document.querySelector(".document-card"));
      },
      // The Changes panel, beside the conversation it is about — which is the
      // whole of what moved, so the shot has to hold both. Opened by pressing
      // the header chip a user presses, then unfolding the conversation-wide
      // diff: the turn list above it is the part that already existed, and the
      // one diff spanning two turns is the part that did not.
      changes: async () => {
        (await click(".topbar .chip", (b) => b.includes("changed")))();
        (await click(".everything-head", (b) => b.includes("whole diff")))();
        await until(() => document.querySelector(".everything .diff"));
      },
      // The catalogue, searched. Three clicks and a keystroke, so the picture
      // is of a list somebody actually looked something up in — and the query
      // is one with no answer, because the entries that explain why are half
      // the reason this is a curated list rather than a registry search.
      catalog: async () => {
        (await click(".rail-link b", (b) => b === "MCP"))();
        (await click(".card-add", (b) => b === "Browse servers"))();
        const box = (await until(() =>
          document.querySelector(".catalog-search"),
        )) as HTMLInputElement;
        // Left empty: the frame worth having is the whole list, where the
        // installable entries and the ones that only explain themselves sit
        // next to each other. Searching would photograph one card.
        typeInto(box, "");
        await until(() => document.querySelector(".catalog-card"));
      },
      canvas: async () => {
        // The editor, not the panel: `.canvas` is on screen the moment the
        // errand lands, and waiting on that would photograph an empty frame
        // while the file was still being read. `.doc-input` is the thing that
        // only exists once there is a document in it.
        await until(() => document.querySelector(".doc-input"));
        scrollTo(transcript, document.querySelector(".document-card"));
      },
      // The other half, taken the way somebody takes it: press the card's own
      // button and photograph where it lands. Async because the grid does not
      // exist until the pane has mounted and the query has come back — which
      // is also what makes this a check of the round trip and not just of the
      // stylesheet.
      "query-run": async () => {
        (await click(".query-card button", (b) => b === "Run in Query"))();
        await until(() => document.querySelector(".grid-box"));
      },
      // A join, halfway through being written. This is the only picture of
      // the completion list there is, and the only check of where it lands:
      // jsdom has no layout, so the mount tests can prove the list has the
      // right rows in it and nothing about whether it is under the caret.
      "query-complete": async () => {
        (await click(".pane-switch .seg", (b) => b.startsWith("Data")))();
        (await click(".data-switch .seg", (b) => b === "Query"))();
        const box = (await until(() =>
          document.querySelector(".sql-input"),
        )) as HTMLTextAreaElement;
        (await click(".query-tables", (b) => b.includes("tables")))();
        typeInto(box, JOIN);
        await until(() => document.querySelector(".sql-menu"));
      },
      // A section folded away, which is the only state of the fold worth a
      // picture: open, a section looks like the plain list it replaced.
      //
      // The tooltip that landed alongside it is deliberately not here. It opens
      // on hover or focus, and neither survives `--virtual-time-budget` — the
      // tip is in the document on every run of this and at `opacity: 0` in
      // every image, because the clock the animation reads does not advance
      // with the one the timers do. It is checked against a real browser
      // instead; `Tooltip.tsx` says what that check covers.
      rail: async () => {
        (await click(".rail-group", (b) => b.includes("Earlier")))();
      },
      // The palette, mid-search. Opened with the key rather than by pressing
      // anything, which makes this the one check that the shortcut is bound at
      // all — jsdom can prove `isChord` agrees with the label, and nothing but
      // a real browser can prove the listener is on the window.
      //
      // Typed rather than seeded, because what this is a picture of is the
      // three groups arriving at different speeds: two of them are already
      // there when the third lands underneath.
      palette: async () => {
        // The modifier the app is actually listening for on this machine,
        // taken from the same constant the row's label is drawn from. Sending
        // the other one would be refused — which is correct, and would make
        // this shot fail for the one reason that is not a regression.
        window.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "k",
            metaKey: APPLE,
            ctrlKey: !APPLE,
            bubbles: true,
          }),
        );
        const box = (await until(() =>
          document.querySelector(".palette-input"),
        )) as HTMLInputElement;
        typeInto(box, "context");
        await until(() => document.querySelector(".palette-excerpt"));
      },
      // The context account, opened the way the rail opens it. The meter above
      // the composer opens the same panel and is the more likely route, but it
      // hides itself below half a window — and a shot that had to fill the
      // window first would be a picture of a full context rather than of this.
      context: async () => {
        (await click(".rail-link", (b) => b.startsWith("Context")))();
        await until(() => document.querySelector(".usage-table"));
      },
      // Where the time went, with the top turn's waterfall open. Expanded by
      // pressing the row rather than seeded open, because one turn at a time
      // is state inside the panel — and a shot of the collapsed list would be
      // a picture of three rows rather than of the feature underneath them.
      traces: async () => {
        (await click(".rail-link", (b) => b.startsWith("Traces")))();
        (await click(".trace-turn", () => true))();
        await until(() => document.querySelector(".trace-step"));
      },
      // The dock, on the tab of a test run that failed. Opened and switched
      // to the way anybody does it, because which tab is showing is state in
      // `App` and a seeded one would not prove the strip is reachable.
      //
      // The command that failed rather than the dev server beside it: the
      // whole reason this pane exists is that the model could read this and
      // the user could not.
      background: async () => {
        (await click(".rail-link", (b) => b.startsWith("Terminal")))();
        (await click(".dock-tab", (b) => b.includes("cargo test")))();
        await until(() =>
          document.querySelector(".job-out")?.textContent?.includes("FAILED")
            ? true
            : null,
        );
      },
      mcp: () => {
        const link = [...document.querySelectorAll(".rail-link")].find(
          (button) => button.textContent?.startsWith("MCP"),
        );
        (link as HTMLButtonElement | undefined)?.click();
      },
    }[shot];
    // A frame for React to paint the seeded entries before anything is
    // measured; the scroll targets do not exist until it has. The flag waits
    // on the target rather than racing it — a shot that clicks through two
    // fetches would otherwise be photographed on the first one.
    requestAnimationFrame(() => {
      void Promise.resolve(target?.()).then(() => {
        document.body.dataset.ready = "true";
      });
    });
  }, 400);
});

/**
 * Puts `element` at the top of the scrolling transcript, with a little air.
 *
 * Asked for rather than measured. `.turn` carries `content-visibility: auto`,
 * so a turn that is currently scrolled off has no layout boxes for anything
 * inside it — `getBoundingClientRect` on a card in one reads zero, and the
 * framing computed from that put every image back at the top of the
 * conversation. `scrollIntoView` is the API that copes: the browser renders the
 * skipped subtree to answer it. The measurement afterwards is what places the
 * card exactly, and it is accurate now that the subtree is no longer skipped.
 */
function scrollTo(container: Element | null, element: Element | null | undefined) {
  if (!container || !element) return;
  element.scrollIntoView({ block: "start" });
  const gap = element.getBoundingClientRect().top - container.getBoundingClientRect().top;
  container.scrollTop += gap - 24;
}

declare global {
  interface Window {
    __TAURI_INTERNALS__: Record<string, unknown>;
  }
}
