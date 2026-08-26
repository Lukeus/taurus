// @vitest-environment jsdom
//
// Mounted rather than rendered to a string: everything worth checking here is
// keyboard behaviour, a debounced fetch, or what happens on a click.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { CommandPalette } from "./CommandPalette";
import type { SearchResults, SessionMeta } from "../lib/api";
import type { Action } from "../lib/palette";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// jsdom has no layout, so it has no scrolling either. Same stub the transcript
// tests use.
Element.prototype.scrollIntoView = () => {};

const session = (id: string, title: string): SessionMeta => ({
  id,
  workspace: "/w",
  model: "m",
  started: 1_700_000_000,
  updated: 1_700_000_000,
  title,
  branch: undefined,
  agent: undefined,
});

const results = (patch: Partial<SearchResults> = {}): SearchResults => ({
  sessions: [
    {
      session: session("old", "Something else entirely"),
      hits: 4,
      matches: [
        { message: 3, role: "user", excerpt: "fix the trust banner", from: 8, to: 20 },
      ],
    },
  ],
  more: 0,
  ...patch,
});

let cleanup: (() => void)[] = [];
beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }));
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  invoke.mockReset();
  vi.useRealTimers();
});

async function mount(over: Partial<Parameters<typeof CommandPalette>[0]> = {}) {
  const ran: string[] = [];
  const opened: [string, string | null][] = [];
  const actions: Action[] = [
    { id: "new", label: "New conversation", group: "Do", shortcut: "Ctrl+N", run: () => ran.push("new") },
    { id: "stop", label: "Stop this turn", group: "Do", unavailable: "Nothing is running", run: () => ran.push("stop") },
    { id: "changes", label: "Changes", group: "Panels", keywords: "undo rewind", run: () => ran.push("changes") },
  ];

  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <CommandPalette
        actions={actions}
        sessions={[session("a", "Adding a chart")]}
        onOpenSession={(id, find) => opened.push([id, find])}
        onClose={() => {}}
        {...over}
      />,
    );
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });

  const input = host.querySelector(".palette-input") as HTMLInputElement;
  return {
    host,
    ran,
    opened,
    text: () => host.textContent ?? "",
    rows: () => [...host.querySelectorAll(".palette-row")] as HTMLButtonElement[],
    labels: () =>
      [...host.querySelectorAll(".palette-label")].map((e) => e.textContent),
    active: () =>
      host.querySelector('[data-active="true"] .palette-label')?.textContent,
    type: async (value: string) => {
      await act(async () => {
        const setter = Object.getOwnPropertyDescriptor(
          HTMLInputElement.prototype,
          "value",
        )!.set!;
        setter.call(input, value);
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
    },
    press: async (key: string, init: KeyboardEventInit = {}) => {
      await act(async () => {
        input.dispatchEvent(
          new KeyboardEvent("keydown", { key, bubbles: true, ...init }),
        );
      });
    },
    settle: async () => {
      await act(async () => {
        vi.advanceTimersByTime(300);
      });
      await act(async () => {});
    },
  };
}

describe("the command palette", () => {
  it("opens on everything, in the order the groups are meant to be read", async () => {
    const ui = await mount();
    expect(ui.labels()).toEqual([
      "New conversation",
      "Stop this turn",
      "Changes",
      "Adding a chart",
    ]);
    // And the first row is the one Enter belongs to.
    expect(ui.active()).toBe("New conversation");
  });

  it("shows the key that also does a thing, which is how anybody learns it", async () => {
    expect((await mount()).text()).toContain("Ctrl+N");
  });

  it("keeps a command that cannot run, and says why", async () => {
    // A command that disappears when it is unavailable teaches that it does
    // not exist; one that says why teaches what it needs.
    const ui = await mount();
    expect(ui.text()).toContain("Nothing is running");
    const stop = ui.rows().find((r) => r.textContent?.startsWith("Stop"))!;
    expect(stop.disabled).toBe(true);
    await act(async () => stop.click());
    expect(ui.ran).toEqual([]);
  });

  it("narrows as you type and runs what is highlighted", async () => {
    const ui = await mount();
    await ui.type("chan");
    expect(ui.active()).toBe("Changes");
    await ui.press("Enter");
    expect(ui.ran).toEqual(["changes"]);
  });

  it("walks the list with the arrow keys and wraps", async () => {
    const ui = await mount();
    await ui.press("ArrowDown");
    expect(ui.active()).toBe("Stop this turn");
    await ui.press("ArrowUp");
    await ui.press("ArrowUp");
    // Wrapped past the top to the bottom, which is the whole reason to wrap:
    // the last row is one key away rather than four.
    expect(ui.active()).toBe("Adding a chart");
  });

  it("does not read any transcript for a single character", async () => {
    // One letter matches nearly every conversation, which is a list that costs
    // a disk read per entry to produce and tells you nothing.
    const ui = await mount();
    await ui.type("t");
    await ui.settle();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("searches transcripts once typing pauses, and shows the hit", async () => {
    invoke.mockResolvedValue(results());
    const ui = await mount();
    await ui.type("trust banner");
    await ui.settle();

    expect(invoke).toHaveBeenCalledWith("search_sessions", {
      query: "trust banner",
      everywhere: false,
    });
    expect(ui.text()).toContain("Something else entirely");
    expect(ui.text()).toContain("fix the trust banner");
    // Every hit is counted even though one is shown.
    expect(ui.text()).toContain("4 hits");
  });

  it("hands back what to jump to only for a row found by content", async () => {
    invoke.mockResolvedValue(results());
    const ui = await mount();
    await ui.type("trust banner");
    await ui.settle();

    const hit = ui.rows().find((r) => r.textContent?.includes("Something else"))!;
    await act(async () => hit.click());
    expect(ui.opened).toEqual([["old", "trust banner"]]);
  });

  it("hands back nothing to jump to for a row found by title", async () => {
    // A title match says which conversation, not where in it.
    const ui = await mount();
    await ui.type("chart");
    const row = ui.rows().find((r) => r.textContent?.includes("Adding a chart"))!;
    await act(async () => row.click());
    expect(ui.opened).toEqual([["a", null]]);
  });

  it("does not offer the same conversation twice", async () => {
    // Matched by title and by content is one conversation, and two rows for it
    // is one row of noise plus a second chance to pick the wrong one.
    invoke.mockResolvedValue(
      results({
        sessions: [
          {
            session: session("a", "Adding a chart"),
            hits: 1,
            matches: [
              { message: 1, role: "user", excerpt: "add a chart", from: 6, to: 11 },
            ],
          },
        ],
      }),
    );
    const ui = await mount();
    await ui.type("chart");
    await ui.settle();
    expect(ui.labels().filter((l) => l === "Adding a chart").length).toBe(1);
  });

  it("says how many conversations it did not list", async () => {
    invoke.mockResolvedValue(results({ more: 5 }));
    const ui = await mount();
    await ui.type("widget");
    await ui.settle();
    expect(ui.text()).toContain("5 more conversations matched");
  });

  it("keeps the local answers when the search fails", async () => {
    // The actions are still the answer to most of what this box is opened for.
    invoke.mockRejectedValue("disk on fire");
    const ui = await mount();
    await ui.type("chan");
    await ui.settle();
    expect(ui.labels()).toContain("Changes");
  });

  it("reaches past this workspace when asked to", async () => {
    invoke.mockResolvedValue(results());
    const ui = await mount();
    await ui.type("widget");
    await ui.settle();
    const scope = ui.host.querySelector(".palette-scope") as HTMLButtonElement;
    await act(async () => scope.click());
    await ui.settle();
    expect(invoke).toHaveBeenLastCalledWith("search_sessions", {
      query: "widget",
      everywhere: true,
    });
  });

  it("says so rather than showing an empty box when nothing matches", async () => {
    invoke.mockResolvedValue(results({ sessions: [], more: 0 }));
    const ui = await mount();
    await ui.type("zzzzz");
    await ui.settle();
    expect(ui.text()).toContain("Nothing matches that");
  });
});
