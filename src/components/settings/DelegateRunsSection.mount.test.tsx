// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const delegateRuns = vi.fn();
const discardDelegate = vi.fn();
vi.mock("../../lib/api", async (original) => ({
  ...(await original<typeof import("../../lib/api")>()),
  delegateRuns: () => delegateRuns(),
  discardDelegate: (...args: unknown[]) => discardDelegate(...args),
}));

import { DelegateRunsSection } from "./DelegateRunsSection";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  delegateRuns.mockReset();
  discardDelegate.mockReset();
});

async function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(<DelegateRunsSection />));
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return host;
}

const run = {
  run: "0123456789ab",
  agent: "coder",
  workspace: "/work/shop",
  session: "s1",
  created: 1_700_000_000,
  state: "kept",
  left_behind: null,
};

describe("work delegates kept", () => {
  it("draws nothing when nothing is waiting", async () => {
    delegateRuns.mockResolvedValue([]);
    const host = await mount();
    expect(host.textContent).toBe("");
  });

  it("lists what's waiting, and discards it", async () => {
    delegateRuns.mockResolvedValueOnce([run]).mockResolvedValueOnce([]);
    discardDelegate.mockResolvedValue(undefined);
    const host = await mount();
    expect(host.textContent).toContain("coder in shop");
    expect(host.textContent).toContain("waiting to be applied");
    const discard = [...host.querySelectorAll("button")].find((b) => b.textContent === "Discard");
    await act(async () => discard!.click());
    expect(discardDelegate).toHaveBeenCalledWith("s1", "0123456789ab");
    expect(host.textContent).toBe("");
  });

  it("says why a folder is still on disk", async () => {
    delegateRuns.mockResolvedValue([
      { ...run, left_behind: "The worktree at /x couldn't be removed (busy)." },
    ]);
    const host = await mount();
    expect(host.textContent).toContain("couldn't be removed");
  });
});
