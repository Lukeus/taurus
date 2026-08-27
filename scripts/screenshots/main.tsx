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
  DATASETS,
  DATA_EVENTS,
  DATA_PROFILE,
  DATA_PROMPT,
  DATA_QUERY,
  DATA_TABLES,
  EVENTS,
  MCP_ENVIRONMENT,
  MOTION_EVENTS,
  MCP_SERVERS,
  MODELS,
  PERMISSION,
  PROMPT,
  RECIPES,
  RECIPE_RUN,
  SEARCH_HITS,
  SESSIONS,
  STATUS,
  USAGE,
  TRACES,
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
  list_mcp_servers: MCP_SERVERS,
  search_sessions: SEARCH_HITS,
  usage_report: USAGE,
  trace_report: TRACES,
  mcp_environment: MCP_ENVIRONMENT,
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
            : EVENTS
      ).reduce(reduce, [
        {
          kind: "user",
          id: "u1",
          text: shot.startsWith("query") ? DATA_PROMPT : PROMPT,
        },
      ] as never),
      // The dialog is state, not a route, so seeding it is all it takes to
      // photograph the moment a write is actually approved.
      ...(shot === "permission" ? { permission: PERMISSION as never } : {}),
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
