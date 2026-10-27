// @vitest-environment jsdom
//
// Switching off asks where only when there's a choice, and switching on goes
// to the layer that has the hook off.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

const state: { status: unknown; trust: unknown } = { status: null, trust: null };
vi.mock("../../state/store", () => ({
  useStore: (select: (s: typeof state) => unknown) => select(state),
}));

import { HooksTab, offBy, scopeOf } from "./HooksTab";
import type { HookView } from "../../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const view = (name: string, patch: Partial<HookView> = {}): HookView => ({
  hook: {
    name,
    on: "pre_tool_use",
    command: "./scripts/no-force-push.sh",
    matches: "run_command; commands starting git",
    timeout_seconds: 30 as unknown as bigint,
  },
  defined_in: { kind: "global" },
  decided_by: { kind: "global" },
  enabled: true,
  ...patch,
});

let hooks: HookView[];
beforeEach(() => {
  state.status = { problems: [] };
  state.trust = { trusted: true };
  hooks = [
    view("no-force-push"),
    view("fmt", {
      defined_in: { kind: "workspace" },
      decided_by: { kind: "workspace" },
    }),
    view("lint:check", {
      defined_in: { kind: "plugin", name: "lint" },
      decided_by: { kind: "plugin", name: "lint" },
      enabled: false,
    }),
  ];
  invoke.mockImplementation((command: string) =>
    command === "hook_list" ? Promise.resolve(hooks) : Promise.resolve(),
  );
});

afterEach(() => {
  document.body.innerHTML = "";
  invoke.mockReset();
});

const mount = async () => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => createRoot(host).render(<HooksTab />));
  return host;
};

const card = (host: HTMLElement, name: string) => {
  const found = [...host.querySelectorAll(".hook-card")].find(
    (c) => c.querySelector(".card-title")?.textContent === name,
  );
  if (!found) throw new Error(`no ${name} in: ${host.innerHTML}`);
  return found as HTMLElement;
};

const press = async (within: HTMLElement, label: string) => {
  const button = [...within.querySelectorAll("button")].find((b) => b.textContent === label);
  if (!button) throw new Error(`no ${label} in: ${within.innerHTML}`);
  await act(async () => button.click());
};

describe("the hooks tab", () => {
  it("says what each hook runs, when, and whose it is", async () => {
    const host = await mount();
    const guard = card(host, "no-force-push");
    expect(guard.textContent).toContain("before a tool call, for run_command; commands starting git");
    expect(guard.textContent).toContain("./scripts/no-force-push.sh");
    expect(guard.textContent).toContain("yours");
    expect(card(host, "fmt").textContent).toContain("this project");
    expect(card(host, "lint:check").textContent).toContain("plugin lint");
  });

  it("asks where before switching off one of yours in a trusted project", async () => {
    const host = await mount();
    await press(card(host, "no-force-push"), "Switch off");
    expect(invoke).not.toHaveBeenCalledWith("set_hook", expect.anything());

    await press(card(host, "no-force-push"), "Off in this project");
    expect(invoke).toHaveBeenCalledWith("set_hook", {
      name: "no-force-push",
      scope: "workspace",
      enabled: false,
    });
  });

  it("switches off everywhere without asking when the project's file isn't read", async () => {
    state.trust = { trusted: false };
    const host = await mount();
    await press(card(host, "no-force-push"), "Switch off");
    expect(invoke).toHaveBeenCalledWith("set_hook", {
      name: "no-force-push",
      scope: "global",
      enabled: false,
    });
  });

  it("switches a project's own hook off in the project's file", async () => {
    const host = await mount();
    await press(card(host, "fmt"), "Switch off");
    expect(invoke).toHaveBeenCalledWith("set_hook", {
      name: "fmt",
      scope: "workspace",
      enabled: false,
    });
  });

  it("says why one is off, and switches it on over a plugin's default from yours", async () => {
    const host = await mount();
    const lint = card(host, "lint:check");
    expect(lint.textContent).toContain("plugin lint ships it switched off");

    await press(lint, "Switch on");
    expect(invoke).toHaveBeenCalledWith("set_hook", {
      name: "lint:check",
      scope: "global",
      enabled: true,
    });
  });

  it("says why a switch didn't take", async () => {
    invoke.mockImplementation((command: string) =>
      command === "hook_list"
        ? Promise.resolve(hooks)
        : Promise.reject(".taurus/hooks.json doesn't parse"),
    );
    const host = await mount();
    await press(card(host, "fmt"), "Switch off");
    expect(host.textContent).toContain("fmt stayed on: .taurus/hooks.json doesn't parse");
  });

  it("says where to add one when there are none", async () => {
    hooks = [];
    const host = await mount();
    expect(host.textContent).toContain("~/.taurus/hooks.json");
  });

  it("puts hooks that won't load at the top", async () => {
    state.status = { problems: [{ source: "hooks", message: "hook 'bad' has no command" }] };
    const host = await mount();
    expect(host.textContent).toContain("hook 'bad' has no command");
  });
});

describe("which layer a switch goes to", () => {
  it("is the one that has it off, or yours for a plugin's", () => {
    expect(scopeOf({ kind: "workspace" })).toBe("workspace");
    expect(scopeOf({ kind: "global" })).toBe("global");
    expect(scopeOf({ kind: "plugin", name: "x" })).toBe("global");
    expect(offBy({ kind: "global" })).toContain("~/.taurus/hooks.json");
  });
});
