// What the store does with a status: taking one that was pushed, and not asking
// for one when the window comes back. Tauri is stood in for.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import type { AppStatus } from "../lib/api";
import { settle, useStore } from "./store";

const status = (patch: Record<string, unknown> = {}): AppStatus =>
  ({
    workspace: "/src/a",
    providers: [{ id: "local" }],
    settings: { theme: "system" },
    skill_count: 3,
    note_count: 0,
    agent_count: 2,
    dataset_count: 0,
    problems: [],
    tool_names: ["read_file"],
    mcp_servers: [],
    theme: null,
    branch: "main",
    ...patch,
  }) as unknown as AppStatus;

describe("a pushed status", () => {
  it("that moved nothing leaves the held one in place", () => {
    // Every drawer reading `status` redraws when the object changes, and most
    // pushes change nothing any of them shows.
    const held = status();
    expect(settle(held, status())).toBe(held);
  });

  it("that moved one field keeps every other field it held", () => {
    const held = status();
    const next = settle(held, status({ note_count: 1 }));

    expect(next).not.toBe(held);
    expect(next.note_count).toBe(1);
    expect(next.providers).toBe(held.providers);
    expect(next.settings).toBe(held.settings);
  });

  it("arriving first is taken as it is", () => {
    const pushed = status();
    expect(settle(null, pushed)).toBe(pushed);
  });
});

describe("returning to the window", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === "workspace_trust" ? { trusted: true } : []),
    );
  });

  it("asks the trust question, and not for the whole status", async () => {
    // The status is pushed whenever any part of it moves. Asking for it on
    // every focus rebuilt all of it — git and the theme included — to say what
    // the window already showed.
    await useStore.getState().recheck();

    const asked = invoke.mock.calls.map(([command]) => command);
    expect(asked).toContain("workspace_trust");
    expect(asked).not.toContain("get_status");
    expect(useStore.getState().trust).toEqual({ trusted: true });
  });
});
