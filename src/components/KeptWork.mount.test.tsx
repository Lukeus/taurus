// @vitest-environment jsdom
//
// A delegate's work kept in its own copy of the workspace: the card has to say
// it isn't in the workspace yet, and Apply has to say plainly when it refused.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const applyDelegate = vi.fn();
const discardDelegate = vi.fn();
const delegateRun = vi.fn();
vi.mock("../lib/api", async (original) => ({
  ...(await original<typeof import("../lib/api")>()),
  applyDelegate: (...args: unknown[]) => applyDelegate(...args),
  discardDelegate: (...args: unknown[]) => discardDelegate(...args),
  delegateRun: (...args: unknown[]) => delegateRun(...args),
}));

import { Transcript } from "./Transcript";
import type { ApplyPlan } from "../lib/api";
import { useStore, type Entry } from "../state/store";
import { useKept } from "./KeptWork";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
Element.prototype.scrollIntoView = () => {};

let cleanup: (() => void)[] = [];
beforeEach(() => {
  applyDelegate.mockReset();
  delegateRun.mockReset();
  delegateRun.mockResolvedValue({ run: "0123456789ab", state: "kept" });
  useKept.setState({ runs: {} });
  discardDelegate.mockReset();
  useStore.setState({
    session: { id: "s1" } as never,
    busy: false,
    wrote: null,
    refreshChanged: async () => {},
  });
});
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

const kept: Entry = {
  kind: "tool",
  id: "d1",
  name: "spawn_subagent",
  preview: "Delegate to coder: rewrite a.txt",
  status: "ok",
  steps: [],
  output: "Status: done.",
  report: {
    disposition: "done",
    summary: "Rewrote a.txt.",
    files: ["a.txt"],
    notes: ["Copied in because .worktreeinclude lists them: .env."],
    kept: { run: "0123456789ab", base: "deadbeef00", files: ["a.txt", "b.txt"], outside: [] },
  },
};

function mount(entries: Entry[]) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() =>
    root.render(
      <Transcript entries={entries} busy={false} empty={null} find={null} onAnswer={() => {}} />,
    ),
  );
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  const button = (label: string) =>
    [...host.querySelectorAll("button")].find((b) => b.textContent === label) ?? null;
  return { host, button };
}

const plan = (patch: Partial<ApplyPlan>): ApplyPlan => ({
  run: "0123456789ab",
  files: [{ path: "a.txt", action: "take" }],
  outside: [],
  applied: true,
  ...patch,
});

describe("a delegate's kept work", () => {
  it("says it isn't in the workspace yet, and names the files", () => {
    const { host } = mount([kept]);
    const head = host.querySelector(".run-row-kept-head")?.textContent ?? "";
    expect(head).toContain("not in the workspace yet");
    expect(head).toContain("deadbeef00");
    expect([...host.querySelectorAll(".run-row-kept-files li")].map((l) => l.textContent)).toEqual([
      "a.txt",
      "b.txt",
    ]);
    expect(host.querySelector(".run-row-notes")?.textContent).toContain(".worktreeinclude");
  });

  it("applies from this conversation and says how to undo it", async () => {
    applyDelegate.mockResolvedValue(plan({}));
    const { host, button } = mount([kept]);
    await act(async () => button("Apply to workspace")!.click());
    expect(applyDelegate).toHaveBeenCalledWith("s1", "0123456789ab", false);
    expect(host.querySelector(".run-row-kept-head")?.textContent).toContain(
      "Rewinding this turn undoes it",
    );
    expect(button("Apply to workspace")).toBeNull();
    // An editor open on a.txt is told, as it is for a turn's own write.
    expect(useStore.getState().wrote?.paths).toEqual(["a.txt"]);
  });

  it("says which files refused an apply, and offers it again", async () => {
    applyDelegate.mockResolvedValue(
      plan({
        applied: false,
        files: [{ path: "a.txt", action: "conflict", why: "the two changes overlap" }],
      }),
    );
    const { host, button } = mount([kept]);
    await act(async () => button("Apply to workspace")!.click());
    const refused = host.querySelector(".run-row-kept-refused")?.textContent ?? "";
    expect(refused).toContain("Nothing was applied");
    expect(refused).toContain("a.txt: the two changes overlap");
    expect(button("Apply to workspace")).not.toBeNull();
  });

  it("can't be applied while a turn is running", () => {
    useStore.setState({ busy: true });
    const { button } = mount([kept]);
    expect(button("Apply to workspace")?.hasAttribute("disabled")).toBe(true);
  });

  it("discards without applying", async () => {
    discardDelegate.mockResolvedValue(undefined);
    const { host, button } = mount([kept]);
    await act(async () => button("Discard")!.click());
    expect(discardDelegate).toHaveBeenCalledWith("s1", "0123456789ab");
    expect(applyDelegate).not.toHaveBeenCalled();
    expect(host.querySelector(".run-row-kept-head")?.textContent).toContain("Discarded");
  });

  it("doesn't offer Apply again on work applied somewhere else", async () => {
    // Settings, the CLI, the model, or this window before a reload: the card
    // is drawn again knowing nothing, and asks.
    delegateRun.mockResolvedValue({ run: "0123456789ab", state: "applied" });
    const { host, button } = mount([kept]);
    await act(async () => {});
    expect(delegateRun).toHaveBeenCalledWith("0123456789ab");
    expect(host.querySelector(".run-row-kept-head")?.textContent).toContain("Applied");
    expect(button("Apply to workspace")).toBeNull();
  });

  it("says so for work discarded somewhere else", async () => {
    delegateRun.mockResolvedValue({ run: "0123456789ab", state: "discarded" });
    const { host, button } = mount([kept]);
    await act(async () => {});
    expect(host.querySelector(".run-row-kept-head")?.textContent).toContain("Discarded");
    expect(button("Discard")).toBeNull();
  });
});
