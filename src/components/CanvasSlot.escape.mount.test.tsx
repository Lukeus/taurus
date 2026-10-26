// @vitest-environment jsdom
//
// Escape closes the canvas the way it closes Changes, and only when it was
// aimed at the canvas.
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

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const DOC = { path: "src/main.rs", text: "fn main() {}\n", lines: 2, fingerprint: "1-2" };
const pane = { size: 400, dragging: false, min: 200, max: 800, start() {}, nudge() {} };

afterEach(() => {
  document.body.innerHTML = "";
  invoke.mockReset();
});

async function mount(onClose: () => void) {
  invoke.mockImplementation((command: string) =>
    command === "open_document" ? Promise.resolve(DOC) : Promise.resolve(),
  );
  const outside = document.createElement("input");
  document.body.appendChild(outside);
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
        onClose={onClose}
      />,
    );
  });
  const escape = (target: Element) =>
    act(() => {
      target.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
  return { host, outside, escape, unmount: () => act(() => root.unmount()) };
}

describe("Escape on the canvas", () => {
  it("leaves the editor first, then closes", async () => {
    const onClose = vi.fn();
    const ui = await mount(onClose);
    const editor = ui.host.querySelector("textarea")!;
    editor.focus();
    await ui.escape(editor);
    expect(onClose).not.toHaveBeenCalled();
    expect(document.activeElement).not.toBe(editor);

    await ui.escape(document.body);
    expect(onClose).toHaveBeenCalledTimes(1);
    ui.unmount();
  });

  it("leaves an Escape aimed somewhere else alone", async () => {
    // The composer's Escape stops a reply being read; it shouldn't also
    // close the file.
    const onClose = vi.fn();
    const ui = await mount(onClose);
    ui.outside.focus();
    await ui.escape(ui.outside);
    expect(onClose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(ui.outside);
    ui.unmount();
  });
});
