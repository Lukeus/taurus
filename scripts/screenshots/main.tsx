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

import {
  CHECKPOINTS,
  DATASETS,
  DATA_PROFILE,
  EVENTS,
  MCP_ENVIRONMENT,
  MCP_SERVERS,
  MODELS,
  PERMISSION,
  PROMPT,
  SESSIONS,
  STATUS,
} from "./fixtures";

/** Which part of the conversation to frame. See `capture.mjs` for the set. */
const shot = new URLSearchParams(location.search).get("shot") ?? "top";
const theme = new URLSearchParams(location.search).get("theme") ?? "dark";

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
  list_mcp_servers: MCP_SERVERS,
  mcp_environment: MCP_ENVIRONMENT,
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
  invoke: async (cmd: string) => {
    // The event plugin's listen/unlisten. Nothing is ever emitted here — a
    // permission prompt or a proposal card would be a different screenshot.
    if (cmd.startsWith("plugin:event|")) return 0;
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
      entries: EVENTS.reduce(reduce, [
        { kind: "user", id: "u1", text: PROMPT },
      ] as never),
      // The dialog is state, not a route, so seeding it is all it takes to
      // photograph the moment a write is actually approved.
      ...(shot === "permission" ? { permission: PERMISSION as never } : {}),
      // Seeded rather than fetched, for the same reason the entries above are:
      // the list arrives from `refresh`, which the harness never calls, and
      // the switch this shot presses does not exist until the list is there.
      ...(shot === "data" ? { datasets: DATASETS as never } : {}),
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
      mcp: () => {
        const link = [...document.querySelectorAll(".rail-link")].find(
          (button) => button.textContent?.startsWith("MCP"),
        );
        (link as HTMLButtonElement | undefined)?.click();
      },
    }[shot];
    // A frame for React to paint the seeded entries before anything is
    // measured; the scroll targets do not exist until it has.
    requestAnimationFrame(() => {
      target?.();
      document.body.dataset.ready = "true";
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
