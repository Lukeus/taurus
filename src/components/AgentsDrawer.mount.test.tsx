// @vitest-environment jsdom
//
// Same reason SkillsDrawer has one of these: rendering to a string only asks
// what the first paint looks like, and the zustand-snapshot bug that kept that
// drawer from opening at all was invisible to every string test in the suite.
// This drawer reads the store the same way, so it gets the same guard.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

/** Whatever `list_agents` should answer for the test being run. */
let roster: unknown[] = [];

/**
 * Enough of `AppStatus` to survive a refresh.
 *
 * The drawer's own refresh ends in the store's, which asks for `get_status` —
 * so a double that answers that with a stand-in resolves the *store* into
 * something no screen can render, and every card crashes on the next paint.
 */
const appStatus = () => ({
  workspace: "/x",
  providers: [],
  settings: { max_iterations: 25, theme: "system" },
  skill_count: 0,
  agent_count: roster.length,
  problems: [],
  tool_names: [],
  mcp_servers: [],
});

/** What the backend answers unless a test says otherwise. */
const answers = (command: string): Promise<unknown> => {
  if (command === "list_agents") return Promise.resolve(roster);
  if (command === "agent_roster_cost") return Promise.resolve(0);
  if (command === "get_status") return Promise.resolve(appStatus());
  if (command === "set_agent_iterations")
    return Promise.resolve("/home/x/.taurus/agents/worker.md");
  return Promise.resolve([]);
};

const invoke = vi.fn(answers);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [string])),
  Channel: class {},
}));

import { AgentsDrawer } from "./AgentsDrawer";
import { useStore } from "../state/store";
import { MAX_ITERATIONS_LIMIT } from "../lib/limits";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mount = (node: React.ReactNode) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
  try {
    act(() => root.render(node));
  } finally {
    quiet.mockRestore();
  }
  return { html: host.innerHTML, unmount: () => act(() => root.unmount()) };
};

afterEach(() => {
  document.body.innerHTML = "";
  // `mockReset` rather than `mockClear`: a test that installs a failing
  // implementation would otherwise leave it installed for every test after it,
  // and the one that noticed would be whichever happened to run next.
  invoke.mockReset();
  invoke.mockImplementation(answers);
  roster = [];
});

/**
 * One agent, as `list_agents` reports it.
 *
 * `forks_on_edit` is derived rather than defaulted, because the backend derives
 * it too: an agent with no file of its own has nowhere to be edited in place. A
 * fixture that let the two disagree would let the warning be tested against a
 * state the backend never produces.
 */
const agent = (over: Record<string, unknown> = {}) => {
  const base = {
    name: "worker",
    description: "does the work",
    tier: "builtin",
    tools: null,
    max_iterations: 20,
    model: null,
    provider: null,
    shadows: null,
    degraded: null,
    path: null,
    ...over,
  };
  return { forks_on_edit: base.path === null, ...base };
};

/**
 * Mounts and waits for the roster to arrive.
 *
 * The sync `mount` above deliberately looks at the first paint. This one is for
 * the controls on a card, which do not exist until `list_agents` resolves.
 */
const mountLoaded = async () => {
  useStore.setState({
    status: { workspace: "/x", settings: { max_iterations: 25 }, problems: [] },
  } as never);
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => {
    createRoot(host).render(<AgentsDrawer onClose={() => {}} />);
  });
  return host;
};

const iterations = (host: HTMLElement, name: string): HTMLInputElement => {
  const input = host.querySelector(`input[aria-label="Max iterations for ${name}"]`);
  if (!input) throw new Error(`no field for ${name} in: ${host.innerHTML}`);
  return input as HTMLInputElement;
};

/** Types a value and commits it the way leaving the field would. */
const type = async (input: HTMLInputElement, value: string) => {
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  // `focusout`, not `blur`: React delegates from the root and `blur` does not
  // bubble, so an `onBlur` handler never sees one dispatched at the element.
  await act(async () => {
    input.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
  });
};

describe("retuning an agent from the roster", () => {
  it("writes the new limit through the in-place command", async () => {
    // Not `save_agent`: that rebuilds a file from a proposal and would drop a
    // hand-written `model:`.
    roster = [agent({ path: "/x/.taurus/agents/reviewer.md", name: "reviewer" })];
    const host = await mountLoaded();
    await type(iterations(host, "reviewer"), "40");

    expect(invoke).toHaveBeenCalledWith("set_agent_iterations", {
      name: "reviewer",
      limit: 40,
    });
    expect(invoke).not.toHaveBeenCalledWith("save_agent", expect.anything());
  });

  it("pulls an over-range number back to the ceiling before sending it", async () => {
    roster = [agent()];
    const host = await mountLoaded();
    const field = iterations(host, "worker");
    await type(field, String(MAX_ITERATIONS_LIMIT * 10));

    expect(field.value).toBe(String(MAX_ITERATIONS_LIMIT));
    expect(invoke).toHaveBeenCalledWith("set_agent_iterations", {
      name: "worker",
      limit: MAX_ITERATIONS_LIMIT,
    });
  });

  it("does not write when the number has not moved", async () => {
    roster = [agent()];
    const host = await mountLoaded();
    await type(iterations(host, "worker"), "20");

    expect(invoke).not.toHaveBeenCalledWith(
      "set_agent_iterations",
      expect.anything(),
    );
  });

  it("says a built-in will be copied before it is", async () => {
    // Editing one writes a user-tier file that did not exist. Finding that out
    // afterwards, from a directory you did not know you had, is worse than
    // reading one clause first.
    roster = [agent()];
    const host = await mountLoaded();
    expect(host.textContent).toMatch(/saves a copy you own/i);
  });

  it("says a borrowed file will be copied rather than changed", async () => {
    // A Copilot agent has a file, so "has a file" is not the test — it is
    // whose file it is. Rewriting it would drop the frontmatter keys Copilot
    // has and Taurus does not, out of a file that is usually committed.
    roster = [
      agent({
        name: "reviewer",
        tier: "project",
        path: "/x/.github/agents/reviewer.agent.md",
        forks_on_edit: true,
      }),
    ];
    const host = await mountLoaded();
    expect(host.textContent).toMatch(/saves a copy you own/i);
    expect(host.textContent).toMatch(/shadows the original/i);
    expect(host.textContent).not.toMatch(/shadows the built-in/i);
  });

  it("does not warn about copies for an agent that has a file", async () => {
    roster = [agent({ path: "/x/.taurus/agents/reviewer.md", name: "reviewer" })];
    const host = await mountLoaded();
    expect(host.textContent).not.toMatch(/saves a copy you own/i);
  });

  it("puts the number back when the write fails", async () => {
    // A field showing a limit the file does not have is worse than one that
    // never moved.
    roster = [agent()];
    invoke.mockImplementation((command: string) => {
      if (command === "list_agents") return Promise.resolve(roster);
      if (command === "agent_roster_cost") return Promise.resolve(0);
      if (command === "get_status") return Promise.resolve(appStatus());
      if (command === "set_agent_iterations")
        return Promise.reject(new Error("read-only filesystem"));
      return Promise.resolve([]);
    });

    const host = await mountLoaded();
    const field = iterations(host, "worker");
    await type(field, "45");

    expect(field.value).toBe("20");
    expect(host.textContent).toMatch(/read-only filesystem/);
  });

  it("names the conversation's own limit beside the agents'", async () => {
    // Two numbers that read as one concept. Kept apart, someone raises an
    // agent's ceiling and wonders why the turn that spawned it still stops.
    roster = [agent()];
    const host = await mountLoaded();
    expect(host.textContent).toMatch(/25 steps of its own/);
  });
});

describe("opening the agents drawer", () => {
  it("mounts and stays mounted with nothing loaded yet", () => {
    useStore.setState({ status: null });

    const { html, unmount } = mount(<AgentsDrawer onClose={() => {}} />);
    expect(html).toContain("Agents");
    // The shadowing rule, which is the one thing about this list that cannot
    // be worked out by looking at it.
    expect(html).toContain("overrides a built-in");
    // And a way in for someone who has never written the frontmatter.
    expect(html).toContain("New agent");
    unmount();
  });

  it("shows only this drawer's problems once a status has arrived", () => {
    useStore.setState({
      status: {
        workspace: "/Users/x/code/taurus",
        // The drawer names the parent turn's limit beside the per-agent ones,
        // so a status double without settings is not one this drawer could
        // ever be handed.
        settings: { max_iterations: 25 },
        problems: [
          { source: "agents", message: "reviewer.md: rename one to match" },
          { source: "skills", message: "not this drawer's problem" },
        ],
      } as never,
    });

    const { html, unmount } = mount(<AgentsDrawer onClose={() => {}} />);
    expect(html).toContain("rename one to match");
    // The skills drawer owns skill failures; this one must not repeat them.
    expect(html).not.toContain("not this drawer's problem");
    unmount();
  });
});
