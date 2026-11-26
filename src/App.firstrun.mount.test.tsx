// @vitest-environment jsdom
//
// The empty window leads with whichever step is actually in the way, rather
// than offering both side by side and leaving the reader to work out which.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import { FirstRun } from "./App";
import type { ProviderHealth } from "./components/Rail";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

function mount(workspace: string | null, health: ProviderHealth, ready = false) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => {
    root.render(
      <FirstRun
        workspace={workspace}
        ready={ready}
        health={health}
        onPickWorkspace={() => {}}
        onSettings={() => {}}
      />,
    );
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return {
    heading: host.querySelector("h1")?.textContent,
    primary: host.querySelector(".hero-actions .primary")?.textContent,
  };
}

describe("the first-run hero", () => {
  it("asks for a workspace first when there is none", () => {
    const { heading, primary } = mount(null, { state: "none" });
    expect(heading).toBe("Point Taurus at a workspace");
    expect(primary).toBe("Choose a workspace");
  });

  it("leads with the model once a workspace is open and no provider is", () => {
    const { heading, primary } = mount("/src/taurus", { state: "none" });
    expect(heading).toBe("Connect a model to start");
    expect(primary).toBe("Connect a model");
  });

  it("names a provider that is configured but not answering", () => {
    const { heading, primary } = mount("/src/taurus", { state: "unreachable", id: "ollama" });
    expect(heading).toBe("ollama is not answering");
    expect(primary).toBe("Connect a model");
  });

  it("says it is ready when both are in place", () => {
    const { heading } = mount(
      "/src/taurus",
      { state: "connected", id: "ollama", models: 2 },
      true,
    );
    expect(heading).toBe("Ready in taurus");
  });
});
