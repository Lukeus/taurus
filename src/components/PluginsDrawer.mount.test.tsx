// @vitest-environment jsdom
//
// The drawer reads its list in an effect and redraws from what each change
// answers with, so what's worth checking happens after the first paint.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { PluginsDrawer } from "./PluginsDrawer";
import type { PluginSummary } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const plugin = (overrides: Partial<PluginSummary> = {}): PluginSummary => ({
  name: "ops",
  scope: "global",
  root: "/home/me/.taurus/plugins/ops",
  version: "1.2.0",
  description: "Deploys things",
  enabled: true,
  shadowed: false,
  skills: ["deploy"],
  agents: ["reviewer"],
  mcp_servers: ["db"],
  hooks: [],
  unsupported: [
    {
      part: "commands/",
      reason: "Taurus has no command files; a skill marked user-invocable is its /command",
    },
  ],
  problems: [],
  warnings: [],
  source: { from: "https://example.invalid/ops.git", commit: "0123456789abcdef0123" },
  ...overrides,
});

let cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  invoke.mockReset();
});

async function mount(props: Partial<Parameters<typeof PluginsDrawer>[0]> = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<PluginsDrawer busy={false} trusted onClose={() => {}} {...props} />);
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  const button = (label: string) =>
    [...host.querySelectorAll("button")].find((b) => b.textContent === label);
  return {
    host,
    text: () => host.textContent ?? "",
    button,
    click: async (label: string) => {
      const found = button(label);
      if (!found) throw new Error(`no button reading "${label}"`);
      await act(async () => found.click());
    },
  };
}

/** Answers the list, and each change with `after`. */
function backend(list: PluginSummary[], after: PluginSummary[] = list) {
  invoke.mockImplementation((command: string) =>
    Promise.resolve(command === "list_plugins" ? list : after),
  );
}

describe("the plugins drawer", () => {
  it("shows what each plugin brings, and names what isn't run here", async () => {
    backend([plugin()]);
    const drawer = await mount();
    expect(drawer.text()).toContain("ops");
    expect(drawer.text()).toContain("deploy");
    expect(drawer.text()).toContain("reviewer");
    expect(drawer.text()).toContain("not run here");
    expect(drawer.text()).toContain("commands/");
    expect(drawer.text()).toContain("at 0123456789ab");
  });

  it("switches a plugin off in the scope it lives in", async () => {
    backend([plugin({ scope: "workspace" })], [plugin({ scope: "workspace", enabled: false })]);
    const drawer = await mount();
    await drawer.click("Switch off");
    expect(invoke).toHaveBeenCalledWith("set_plugin_enabled", {
      name: "ops",
      project: true,
      enabled: false,
    });
    expect(drawer.button("Switch on")).toBeTruthy();
  });

  it("removes only on a second press", async () => {
    backend([plugin()], []);
    const drawer = await mount();
    await drawer.click("Remove");
    expect(invoke).not.toHaveBeenCalledWith("remove_plugin", expect.anything());
    await drawer.click("Remove it");
    expect(invoke).toHaveBeenCalledWith("remove_plugin", { name: "ops", project: false });
    expect(drawer.text()).toContain("No plugins installed");
  });

  it("adds from a git URL with the ref it was given", async () => {
    backend([], [plugin()]);
    const drawer = await mount();
    const [from] = drawer.host.querySelectorAll("input");
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
      setter.call(from, "https://example.invalid/ops.git");
      from.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const ref = drawer.host.querySelector<HTMLInputElement>('input[aria-label="Branch or tag"]')!;
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
      setter.call(ref, "v1");
      ref.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await drawer.click("Add");
    expect(invoke).toHaveBeenCalledWith("add_plugin", {
      from: "https://example.invalid/ops.git",
      project: false,
      gitRef: "v1",
    });
    expect(drawer.text()).toContain("Deploys things");
  });

  it("says a project plugin waits for trust, and a broken one why", async () => {
    backend([
      plugin({ scope: "workspace" }),
      plugin({ name: "bad", problems: ["the plugin name \"Bad\" isn't usable"] }),
    ]);
    const drawer = await mount({ trusted: false });
    expect(drawer.text()).toContain("waiting for trust");
    expect(drawer.text()).toContain("doesn't load");
    expect(drawer.text()).toContain("isn't usable");
  });

  it("changes nothing while a turn is running", async () => {
    backend([plugin()]);
    const drawer = await mount({ busy: true });
    expect(drawer.button("Switch off")?.disabled).toBe(true);
    expect(drawer.button("Remove")?.disabled).toBe(true);
  });

  it("says why a change was refused", async () => {
    invoke.mockImplementation((command: string) =>
      command === "list_plugins"
        ? Promise.resolve([plugin()])
        : Promise.reject("git couldn't fetch https://example.invalid/ops.git"),
    );
    const drawer = await mount();
    await drawer.click("Update");
    expect(drawer.text()).toContain("git couldn't fetch");
  });
});
