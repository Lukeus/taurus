// @vitest-environment jsdom
//
// The slot owns the reads and writes behind the canvas. What's checked here is
// what it tells the header when one of them fails.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { CanvasSlot } from "./CanvasSlot";
import { useStore } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const TEXT = "fn main() {}\n";
const DOC = { path: "src/main.rs", text: TEXT, lines: 2, fingerprint: "1-2" };
const pane = { size: 400, dragging: false, min: 200, max: 800, start() {}, nudge() {} };

afterEach(() => {
  document.body.innerHTML = "";
  invoke.mockReset();
  useStore.setState({ wrote: null });
});

describe("the open file", () => {
  it("is said to be gone when the turn deleted it, and its text is kept", async () => {
    let reads = 0;
    invoke.mockImplementation((command: string) => {
      if (command !== "open_document") return Promise.resolve();
      reads += 1;
      return reads === 1
        ? Promise.resolve(DOC)
        : Promise.reject("src/main.rs is no longer there.");
    });
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => {
      root.render(
        <CanvasSlot
          path="src/main.rs"
          reveal={null}
          workspace="/src/project"
          showing
          pane={pane}
          onUnsaved={() => {}}
          onSelect={() => {}}
          onAsk={() => {}}
          onClose={() => {}}
        />,
      );
    });
    expect(host.querySelector(".canvas-state")).toBeNull();

    // The running turn reports writing it, and the re-read finds nothing.
    await act(async () => useStore.setState({ wrote: { paths: ["src/main.rs"], at: 1 } }));
    expect(host.querySelector(".canvas-state")?.textContent).toBe("Gone from disk");
    expect((host.querySelector(".doc-input") as HTMLTextAreaElement).value).toBe(TEXT);
    act(() => root.unmount());
  });
});
