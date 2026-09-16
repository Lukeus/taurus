// @vitest-environment jsdom
//
// Mounted, because the bug is in when App builds the palette's list: once,
// before the models arrive, and not again until something it depends on
// moves. A static render never gets far enough to see it.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it, vi } from "vitest";

let answerModels: (models: unknown) => void = () => {};
const invoke = vi.fn((command: string): Promise<unknown> => {
  switch (command) {
    case "list_models":
      return new Promise((resolve) => {
        answerModels = resolve;
      });
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

/** A provider whose config names no model, so only its listing can offer one. */
const ollama = {
  id: "ollama",
  kind: "ollama",
  base_url: "http://localhost:11434",
  models: [],
  default_model: null,
  api_key_env: null,
  api_key_header: null,
  native_tools: null,
  context_length: null,
  vision: null,
  api_prefix: null,
  thinking: null,
};

const state = {
  status: {
    workspace: "/tmp/project",
    providers: [ollama],
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
      last_provider: "ollama",
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
  wrote: [],
  permission: null,
  trust: null,
  opening: false,
  resuming: null,
  busy: false,
  running: [] as string[],
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
// Applied rather than ignored, as in `App.test.tsx`: App and the panes under
// it subscribe to slices, and a mock handing every caller the whole state
// would give a pane the store object where it expects a list.
vi.mock("./state/store", async (original) => ({
  ...(await original<typeof import("./state/store")>()),
  useStore: (select?: (s: typeof state) => unknown) =>
    select ? select(state) : state,
}));

import App from "./App";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;
// jsdom lays nothing out, so it has no `scrollIntoView`, and the palette keeps
// its selected row in view with it.
Element.prototype.scrollIntoView = vi.fn();

/** Lets pending promises and lazily loaded panels land. */
const settle = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
  });

describe("the command palette", () => {
  it("starts a conversation on the models that arrived after its list was built", async () => {
    // Built before the model list landed and never rebuilt, "New conversation"
    // still saw no models and opened Settings instead.
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => {
      root.render(<App />);
    });

    await act(async () => {
      answerModels([
        { id: "qwen3-coder", display_name: "qwen3-coder", context_length: null },
      ]);
    });
    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }),
      );
    });
    let option: HTMLElement | undefined;
    for (let i = 0; i < 100 && !option; i += 1) {
      await settle();
      option = [...document.querySelectorAll<HTMLElement>('[role="option"]')].find(
        (o) => o.textContent?.includes("New conversation"),
      );
    }
    expect(option, "the palette never opened").toBeDefined();

    await act(async () => option!.click());

    expect(state.startSession).toHaveBeenCalledWith("ollama", "qwen3-coder");
    act(() => root.unmount());
    document.body.innerHTML = "";
  });
});
