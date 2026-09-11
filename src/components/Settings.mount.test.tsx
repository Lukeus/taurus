// @vitest-environment jsdom
//
// Mounted rather than rendered to a string, because the Behavior tab is behind
// a click and its controls are only interesting for what they write. The
// string-rendered tests in `Settings.test.tsx` never reach this panel.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Declared taking its arguments, so a test can answer one command differently
// from the rest — the provider tab reads three of them on mount.
const invoke = vi.fn((..._args: unknown[]): Promise<unknown> =>
  Promise.resolve([]),
);
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
import { MAX_ITERATIONS_LIMIT } from "../lib/limits";

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
    max_iterations: 25,
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

/** Two saved providers, both complete, as `list_global_providers` reports. */
const SAVED = [
  {
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
  },
  {
    id: "anthropic",
    kind: "anthropic",
    base_url: "https://api.anthropic.com",
    models: [],
    default_model: null,
    api_key_env: null,
    api_key_header: null,
    native_tools: null,
    context_length: null,
    vision: null,
    api_prefix: null,
    thinking: null,
  },
];

/** Answers the provider tab's reads, and everything else with a list. */
const withProviders = (providers: unknown[] = SAVED) => {
  invoke.mockImplementation((...args: unknown[]) => {
    switch (args[0]) {
      case "list_global_providers":
        return Promise.resolve(providers);
      case "keychain_available":
        return Promise.resolve(true);
      default:
        return Promise.resolve([]);
    }
  });
};

const cards = (host: HTMLElement) => [
  ...host.querySelectorAll(".settings-provider"),
];

const folder = (card: Element) =>
  card.querySelector(".settings-provider-fold") as HTMLButtonElement;

const toggle = (card: Element) =>
  act(() =>
    folder(card).dispatchEvent(new MouseEvent("click", { bubbles: true })),
  );

/** Whether the card is showing its body, asked the way a reader is told. */
const isOpen = (card: Element) =>
  folder(card).getAttribute("aria-expanded") === "true";

const baseUrl = (card: Element) => {
  const field = [...card.querySelectorAll("label.settings-field, .settings-field")]
    .find((f) => f.textContent?.includes("Base URL"));
  return field?.querySelector("input") as HTMLInputElement | undefined;
};

const typeInto = (input: HTMLInputElement, text: string) =>
  act(() => {
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    setter.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });

describe("folding a provider card", () => {
  it("opens a saved provider folded, so the tab is a list", async () => {
    // A configured provider is a dozen fields, and a machine with four of them
    // turned this tab into a page of forms to scroll past.
    withProviders();
    const host = await mount();
    expect(cards(host)).toHaveLength(2);
    expect(cards(host).every((c) => !isOpen(c))).toBe(true);
    expect(baseUrl(cards(host)[0])).toBeUndefined();
  });

  it("keeps the id and the kind on the folded row", async () => {
    // What the row is scanned for. A summary that could not be edited where it
    // is read would mean opening a card to rename it.
    withProviders();
    const host = await mount();
    const [first] = cards(host);
    expect(
      (first.querySelector(".settings-id") as HTMLInputElement).value,
    ).toBe("ollama");
    expect(first.querySelector("select")).toBeTruthy();
  });

  it("shows the rest when the disclosure is pressed, and hides it again", async () => {
    withProviders();
    const host = await mount();
    const card = () => cards(host)[0];

    await toggle(card());
    expect(isOpen(card())).toBe(true);
    expect(baseUrl(card())?.value).toBe("http://localhost:11434");

    await toggle(card());
    expect(isOpen(card())).toBe(false);
    expect(baseUrl(card())).toBeUndefined();
  });

  it("folds each card on its own", async () => {
    withProviders();
    const host = await mount();
    await toggle(cards(host)[0]);
    expect(isOpen(cards(host)[0])).toBe(true);
    expect(isOpen(cards(host)[1])).toBe(false);
  });

  it("opens a provider that is not finished", async () => {
    // Which is every provider just added: it has no base URL yet, and folding
    // it away would hide the one field that has to be filled in.
    withProviders([{ ...SAVED[0], base_url: "" }]);
    const host = await mount();
    expect(isOpen(cards(host)[0])).toBe(true);
  });

  it("marks a folded card that is stopping the save", async () => {
    // The hazard folding introduces: a save refused over something out of
    // sight. The mark says which card would fix it.
    withProviders();
    const host = await mount();
    const card = () => cards(host)[0];

    await toggle(card());
    await typeInto(baseUrl(card())!, "");
    await toggle(card());

    const mark = card().querySelector(".dot.error");
    expect(mark).toBeTruthy();
    expect(mark?.getAttribute("aria-label")).toContain("needs a base URL");
  });

  it("leaves a folded card unmarked while it is fine", async () => {
    withProviders();
    const host = await mount();
    expect(cards(host)[0].querySelector(".dot.error")).toBeNull();
  });
});

describe("removing a provider", () => {
  /** Two hosted providers, so both cards have a key field. */
  const HOSTED = [
    { ...SAVED[1], id: "work" },
    { ...SAVED[1], id: "personal" },
  ];

  const keyField = (card: Element) =>
    card.querySelector('input[type="password"]') as HTMLInputElement | null;

  const remove = (card: Element) => {
    const button = [...card.querySelectorAll("button")].find(
      (b) => b.textContent === "Remove",
    );
    if (!button) throw new Error(`no Remove button in: ${card.outerHTML}`);
    return act(() =>
      button.dispatchEvent(new MouseEvent("click", { bubbles: true })),
    );
  };

  it("takes the card's typed key with it rather than handing it to the next", async () => {
    // Each card holds a key typed but not yet stored, and whether it is open,
    // in its own state. Keyed by position, removing the first card hands both
    // to the one that moves up into its place, and the key typed for one
    // provider is then a press of Store away from being filed under another.
    invoke.mockImplementation((...args: unknown[]) => {
      switch (args[0]) {
        case "list_global_providers":
          return Promise.resolve(HOSTED);
        case "keychain_available":
          return Promise.resolve(true);
        case "list_key_statuses":
          return Promise.resolve([
            ["work", { kind: "missing" }],
            ["personal", { kind: "missing" }],
          ]);
        default:
          return Promise.resolve([]);
      }
    });
    const host = await mount();
    await toggle(cards(host)[0]);
    await typeInto(keyField(cards(host)[0])!, "sk-meant-for-work");

    await remove(cards(host)[0]);
    expect(cards(host)).toHaveLength(1);
    const left = cards(host)[0];
    expect((left.querySelector(".settings-id") as HTMLInputElement).value).toBe(
      "personal",
    );
    expect(isOpen(left)).toBe(false);

    await toggle(left);
    expect(keyField(cards(host)[0])?.value).toBe("");
  });
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

/** The steps-per-message field, once the Behavior tab is open. */
const steps = (host: HTMLElement): HTMLInputElement => {
  const input = host.querySelector('input[aria-label="Steps per message"]');
  if (!input) throw new Error(`no steps field in: ${host.innerHTML}`);
  return input as HTMLInputElement;
};

/** Types a value and commits it the way leaving the field would. */
const type = async (input: HTMLInputElement, value: string) => {
  await act(async () => {
    // React tracks the last value it set, so assigning `.value` directly reads
    // as no change. The native setter is what makes the input event land.
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  // `focusout`, not `blur`: React delegates from the root, and `blur` does not
  // bubble, so an `onBlur` handler never sees one dispatched at the element.
  await act(async () => {
    input.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
  });
};

describe("the iteration limit", () => {
  it("shows what settings actually report rather than the default", async () => {
    state.status = { ...status(true, true) };
    (state.status as { settings: { max_iterations: number } }).settings.max_iterations = 40;
    const host = await mount();
    click(host, "Behavior");
    expect(steps(host).value).toBe("40");
  });

  it("writes the new limit and refreshes", async () => {
    const host = await mount();
    click(host, "Behavior");
    await type(steps(host), "40");
    expect(invoke).toHaveBeenCalledWith("set_max_iterations", { limit: 40 });
    expect(refresh).toHaveBeenCalled();
  });

  it("pulls an over-range number back to the ceiling instead of sending it", async () => {
    // The host clamps too, but a field that accepts 900 and then displays 900
    // while the turn runs 50 is lying about what will happen.
    const host = await mount();
    click(host, "Behavior");
    const input = steps(host);
    await type(input, String(MAX_ITERATIONS_LIMIT * 10));
    expect(input.value).toBe(String(MAX_ITERATIONS_LIMIT));
    expect(invoke).toHaveBeenCalledWith("set_max_iterations", {
      limit: MAX_ITERATIONS_LIMIT,
    });
  });

  it("keeps the current limit when the field is left empty", async () => {
    // Clearing the box is on the way to typing a number, not a request for a
    // turn that cannot take a step.
    const host = await mount();
    click(host, "Behavior");
    const input = steps(host);
    await type(input, "");
    expect(input.value).toBe("25");
    expect(invoke).not.toHaveBeenCalledWith(
      "set_max_iterations",
      expect.anything(),
    );
  });

  it("does not write when the number has not changed", async () => {
    const host = await mount();
    click(host, "Behavior");
    await type(steps(host), "25");
    expect(invoke).not.toHaveBeenCalledWith(
      "set_max_iterations",
      expect.anything(),
    );
  });
});

describe("a write that does not take", () => {
  it("says why a revoke failed, rather than leaving the rule there in silence", async () => {
    invoke.mockImplementation((...args: unknown[]) => {
      switch (args[0]) {
        case "list_permission_rules":
          return Promise.resolve([{ rule: "write_file", scope: "global" }]);
        case "revoke_permission_rule":
          return Promise.reject("permissions.json is read-only");
        default:
          return Promise.resolve([]);
      }
    });
    const host = await mount();
    click(host, "Permissions");
    await act(async () => {
      [...host.querySelectorAll("button")]
        .find((b) => b.textContent === "Revoke")!
        .click();
    });

    expect(host.textContent).toContain("permissions.json is read-only");
  });

  it("says why a switch did not move", async () => {
    invoke.mockImplementation((...args: unknown[]) =>
      args[0] === "set_skill_synthesis"
        ? Promise.reject("settings.json is read-only")
        : Promise.resolve([]),
    );
    const host = await mount();
    click(host, "Behavior");
    await flip(checkbox(host, "propose skills"));

    expect(host.textContent).toContain("settings.json is read-only");
  });

  it("says the stored keys could not be read, rather than asking for a saved provider to be saved", async () => {
    // A locked keychain read as "nothing stored", and every saved provider
    // then said to save it before storing a key.
    invoke.mockImplementation((...args: unknown[]) => {
      switch (args[0]) {
        case "list_global_providers":
          return Promise.resolve(SAVED);
        case "keychain_available":
          return Promise.resolve(true);
        case "list_key_statuses":
          return Promise.reject("the keychain is locked");
        default:
          return Promise.resolve([]);
      }
    });
    const host = await mount();
    await toggle(cards(host)[1]);
    const card = cards(host)[1];

    expect(card.textContent).toContain("the keychain is locked");
    expect(card.textContent).not.toContain("Save this provider before storing a key");
  });
});

describe("the search tab", () => {
  const BRAVE = {
    selected: null,
    backends: [
      {
        id: "brave",
        kind: "brave",
        base_url: "https://api.search.brave.com",
        api_key_env: null,
        max_results: null,
        needs_key: true,
      },
    ],
    key_statuses: [],
    active: false,
    problems: [],
  };

  it("says why its settings could not be read, rather than loading forever", async () => {
    invoke.mockImplementation((...args: unknown[]) =>
      args[0] === "get_search_settings"
        ? Promise.reject("search.json is not valid JSON")
        : Promise.resolve([]),
    );
    const host = await mount();
    click(host, "Search");
    await act(async () => {});
    // Which tab is current, said and not only shaded.
    const search = [...host.querySelectorAll('[role="tab"]')].find((t) => t.textContent === "Search");
    expect(search?.getAttribute("aria-selected")).toBe("true");

    expect(host.textContent).toContain("search.json is not valid JSON");
    expect(host.textContent).not.toContain("Loading…");
  });

  it("says why a save did not take", async () => {
    invoke.mockImplementation((...args: unknown[]) => {
      switch (args[0]) {
        case "get_search_settings":
          return Promise.resolve(BRAVE);
        case "save_search_settings":
          return Promise.reject("search.json is read-only");
        default:
          return Promise.resolve([]);
      }
    });
    const host = await mount();
    click(host, "Search");
    await act(async () => {});
    const select = [...host.querySelectorAll("select")].find((s) =>
      s.querySelector('option[value="brave"]'),
    ) as HTMLSelectElement;
    await act(async () => {
      select.value = "brave";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });

    expect(host.textContent).toContain("search.json is read-only");
  });
});

describe("searching the codebase", () => {
  const indexing = (model: string) => {
    const base = status(true, true);
    return { ...base, settings: { ...base.settings, embedding_model: model } };
  };
  const NONE = { selected: null, backends: [], key_statuses: [], active: false, problems: [] };

  it("shows the embedding model the settings say now, not the one it opened with", async () => {
    // Seeded once and never followed, the field kept showing a model that a
    // refresh from elsewhere had already replaced.
    invoke.mockImplementation((...args: unknown[]) =>
      Promise.resolve(args[0] === "get_search_settings" ? NONE : []),
    );
    state.status = indexing("nomic-embed-text");
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => {
      root.render(<Settings onClose={() => {}} />);
    });
    click(host, "Search");
    const field = () =>
      [...host.querySelectorAll("input")].find(
        (i) => i.placeholder === "nomic-embed-text",
      ) as HTMLInputElement;
    expect(field().value).toBe("nomic-embed-text");

    state.status = indexing("mxbai-embed-large");
    await act(async () => {
      root.render(<Settings onClose={() => {}} />);
    });
    expect(field().value).toBe("mxbai-embed-large");
  });

  it("keeps a build it started when the tab is switched away and back", async () => {
    invoke.mockImplementation((...args: unknown[]) => {
      if (args[0] === "build_index") return new Promise(() => {});
      return Promise.resolve(args[0] === "get_search_settings" ? NONE : []);
    });
    state.status = indexing("nomic-embed-text");
    const host = await mount();
    click(host, "Search");
    click(host, "Build index now");
    const stopShown = () =>
      [...host.querySelectorAll("button")].some((b) => b.textContent === "Stop");
    expect(stopShown()).toBe(true);

    click(host, "Behavior");
    click(host, "Search");
    expect(stopShown()).toBe(true);
  });
});
