// @vitest-environment jsdom
//
// Each row saves the whole set of routes, and a row only becomes a route once
// it has a model.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

const refresh = vi.fn(() => Promise.resolve());
const state: { status: unknown; refresh: () => Promise<void> } = { status: null, refresh };
vi.mock("../../state/store", () => ({
  useStore: (select: (s: typeof state) => unknown) => select(state),
}));

import { RoutingTab } from "./RoutingTab";
import type { ModelRoutes } from "../../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const status = (routes: ModelRoutes, problems: unknown[] = []) => ({
  providers: [{ id: "ollama" }, { id: "gemini" }],
  problems,
  settings: { routes, from_project: [] },
});

const AGENTS = [
  { name: "explorer", description: "Reads around.", model: null, provider: null },
  { name: "deep", description: "Thinks.", model: "gpt-5", provider: "gateway" },
];

beforeEach(() => {
  state.status = status({ agents: {} });
  invoke.mockImplementation((command: string) => {
    if (command === "list_agents") return Promise.resolve(AGENTS);
    if (command === "list_models") return Promise.resolve([{ id: "gemini-flash" }]);
    return Promise.resolve();
  });
});

afterEach(() => {
  document.body.innerHTML = "";
  invoke.mockReset();
});

const mount = async () => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => createRoot(host).render(<RoutingTab />));
  return host;
};

const field = <T extends HTMLElement>(host: HTMLElement, label: string) => {
  const el = host.querySelector(`[aria-label="${label}"]`);
  if (!el) throw new Error(`no ${label} in: ${host.innerHTML}`);
  return el as T;
};

/** Sets a controlled field's value the way typing does. */
const type = async (el: HTMLInputElement | HTMLSelectElement, value: string) => {
  const proto = el instanceof HTMLSelectElement ? HTMLSelectElement : HTMLInputElement;
  const setter = Object.getOwnPropertyDescriptor(proto.prototype, "value")!.set!;
  await act(async () => {
    setter.call(el, value);
    el.dispatchEvent(new Event(el instanceof HTMLSelectElement ? "change" : "input", { bubbles: true }));
  });
};

const blur = async (el: HTMLElement) => {
  await act(async () => {
    el.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
  });
};

const saved = () =>
  invoke.mock.calls.filter(([command]) => command === "set_routes").map(([, args]) => args.routes);

describe("the routing tab", () => {
  it("saves a route once it has a model, not when only a provider is picked", async () => {
    const host = await mount();
    await type(field(host, "Drafts provider"), "gemini");
    expect(saved()).toEqual([]);

    const model = field<HTMLInputElement>(host, "Drafts model");
    await type(model, "gemini-flash");
    await blur(model);
    expect(saved()).toEqual([
      { agents: {}, utility: { provider: "gemini", model: "gemini-flash" } },
    ]);
    expect(refresh).toHaveBeenCalled();
  });

  it("offers the chosen provider's models", async () => {
    const host = await mount();
    await type(field(host, "Reviews provider"), "gemini");
    expect(invoke).toHaveBeenCalledWith("list_models", { providerId: "gemini" });
    expect(host.querySelector("datalist option")?.getAttribute("value")).toBe("gemini-flash");
  });

  it("routes one sub-agent by name, beside the others", async () => {
    state.status = status({ agents: { deep: { provider: "ollama", model: "qwen3:32b" } } });
    const host = await mount();
    const model = field<HTMLInputElement>(host, "explorer model");
    await type(model, "qwen3:8b");
    await blur(model);
    expect(saved()).toEqual([
      {
        agents: {
          deep: { provider: "ollama", model: "qwen3:32b" },
          explorer: { provider: "", model: "qwen3:8b" },
        },
      },
    ]);
  });

  it("says an agent's own file wins while it can run", async () => {
    const host = await mount();
    expect(host.textContent).toContain("Its file names gpt-5 on gateway, which wins");
  });

  it("shows why a route isn't in use, and why a save was refused", async () => {
    state.status = status({ agents: {} }, [
      { source: "routes", message: "the review route sends work to gemini/pro, but it has no key" },
      { source: "providers", message: "not this one" },
    ]);
    invoke.mockImplementation((command: string) => {
      if (command === "list_agents") return Promise.resolve([]);
      if (command === "set_routes") return Promise.reject("the delegate route names provider 'x'");
      return Promise.resolve();
    });
    const host = await mount();
    expect(host.textContent).toContain("gemini/pro, but it has no key");
    expect(host.textContent).not.toContain("not this one");

    const model = field<HTMLInputElement>(host, "Sub-agents model");
    await type(model, "m");
    await blur(model);
    expect(host.textContent).toContain("names provider 'x'");
  });
});
