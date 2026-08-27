// @vitest-environment jsdom
//
// The panel fetches in an effect, refetches when the scope changes, and opens
// a turn's waterfall on a click. None of the three happens before the first
// paint, so a string render would see none of them.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { TracePanel, ms } from "./TracePanel";
import type { TraceReport, TurnTrace } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const turn = (patch: Partial<TurnTrace> = {}): TurnTrace => ({
  seq: 10,
  conversation: "s1",
  model: "qwen3.6:27b",
  provider: "ollama",
  started: new Date(2026, 0, 2, 9, 30, 0).getTime(),
  duration_ms: 8_000,
  model_ms: 6_000,
  other_ms: 2_000,
  input_tokens: 12_000,
  output_tokens: 400,
  finish: "stop",
  error: null,
  steps: [
    {
      kind: "chat",
      name: "qwen3.6:27b",
      offset_ms: 0,
      duration_ms: 3_000,
      depth: 1,
      error: null,
      output_tokens: 120,
    },
    {
      kind: "tool",
      name: "spawn",
      offset_ms: 3_000,
      duration_ms: 4_000,
      depth: 1,
      error: null,
      output_tokens: null,
    },
    {
      kind: "chat",
      name: "qwen3.6:27b",
      offset_ms: 3_100,
      duration_ms: 3_000,
      depth: 2,
      error: null,
      output_tokens: 280,
    },
  ],
  ...patch,
});

const report = (patch: Partial<TraceReport> = {}): TraceReport => ({
  turns: 1,
  spans: 4,
  dropped: 0,
  since: new Date(2026, 0, 2, 9, 29, 0).getTime(),
  total_ms: 8_000,
  model_ms: 6_000,
  other_ms: 2_000,
  median_turn_ms: 8_000,
  slowest_turn_ms: 8_000,
  failures: 0,
  models: [
    {
      name: "qwen3.6:27b",
      provider: "ollama",
      calls: 2,
      median_ms: 3_000,
      slowest_ms: 3_000,
      input_tokens: 12_000,
      output_tokens: 400,
      cached_tokens: null,
      failures: 0,
      output_per_second: 66,
    },
  ],
  tools: [
    {
      name: "spawn",
      calls: 1,
      failures: 0,
      median_ms: 4_000,
      slowest_ms: 4_000,
      total_ms: 4_000,
      share: 100,
      nested: true,
    },
  ],
  recent: [turn()],
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
    root.render(<TracePanel sessionId={sessionId} onClose={() => {}} />);
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  const find = (label: string) =>
    [...host.querySelectorAll("button")].find((b) =>
      b.textContent?.includes(label),
    );
  return {
    host,
    text: () => host.textContent ?? "",
    find,
    click: async (label: string) => {
      await act(async () => (find(label) as HTMLElement).click());
    },
  };
}

describe("the trace panel", () => {
  it("asks about the open conversation first", async () => {
    // The conversation is the one that just did something. The window-wide
    // view answers "is it always like this", which is asked second.
    invoke.mockResolvedValue(report());
    const ui = await mount("s1");

    expect(invoke).toHaveBeenCalledWith("trace_report", { sessionId: "s1" });
    expect(ui.text()).toContain("qwen3.6:27b");
  });

  it("asks about the whole window when the scope is switched", async () => {
    invoke.mockResolvedValue(report({ turns: 40 }));
    const ui = await mount("s1");
    await ui.click("Everything since launch");

    expect(invoke).toHaveBeenLastCalledWith("trace_report", {
      sessionId: null,
    });
    expect(ui.text()).toContain("40");
  });

  it("starts on the window when there is no conversation open", async () => {
    invoke.mockResolvedValue(report());
    const ui = await mount(null);

    expect(invoke).toHaveBeenCalledWith("trace_report", { sessionId: null });
    expect((ui.find("This conversation") as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it("says what would fill an empty panel rather than drawing nothing", async () => {
    // A window that has not run a turn has nothing to time, and that is not a
    // failure. Naming what appears here is more use than an empty table.
    invoke.mockResolvedValue(report({ spans: 0, turns: 0, recent: [] }));
    const ui = await mount();

    expect(ui.text()).toContain("Nothing has been timed yet");
    expect(ui.text()).not.toContain("Where the time went");
  });

  it("splits the time into model calls and everything else", async () => {
    // The headline, and the one split that is safe to make.
    invoke.mockResolvedValue(report());
    const ui = await mount();

    expect(ui.text()).toContain("Model 75%");
    expect(ui.text()).toContain("Everything else 25%");
  });

  it("opens one turn's waterfall at a time", async () => {
    // Every turn expanded at once is a page of bars with no way to compare
    // two of them.
    invoke.mockResolvedValue(
      report({ recent: [turn(), turn({ seq: 11, started: 1 })] }),
    );
    const ui = await mount();
    expect(ui.host.querySelectorAll(".trace-flow")).toHaveLength(0);

    await ui.click("09:30:00");
    expect(ui.host.querySelectorAll(".trace-flow")).toHaveLength(1);
    expect(ui.host.querySelectorAll(".trace-step")).toHaveLength(3);
  });

  it("indents a delegate's work under the tool that spawned it", async () => {
    // The indent is the only thing on a waterfall that tells a sub-agent's
    // model call apart from the turn's own.
    invoke.mockResolvedValue(report());
    const ui = await mount();
    await ui.click("09:30:00");

    const depths = [...ui.host.querySelectorAll<HTMLElement>(".trace-step")].map(
      (step) => step.style.getPropertyValue("--depth"),
    );
    expect(depths).toEqual(["1", "1", "2"]);
  });

  it("says when a tool's time contains a delegate's whole turn", async () => {
    // Otherwise the row simply dwarfs the others with no explanation, and the
    // reader concludes `spawn` itself is slow.
    invoke.mockResolvedValue(report());
    expect((await mount()).text()).toContain("includes a delegate");
  });

  it("names the cache only when a backend reported one", async () => {
    // A local model has no cache to have missed, and a 0% beside its name
    // invites exactly the wrong conclusion.
    invoke.mockResolvedValue(report());
    expect((await mount()).text()).not.toContain("cached");

    const models = report().models.map((m) => ({
      ...m,
      cached_tokens: 6_000,
    }));
    invoke.mockResolvedValue(report({ models }));
    expect((await mount()).text()).toContain("50% cached");
  });

  it("shows a turn that failed as having failed", async () => {
    invoke.mockResolvedValue(
      report({ recent: [turn({ error: "provider" })], failures: 1 }),
    );
    expect((await mount()).text()).toContain("provider");
  });

  it("says the window has a start when spans have already been forgotten", async () => {
    // A dashboard that has quietly stopped covering the period it appears to
    // cover is worse than one that says so.
    invoke.mockResolvedValue(report({ dropped: 312 }));
    expect((await mount()).text()).toContain("312 older spans already forgotten");
  });

  it("forgets and asks again when told to clear", async () => {
    invoke.mockResolvedValue(report());
    const ui = await mount();
    invoke.mockClear();
    invoke.mockResolvedValue(report({ spans: 0, turns: 0, recent: [] }));

    await ui.click("Clear");
    expect(invoke).toHaveBeenCalledWith("clear_traces");
    expect(ui.text()).toContain("Nothing has been timed yet");
  });

  it("reports a backend that could not answer rather than staying blank", async () => {
    invoke.mockRejectedValue("no such session");
    const ui = await mount("gone");
    expect(ui.text()).toContain("no such session");
    expect(ui.text()).not.toContain("Reading…");
  });
});

describe("reading a duration", () => {
  it("changes unit with the scale, because the difference that matters does", () => {
    // Under a second, milliseconds are what tool calls are compared in. Past
    // it, a decimal second is how long a turn felt. Past a minute, nobody is
    // counting seconds.
    expect(ms(4)).toBe("4ms");
    expect(ms(820)).toBe("820ms");
    expect(ms(7_400)).toBe("7.4s");
    expect(ms(59_900)).toBe("59.9s");
    expect(ms(72_000)).toBe("1m 12s");
  });
});
