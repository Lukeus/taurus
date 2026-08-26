// @vitest-environment jsdom
//
// The panel fetches in an effect and refetches when the scope changes, so both
// of the things worth checking here happen after the first paint. A string
// render would see neither.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { UsagePanel } from "./UsagePanel";
import type { UsageReport } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const report = (patch: Partial<UsageReport> = {}): UsageReport => ({
  sessions: 1,
  turns: 4,
  messages: 22,
  reported_in: 180_000,
  reported_out: 6_000,
  cached_in: null,
  history: 41_000,
  tools: [
    { name: "read_file", calls: 9, tokens: 30_000, failures: 0, share: 75 },
    { name: "grep", calls: 4, tokens: 10_000, failures: 1, share: 25 },
  ],
  repeats: 0,
  repeat_tokens: 0,
  system_prompt: 3_400,
  schemas: [{ name: "run_command", tokens: 900 }],
  ...patch,
});

let cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  invoke.mockReset();
});

async function mount(sessionId: string | null = "s1") {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<UsagePanel sessionId={sessionId} onClose={() => {}} />);
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return {
    host,
    text: () => host.textContent ?? "",
    click: async (label: string) => {
      const button = [...host.querySelectorAll("button")].find(
        (b) => b.textContent === label,
      );
      await act(async () => (button as HTMLElement).click());
    },
  };
}

describe("the context account", () => {
  it("asks about the open conversation first", async () => {
    // The conversation is the one that just did something. The workspace view
    // answers "is it always like this", which is a question you ask second.
    invoke.mockResolvedValue(report());
    const ui = await mount("s1");

    expect(invoke).toHaveBeenCalledWith("usage_report", { sessionId: "s1" });
    expect(ui.text()).toContain("read_file");
    expect(ui.text()).toContain("75%");
  });

  it("asks about the workspace when the scope is switched", async () => {
    invoke.mockResolvedValue(report({ sessions: 12 }));
    const ui = await mount("s1");
    await ui.click("Every conversation here");

    expect(invoke).toHaveBeenLastCalledWith("usage_report", { sessionId: null });
    expect(ui.text()).toContain("12 conversations");
  });

  it("starts on the workspace when there is no conversation open", async () => {
    invoke.mockResolvedValue(report({ sessions: 3 }));
    const ui = await mount(null);

    expect(invoke).toHaveBeenCalledWith("usage_report", { sessionId: null });
    // And the tab that would ask about nothing is not offered.
    const tab = [...ui.host.querySelectorAll("button")].find(
      (b) => b.textContent === "This conversation",
    ) as HTMLButtonElement;
    expect(tab.disabled).toBe(true);
  });

  it("still reports what a request costs in a workspace with no history", async () => {
    // The half that does not come from a transcript, which is the half worth
    // reading *before* running anything — so an empty workspace is not an
    // empty panel.
    invoke.mockResolvedValue(
      report({ sessions: 0, turns: 0, messages: 0, tools: [], history: 0 }),
    );
    const ui = await mount(null);

    expect(ui.text()).toContain("Nothing has been recorded");
    expect(ui.text()).toContain("Sent again with every request");
    expect(ui.text()).toContain("run_command");
  });

  it("names the cache only when there was one to read from", async () => {
    // A local Ollama has no cache to have missed, and a line reading "0
    // cached" beside its numbers invites exactly the wrong conclusion.
    invoke.mockResolvedValue(report({ cached_in: null }));
    expect((await mount()).text()).not.toContain("from cache");

    invoke.mockResolvedValue(report({ cached_in: 90_000 }));
    expect((await mount()).text()).toContain("50% of input came from cache");
  });

  it("calls out repeated calls, which are the part that is pure waste", async () => {
    invoke.mockResolvedValue(report({ repeats: 3, repeat_tokens: 12_000 }));
    const ui = await mount();
    expect(ui.text()).toContain("repeated an earlier one exactly");
    expect(ui.text()).toContain("12k");
  });

  it("says nothing about repeats when there were none", async () => {
    invoke.mockResolvedValue(report());
    expect((await mount()).text()).not.toContain("repeated an earlier one");
  });

  it("says how many schemas it did not name", async () => {
    // A list that stops without saying so reads as the whole list, and the
    // tools it hid are exactly the ones somebody deciding what to turn off
    // would want to know about.
    invoke.mockResolvedValue(
      report({
        schemas: Array.from({ length: 9 }, (_, i) => ({
          name: `tool_${i}`,
          tokens: 100 * (9 - i),
        })),
      }),
    );
    const ui = await mount();
    expect(ui.text()).toContain("4 more");
  });

  it("reports a backend that could not answer rather than staying blank", async () => {
    invoke.mockRejectedValue("no such session");
    const ui = await mount("gone");
    expect(ui.text()).toContain("no such session");
    expect(ui.text()).not.toContain("Reading…");
  });
});
