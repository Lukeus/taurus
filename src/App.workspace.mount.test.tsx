// @vitest-environment jsdom
//
// What `App` itself does around a workspace switch and a click on a conversation:
// the order of the steps, which only shows with the whole window mounted. The
// canvas is a stand-in here, since its own tests cover what its flush does.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn((command: string): Promise<unknown> => {
  switch (command) {
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
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(() => Promise.resolve("/tmp/next")) }));

const SETTINGS_TAB = "taurus.settings.tab";
const steps: string[] = [];
let kept = 0;
vi.mock("./components/CanvasSlot", async () => {
  const { useEffect } = await import("react");
  return {
    keptIn: (workspace: string | null) => {
      steps.push(`keptIn ${workspace}`);
      return kept;
    },
    CanvasSlot: ({ flushRef, path, showing }: { path: string; showing: boolean; flushRef: { current: (() => Promise<boolean>) | null } }) => {
      useEffect(() => {
        flushRef.current = async () => {
          steps.push("flush");
          return true;
        };
      }, [flushRef]);
      return <div data-canvas={path} data-showing={showing} />;
    },
  };
});

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
  wire_api: null,
  reasoning_effort: null,
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
      recent_workspaces: ["/tmp/project", "/tmp/older"],
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
  sessions: [
    { id: "other", workspace: "/tmp/project", model: "m", started: 1, updated: 1, title: "Another conversation", turns: 1 },
  ],
  entries: [],
  changed: [],
  proposals: [],
  agentProposals: [],
  datasets: [],
  queued: [],
  wrote: null,
  permission: null,
  trust: null,
  opening: { path: "src/main.rs", lines: null, at: 1 },
  resuming: null,
  busy: false,
  running: [] as string[],
  stopping: false,
  error: null,
  init: vi.fn(),
  startSession: vi.fn(),
  resume: vi.fn(),
  openConversation: vi.fn(),
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
  useStore: Object.assign(
    (select?: (s: typeof state) => unknown) => (select ? select(state) : state),
    { getState: () => state },
  ),
}));

import App from "./App";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
Element.prototype.scrollIntoView = vi.fn();

const settle = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
  });

async function mounted() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => createRoot(host).render(<App />));
  await settle();
  return host;
}

const changeWorkspace = async (host: HTMLElement) => {
  const button = host.querySelector<HTMLButtonElement>(".rail-workspace, .composer-workspace")
    ?? [...host.querySelectorAll("button")].find((b) => /workspace/i.test(b.textContent ?? ""));
  expect(button, host.innerHTML.slice(0, 400)).toBeDefined();
  await act(async () => button!.click());
  await settle();
  // The rail opens the recent folders; the dialog is their last row. Waited
  // for rather than assumed after one settle: under a full parallel run the
  // switcher can take longer than 20ms to land.
  let row = option("Open folder…");
  for (let i = 0; i < 50 && !row; i += 1) {
    await settle();
    row = option("Open folder…");
  }
  expect(row, "the recent folders never opened").toBeDefined();
  await act(async () => row!.click());
  await settle();
};

const option = (text: string) =>
  [...document.querySelectorAll<HTMLElement>('[role="option"]')].find((o) =>
    o.textContent?.includes(text),
  );

beforeEach(() => {
  // Open tabs are kept per folder in storage, so each test starts with none.
  localStorage.clear();
  steps.length = 0;
  kept = 0;
  document.body.innerHTML = "";
  state.noteError.mockClear();
  state.openConversation.mockClear();
  state.status.workspace = "/tmp/project";
  state.setWorkspace.mockReset();
  state.setWorkspace.mockImplementation(async () => {
    steps.push("setWorkspace");
    state.status.workspace = "/tmp/next";
  });
});

describe("switching workspace", () => {
  it("writes the canvas first, and says what was kept after the switch", async () => {
    const host = await mounted();
    kept = 2;
    await changeWorkspace(host);

    expect(steps).toEqual(["flush", "setWorkspace", "keptIn /tmp/project"]);
    expect(state.noteError).toHaveBeenCalledTimes(1);
    expect(state.noteError.mock.calls[0][0]).toContain("2 files");
    expect(state.noteError.mock.calls[0][0]).toContain("project");
  });

  it("says nothing when nothing was kept", async () => {
    const host = await mounted();
    await changeWorkspace(host);
    expect(state.noteError).not.toHaveBeenCalled();
  });

  it("says nothing when the switch was refused and the folder is still open", async () => {
    state.setWorkspace.mockImplementation(async () => {
      steps.push("setWorkspace");
    });
    const host = await mounted();
    kept = 1;
    await changeWorkspace(host);
    expect(state.noteError).not.toHaveBeenCalled();
  });
});

describe("the recent folders", () => {
  it("opens from the rail, starts on the folder before this one, and switches on Enter", async () => {
    const host = await mounted();
    await act(async () => host.querySelector<HTMLButtonElement>(".rail-workspace")!.click());
    await settle();
    expect(option("project")?.textContent).toContain("open");
    expect(option("older")?.getAttribute("aria-selected")).toBe("true");

    const box = document.querySelector<HTMLInputElement>('[aria-label="Switch workspace"]')!;
    await act(async () => {
      box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    });
    await settle();
    expect(state.setWorkspace).toHaveBeenCalledWith("/tmp/older");
    expect(document.querySelector('[aria-label="Switch workspace"]')).toBeNull();
  });

  it("opens on Control and R, but not from the terminal", async () => {
    await mounted();
    const press = async (on: EventTarget) =>
      act(async () => {
        on.dispatchEvent(
          new KeyboardEvent("keydown", { key: "r", ctrlKey: true, bubbles: true, cancelable: true }),
        );
      });
    const shell = document.body.appendChild(document.createElement("div"));
    shell.className = "xterm";
    await press(shell);
    await settle();
    expect(document.querySelector('[aria-label="Switch workspace"]')).toBeNull();

    await press(window);
    await settle();
    expect(document.querySelector('[aria-label="Switch workspace"]')).not.toBeNull();
  });

  it("takes a folder off the list without switching to it", async () => {
    const host = await mounted();
    await act(async () => host.querySelector<HTMLButtonElement>(".rail-workspace")!.click());
    await settle();
    await act(async () =>
      document.querySelector<HTMLButtonElement>('[aria-label="Remove older from recent folders"]')!.click(),
    );
    expect(invoke).toHaveBeenCalledWith("forget_workspace", { path: "/tmp/older" });
    expect(state.setWorkspace).not.toHaveBeenCalled();
  });
});

describe("clicking a conversation in the rail", () => {
  it("goes through the action that reports a failure", async () => {
    const host = await mounted();
    const row = [...host.querySelectorAll<HTMLElement>("button")].find((b) =>
      b.textContent?.includes("Another conversation"),
    );
    expect(row).toBeDefined();
    await act(async () => row!.click());
    expect(state.openConversation).toHaveBeenCalledWith("other");
    expect(state.resume).not.toHaveBeenCalled();
  });
});

describe("a broken hooks file", () => {
  const message = "/tmp/project/.taurus/hooks.json doesn't parse: x";

  it("opens from Settings in the canvas, with Settings shut", async () => {
    (state.status as { problems: unknown[] }).problems = [{ source: "hooks", message }];
    localStorage.setItem(SETTINGS_TAB, "hooks");
    const host = await mounted();
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: ",", ctrlKey: true, bubbles: true }));
    });
    let button: HTMLButtonElement | undefined;
    for (let i = 0; i < 100 && !button; i += 1) {
      await settle();
      button = [...document.querySelectorAll("button")].find((b) => b.textContent === "Open hooks.json");
    }
    expect(button, "the button never appeared").toBeDefined();
    await act(async () => button!.click());
    await settle();

    expect(host.querySelector('[data-showing="true"]')?.getAttribute("data-canvas")).toBe(
      ".taurus/hooks.json",
    );
    expect(document.querySelector(".drawer-intro")).toBeNull();
    (state.status as { problems: unknown[] }).problems = [];
    localStorage.removeItem(SETTINGS_TAB);
  }, 15_000);

  it("gets what's typed in the canvas to disk before the message goes", async () => {
    const session = { id: "s", provider_id: "ollama", model: "m", native_tools: true, vision: false, context_length: 1 };
    (state as { session: unknown }).session = session;
    state.send.mockReset();
    state.send.mockImplementation(async () => {
      steps.push("send");
    });
    const host = await mounted();
    const box = host.querySelector("textarea") as HTMLTextAreaElement;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(box, "again");
      box.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    });
    await settle();
    (state as { session: unknown }).session = null;
    expect(steps).toEqual(["flush", "send"]);
  });
});

describe("the canvas across a restart", () => {
  const KEY = "taurus.canvasTabs";
  const open = (host: HTMLElement) =>
    [...host.querySelectorAll<HTMLElement>("[data-canvas]")].map((slot) => slot.dataset.canvas);

  it("opens the files this folder had open, and keeps what's opened since", async () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({
        "/tmp/project": { paths: ["README.md", "docs/plan.md"], active: "docs/plan.md", at: 1 },
        "/tmp/next": { paths: ["other.md"], active: "other.md", at: 1 },
      }),
    );
    const host = await mounted();
    // `opening` names src/main.rs, which a turn asked for as the window came up.
    expect(open(host).sort()).toEqual(["README.md", "docs/plan.md", "src/main.rs"]);
    const kept = JSON.parse(localStorage.getItem(KEY)!);
    expect([...kept["/tmp/project"].paths].sort()).toEqual(["README.md", "docs/plan.md", "src/main.rs"]);
    // Another folder's are left alone for when it opens.
    expect(kept["/tmp/next"].paths).toEqual(["other.md"]);
  });
});
