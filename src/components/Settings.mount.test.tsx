// @vitest-environment jsdom
//
// Mounted rather than rendered to a string, because the Behavior tab is behind
// a click and its controls are only interesting for what they write. The
// string-rendered tests in `Settings.test.tsx` never reach this panel.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(() => Promise.resolve([]));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));

const refresh = vi.fn(() => Promise.resolve());
const state: { status: unknown; refresh: () => Promise<void> } = {
  status: null,
  refresh,
};
// Settings reads the store through selectors, so the double has to answer a
// selector rather than be one.
vi.mock("../state/store", () => ({
  useStore: (select: (s: typeof state) => unknown) => select(state),
}));

import { Settings } from "./Settings";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

/** Enough of `AppStatus` for the Behavior tab, with the two toggles set. */
const status = (skills: boolean, agents: boolean) => ({
  workspace: "/tmp/project",
  providers: [],
  effective_providers: [],
  skills: [],
  agents: [],
  mcp_servers: [],
  problems: [],
  settings: {
    last_workspace: null,
    last_provider: null,
    last_model: null,
    skill_synthesis_enabled: skills,
    agent_synthesis_enabled: agents,
    disabled_tools: [],
    theme: "system",
  },
});

/**
 * Async because mounting is: an effect asks Rust for the key statuses, and
 * those answers land after a synchronous `act` has already returned.
 */
const mount = async () => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => {
    createRoot(host).render(<Settings onClose={() => {}} />);
  });
  return host;
};

const click = (host: HTMLElement, label: string) => {
  const button = [...host.querySelectorAll("button")].find(
    (b) => b.textContent === label,
  );
  if (!button) throw new Error(`no ${label} button in: ${host.innerHTML}`);
  act(() => button.dispatchEvent(new MouseEvent("click", { bubbles: true })));
};

/** The checkbox whose label contains `text`. */
const checkbox = (host: HTMLElement, text: string): HTMLInputElement => {
  const label = [...host.querySelectorAll("label.settings-check")].find((l) =>
    l.textContent?.includes(text),
  );
  if (!label) throw new Error(`no "${text}" toggle in: ${host.innerHTML}`);
  const input = label.querySelector("input[type=checkbox]");
  if (!input) throw new Error(`"${text}" has no checkbox`);
  return input as HTMLInputElement;
};

/**
 * Flips a checkbox and waits for what the flip set off.
 *
 * `click()` rather than assigning `checked` and dispatching `change`: React
 * tracks a checkbox through the click event and compares against its own last
 * known value, so an assigned `checked` reads as no change and the handler
 * never runs.
 *
 * Async because the handler is — it awaits the write, then a refresh — and the
 * state update lands after a synchronous `act` has already returned.
 */
const flip = async (input: HTMLInputElement) => {
  await act(async () => {
    input.click();
  });
};

beforeEach(() => {
  invoke.mockClear();
  refresh.mockClear();
  state.status = status(true, true);
  window.matchMedia = vi.fn(() => ({
    matches: true,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("the synthesis toggles", () => {
  it("offers skills and sub-agents as separate switches", async () => {
    // One capability is a procedure the model follows and the other is a worker
    // it hands a task to. Wanting one is no reason to want the other, so a
    // single switch for both would be the wrong control.
    const host = await mount();
    click(host, "Behavior");
    expect(checkbox(host, "propose skills")).toBeTruthy();
    expect(checkbox(host, "propose sub-agents")).toBeTruthy();
  });

  it("shows each switch in the state settings actually report", async () => {
    state.status = status(true, false);
    const host = await mount();
    click(host, "Behavior");
    expect(checkbox(host, "propose skills").checked).toBe(true);
    expect(checkbox(host, "propose sub-agents").checked).toBe(false);
  });

  it("writes the sub-agent setting through its own command", async () => {
    // Reaching `set_skill_synthesis` from here would silently move the wrong
    // switch, and both render identically.
    const host = await mount();
    click(host, "Behavior");
    await flip(checkbox(host, "propose sub-agents"));

    expect(invoke).toHaveBeenCalledWith("set_agent_synthesis", {
      enabled: false,
    });
    expect(invoke).not.toHaveBeenCalledWith(
      "set_skill_synthesis",
      expect.anything(),
    );
  });

  it("leaves the sub-agent setting alone when the skill switch moves", async () => {
    const host = await mount();
    click(host, "Behavior");
    await flip(checkbox(host, "propose skills"));

    expect(invoke).toHaveBeenCalledWith("set_skill_synthesis", {
      enabled: false,
    });
    expect(invoke).not.toHaveBeenCalledWith(
      "set_agent_synthesis",
      expect.anything(),
    );
  });

  it("defaults a switch to on when settings have not loaded yet", async () => {
    // The drawer can be opened before `get_status` resolves. Showing an
    // unloaded toggle as off would misreport a capability that is on.
    state.status = null;
    const host = await mount();
    click(host, "Behavior");
    expect(checkbox(host, "propose sub-agents").checked).toBe(true);
  });

  it("says a proposed agent can never exceed the tools you have", async () => {
    // The one fact that makes this safe to leave on, and it is not something a
    // user can infer from the label.
    const host = await mount();
    click(host, "Behavior");
    expect(host.textContent).toMatch(/never be given a tool you do not have/i);
  });
});
