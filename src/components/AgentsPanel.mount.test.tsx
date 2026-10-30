// @vitest-environment jsdom
//
// The panel draws every delegation in a conversation from two partial
// sources: the transcript's calls, and the runs this window watched.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { AgentsView, questions } from "./AgentsPanel";
import type { AgentRun } from "../lib/agents";
import type { Entry } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

afterEach(() => {
  document.body.innerHTML = "";
});

const ask = (id: string, text: string): Entry => ({ kind: "user", id, text }) as Entry;
const spawn = (id: string, agent: string, status: "running" | "ok" = "running"): Entry =>
  ({
    kind: "tool",
    id,
    name: "spawn_subagent",
    preview: `Delegate to ${agent}: find the port`,
    status,
    steps: [],
  }) as unknown as Entry;

const NOW = Date.now();
const running = (id: string, patch: Partial<AgentRun> = {}): AgentRun => ({
  id,
  agent: "explorer",
  model: "qwen3",
  startedAt: NOW - 30_000,
  maxRounds: 20,
  round: 3,
  tools: [
    { call: "r1", name: "grep", preview: "grep port", startedAt: NOW - 20_000, endedAt: NOW - 19_000, ok: true },
    { call: "r2", name: "read_file", preview: "read src/config.rs", startedAt: NOW - 5_000 },
  ],
  text: "The port is set in config.",
  thinking: "",
  detached: false,
  ...patch,
});

function mount(props: Partial<Parameters<typeof AgentsView>[0]> = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const onOpenTranscript = vi.fn();
  const onClose = vi.fn();
  act(() =>
    root.render(
      <AgentsView
        entries={[]}
        agents={{}}
        model="qwen3.6:27b"
        busy={false}
        focus={null}
        onClose={onClose}
        onOpenTranscript={onOpenTranscript}
        {...props}
      />,
    ),
  );
  return { host, onOpenTranscript, onClose, unmount: () => act(() => root.unmount()) };
}

describe("questions", () => {
  it("puts each delegation under the question it was made answering", () => {
    const groups = questions([
      ask("u1", "fix the flaky test"),
      spawn("c1", "explorer"),
      ask("u2", "now the docs\nand more"),
      spawn("c2", "coder"),
      spawn("c3", "explorer"),
    ]);
    const asked = groups.filter((g) => g.calls.length > 0);
    expect(asked.map((g) => [g.text, g.calls.map((c) => c.id)])).toEqual([
      ["fix the flaky test", ["c1"]],
      ["now the docs", ["c2", "c3"]],
    ]);
  });
});

describe("the agent activity panel", () => {
  it("says there's nothing yet in a conversation that delegated nothing", () => {
    const ui = mount({ entries: [ask("u1", "hi")] });
    expect(ui.host.textContent).toContain("Nothing has been delegated");
    ui.unmount();
  });

  it("draws each delegate under its question with what it's doing", () => {
    const ui = mount({
      busy: true,
      entries: [ask("u1", "find the port"), spawn("c1", "explorer"), spawn("c2", "coder")],
      agents: {
        c1: running("c1"),
        c2: running("c2", { agent: "coder", queuedAt: NOW - 10_000, startedAt: undefined, round: 0, tools: [] }),
      },
    });
    const text = ui.host.textContent ?? "";
    expect(text).toContain("find the port");
    expect(text).toContain("explorer");
    expect(text).toContain("round trip 3 of 20");
    // The call it's in the middle of.
    expect(text).toContain("read src/config.rs");
    expect(text).toContain("waiting for a slot");
    // Both on one clock, so side by side looks side by side.
    expect(ui.host.querySelectorAll(".agents-bar").length).toBe(1);
    ui.unmount();
  });

  it("opens on the delegate a card asked to watch, with its calls and words", () => {
    const ui = mount({
      entries: [ask("u1", "find the port"), spawn("c1", "explorer")],
      agents: { c1: running("c1", { session: "child-1" }) },
      focus: "c1",
    });
    const node = ui.host.querySelector(".agents-node.open");
    expect(node).not.toBeNull();
    expect(node?.querySelectorAll(".agents-calls li").length).toBe(2);
    expect(node?.textContent).toContain("The port is set in config.");
    const read = [...ui.host.querySelectorAll("button")].find((b) =>
      b.textContent?.includes("Read its whole transcript"),
    )!;
    act(() => read.click());
    expect(ui.onOpenTranscript).toHaveBeenCalledWith({ session: "child-1", agent: "explorer" });
    ui.unmount();
  });

  it("draws a delegation this window didn't watch from its report", () => {
    const call = {
      ...(spawn("c1", "explorer", "ok") as object),
      report: { disposition: "blocked", owner: "user", needs: "choose a port", summary: "Two configs disagree.", files: [] },
    } as unknown as Entry;
    const ui = mount({ entries: [ask("u1", "find the port"), call], focus: "c1" });
    const text = ui.host.textContent ?? "";
    expect(text).toContain("explorer");
    expect(text).toContain("Needs you to: choose a port");
    expect(text).toContain("no transcript is linked");
    ui.unmount();
  });

  it("closes on Escape, as Changes does", () => {
    const ui = mount();
    act(() => {
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(ui.onClose).toHaveBeenCalled();
    ui.unmount();
  });
});
