// @vitest-environment jsdom
//
// The switches write the whole list, and what they show afterwards is read
// back from the host rather than assumed.
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

import { groupByServer, ToolsTab } from "./ToolsTab";
import type { ToolSwitch } from "../../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SWITCHES: ToolSwitch[] = [
  { name: "read_file", tokens: 180, enabled: true },
  { name: "run_command", tokens: 420, enabled: true },
  { name: "update_plan", enabled: true },
  { name: "mcp__notes__search", server: "notes", tokens: 900, enabled: true },
];

const status = (fromProject: string[] = [], problems: unknown[] = []) => ({
  tool_names: ["read_file", "run_command", "mcp__notes__search"],
  problems,
  settings: { disabled_tools: [], from_project: fromProject },
});

let switches: ToolSwitch[];
beforeEach(() => {
  switches = SWITCHES;
  state.status = status();
  invoke.mockImplementation((command: string, args?: { names: string[] }) => {
    if (command === "tool_switches") return Promise.resolve(switches);
    if (command === "set_disabled_tools") {
      switches = switches.map((s) => ({ ...s, enabled: !args!.names.includes(s.name) }));
      return Promise.resolve();
    }
    return Promise.resolve([]);
  });
});

afterEach(() => {
  document.body.innerHTML = "";
  invoke.mockReset();
});

const mount = async () => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => createRoot(host).render(<ToolsTab />));
  return host;
};

const box = (host: HTMLElement, name: string) => {
  const row = [...host.querySelectorAll("label.tool-switch")].find((l) =>
    l.textContent?.includes(name),
  );
  if (!row) throw new Error(`no ${name} in: ${host.innerHTML}`);
  return row.querySelector("input") as HTMLInputElement;
};

describe("the tools tab", () => {
  it("files Taurus's tools first and each server's under its name", async () => {
    const host = await mount();
    const headings = [...host.querySelectorAll(".tools-group .micro")].map((m) => m.textContent);
    expect(headings).toEqual(["Taurus", "MCP · notes"]);
    // Shown without the namespace its group already says.
    expect(box(host, "search").closest("label")?.textContent).not.toContain("mcp__");
  });

  it("totals what the tools that are on cost on every request", async () => {
    const host = await mount();
    expect(host.querySelector(".tools-summary")?.textContent).toContain("4 of 4 on");
    expect(host.querySelector(".tools-summary")?.textContent).toContain("1.5k");
  });

  it("writes the whole list when one is switched off, and shows what comes back", async () => {
    const host = await mount();
    await act(async () => box(host, "run_command").click());

    expect(invoke).toHaveBeenCalledWith("set_disabled_tools", { names: ["run_command"] });
    expect(box(host, "run_command").checked).toBe(false);
    expect(box(host, "run_command").closest("label")?.textContent).toContain("off");
  });

  it("says why a switch didn't take, and puts it back", async () => {
    const answer = invoke.getMockImplementation()!;
    invoke.mockImplementation((command: string, args?: unknown) =>
      command === "set_disabled_tools"
        ? Promise.reject("settings.json doesn't parse")
        : answer(command, args),
    );
    const host = await mount();
    await act(async () => box(host, "read_file").click());

    expect(host.textContent).toContain("read_file stayed on: settings.json doesn't parse");
    expect(box(host, "read_file").checked).toBe(true);
  });

  it("shows the project's list without offering to copy it into yours", async () => {
    state.status = status(["disabled_tools"]);
    const host = await mount();
    expect(host.textContent).toContain("chooses which tools are off");
    expect(box(host, "read_file").disabled).toBe(true);
  });

  it("puts a name that matches no tool at the top", async () => {
    state.status = status([], [{ source: "tools", message: "settings.json disables 'red_file'" }]);
    const host = await mount();
    expect(host.textContent).toContain("disables 'red_file'");
  });
});

describe("grouping", () => {
  it("keeps Taurus's own first whatever the order", () => {
    const groups = groupByServer([
      { name: "mcp__b__x", server: "b", enabled: true },
      { name: "z", enabled: true },
      { name: "mcp__a__y", server: "a", enabled: true },
    ]);
    expect(groups.map(([server]) => server)).toEqual([null, "a", "b"]);
  });
});
