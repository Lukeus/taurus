// @vitest-environment jsdom
//
// What the rest of the window pays for things that are not about it: a
// keystroke in the file open beside the conversation, and a render of `App`
// for something the rail does not show.
//
// Nothing on screen differs either way — the same rail is redrawn with the
// same list — so only counting the draws can see it. Two probes: the transcript
// pane, which `App` draws on every render and does not memoize, counts the
// window; `useSections`, which the rail calls once per render and nothing else
// calls, counts the rail.
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const open = { path: "src/main.rs", text: "fn main() {}\n", lines: 1, fingerprint: "13:1" };
const invoke = vi.fn((command: string): Promise<unknown> => {
  switch (command) {
    case "open_document":
      return Promise.resolve(open);
    case "background":
      return Promise.resolve({ jobs: [], output: null });
    default:
      return Promise.resolve([]);
  }
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [string])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const drawn = { window: 0, rail: 0 };
vi.mock("./components/Transcript", async (original) => {
  const actual = await original<typeof import("./components/Transcript")>();
  return {
    ...actual,
    Transcript: (props: Parameters<typeof actual.Transcript>[0]) => {
      drawn.window += 1;
      return createElement(actual.Transcript, props);
    },
  };
});
vi.mock("./lib/sections", async (original) => {
  const actual = await original<typeof import("./lib/sections")>();
  return {
    ...actual,
    useSections: () => {
      drawn.rail += 1;
      return actual.useSections();
    },
  };
});

const state = {
  status: {
    workspace: "/tmp/project",
    providers: [],
    effective_providers: [],
    skills: [],
    agents: [],
    mcp_servers: [],
    problems: [],
    skill_count: 0,
    agent_count: 0,
    theme: null,
    settings: {
      last_workspace: null,
      last_provider: null,
      last_model: null,
      skill_synthesis_enabled: true,
      agent_synthesis_enabled: true,
      disabled_tools: [],
      theme: "system",
      max_iterations: 25,
    },
  },
  session: null,
  sessions: [],
  entries: [],
  changed: [],
  proposals: [],
  agentProposals: [],
  datasets: [],
  queued: [],
  wrote: null,
  permission: null,
  trust: null,
  // The model's own request to open a file, which is how the canvas opens
  // without a click.
  opening: { path: "src/main.rs", lines: null, at: 1 },
  resuming: null,
  busy: false,
  stopping: false,
  error: null,
  init: vi.fn(),
  startSession: vi.fn(),
  resume: vi.fn(),
  setWorkspace: vi.fn(),
  recheck: vi.fn(),
  reload: vi.fn(),
  send: vi.fn(),
  stop: vi.fn(),
  refreshDatasets: vi.fn(),
  forgetDataset: vi.fn(),
  dismissError: vi.fn(),
  noteError: vi.fn(),
  answerPermission: vi.fn(),
  answerQuestions: vi.fn(),
  decideTrust: vi.fn(),
  remove: vi.fn(),
  rename: vi.fn(),
  resolveAgentProposal: vi.fn(),
  resolveProposal: vi.fn(),
  retry: vi.fn(),
  switchModel: vi.fn(),
  unqueue: vi.fn(),
};
vi.mock("./state/store", async (original) => ({
  ...(await original<typeof import("./state/store")>()),
  useStore: (select?: (s: typeof state) => unknown) => (select ? select(state) : state),
}));

import App from "./App";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;
Element.prototype.scrollIntoView = vi.fn();

const settle = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
  });

let cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

async function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(<App />));
  cleanup.push(() => {
    act(() => root.unmount());
    document.body.innerHTML = "";
  });
  let editor: HTMLTextAreaElement | null = null;
  for (let i = 0; i < 50 && !editor; i += 1) {
    await settle();
    editor = host.querySelector<HTMLTextAreaElement>(".canvas textarea");
  }
  expect(editor, "the canvas never opened on the file").not.toBeNull();
  return editor!;
}

function type(area: HTMLTextAreaElement, text: string) {
  const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
  act(() => {
    set.call(area, text);
    area.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

describe("what the window pays for things that are not about it", () => {
  it("draws the window once for unsaved typing, not once per key", async () => {
    const editor = await mount();
    const before = { ...drawn };

    const typing = "fn main() {\n    run";
    for (let at = typing.length - 4; at <= typing.length; at += 1) {
      type(editor, typing.slice(0, at));
    }

    expect(editor.value).toBe(typing);
    // The one draw is the edge: the file now holds what the disk does not, and
    // the composer says so. Five keys, one fact.
    expect(drawn.window - before.window).toBe(1);
    expect(drawn.rail).toBe(before.rail);
  });

  it("leaves the rail alone when the window draws for something the rail does not show", async () => {
    await mount();
    const before = { ...drawn };

    // The palette opening is state in `App`, and nothing the rail draws.
    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }),
      );
    });
    await settle();

    expect(drawn.window).toBeGreaterThan(before.window);
    expect(drawn.rail).toBe(before.rail);
  });
});
