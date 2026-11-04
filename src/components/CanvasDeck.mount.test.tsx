// @vitest-environment jsdom
//
// Several files open at once. What's checked is what a hidden tab goes on doing
// and what it must not do: speak for the file on screen.
import { act, useCallback, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { CanvasDeck } from "./CanvasDeck";
import type { TabMark } from "./CanvasSlot";
import { closeTab, nameTab, openTab, showTab, type CanvasTabs } from "../lib/canvasTabs";
import { useStore } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const fill = vi.fn();
const pane = { size: 400, dragging: false, min: 200, max: 800, start() {}, nudge() {}, fill, filled: false, easing: false } as never;
const flushRef: { current: (() => Promise<boolean>) | null } = { current: null };

let saves: { path: string; text: string }[];
let disk: Record<string, { path: string; text: string; lines: number; fingerprint: string }>;
let onSelect: ReturnType<typeof vi.fn>;
let root: Root;
let host: HTMLElement;

function Harness({ start, showing = true }: { start: CanvasTabs; showing?: boolean }) {
  const [canvas, setCanvas] = useState<CanvasTabs | null>(start);
  const [marks, setMarks] = useState<Record<string, TabMark | undefined>>({});
  const onShow = useCallback((path: string) => setCanvas((c) => c && showTab(c, path)), []);
  const onClose = useCallback((path: string) => setCanvas((c) => c && closeTab(c, path)), []);
  const onNamed = useCallback(
    (path: string, shown: string) => setCanvas((c) => c && nameTab(c, path, shown)),
    [],
  );
  const onMark = useCallback(
    (path: string, mark: TabMark) => setMarks((all) => ({ ...all, [path]: mark })),
    [],
  );
  if (!canvas) return <div className="all-closed" />;
  return (
    <CanvasDeck
      canvas={canvas}
      marks={marks}
      workspace="/w"
      showing={showing}
      pane={pane}
      onShow={onShow}
      onClose={onClose}
      onNamed={onNamed}
      onMark={onMark}
      onSelect={onSelect}
      onAsk={() => {}}
      flushRef={flushRef}
    />
  );
}

const two = () => openTab(openTab(null, "a.rs", null), "b.rs", null);
const mount = async (start: CanvasTabs = two(), showing = true) => {
  await act(async () => root.render(<Harness start={start} showing={showing} />));
  await act(async () => vi.advanceTimersByTimeAsync(0));
};
const panels = () => [...host.querySelectorAll<HTMLElement>(".canvas-panel")];
const boxFor = (name: string) => host.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${name}"]`)!;
const tab = (name: string) =>
  [...host.querySelectorAll<HTMLButtonElement>('[role="tab"]')].find((t) => t.textContent === name)!;
const type = (name: string, text: string) =>
  act(async () => {
    const box = boxFor(name);
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(box, text);
    box.dispatchEvent(new Event("input", { bubbles: true }));
  });

beforeEach(() => {
  vi.useFakeTimers();
  saves = [];
  onSelect = vi.fn();
  disk = {
    "a.rs": { path: "a.rs", text: "a\n", lines: 1, fingerprint: "a1" },
    "b.rs": { path: "b.rs", text: "b\n", lines: 1, fingerprint: "b1" },
  };
  invoke.mockImplementation((command: string, args: { path: string; text: string }) => {
    if (command === "open_document") return Promise.resolve(disk[args.path]);
    if (command === "save_document") {
      saves.push({ path: args.path, text: args.text });
      disk[args.path] = { ...disk[args.path], text: args.text, fingerprint: `s${saves.length}` };
      return Promise.resolve({ type: "written", document: disk[args.path] });
    }
    return Promise.resolve();
  });
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  document.body.innerHTML = "";
  invoke.mockReset();
  useStore.setState({ wrote: null });
  vi.useRealTimers();
});

describe("a row of open files", () => {
  it("shows one and keeps the other laid out but invisible", async () => {
    await mount();
    expect(panels().map((p) => p.style.visibility)).toEqual(["hidden", ""]);
    expect(boxFor("a.rs").value).toBe("a\n");
  });

  it("has no row at all for one file", async () => {
    await mount(openTab(null, "a.rs", null));
    expect(host.querySelector(".canvas-tabs")).toBeNull();
  });

  it("widens the column from the file's own header", async () => {
    // The edge's double-click is the same thing, and nobody finds that alone.
    await mount();
    const widen = host.querySelector<HTMLButtonElement>(
      '.canvas-panel:not([style*="hidden"]) [aria-label="Widen the editor"]',
    );
    expect(widen?.getAttribute("aria-pressed")).toBe("false");
    await act(async () => widen!.click());
    expect(fill).toHaveBeenCalledTimes(1);
  });

  it("has one marker for the row, under the file on screen", async () => {
    await mount();
    expect(host.querySelectorAll(".canvas-tabs .glide")).toHaveLength(1);
    expect(host.querySelector(".canvas-tabs")?.hasAttribute("data-glide")).toBe(true);
  });

  it("moves to the file a tab names", async () => {
    await mount();
    await act(async () => tab("a.rs").click());
    expect(panels().map((p) => p.style.visibility)).toEqual(["", "hidden"]);
  });

  it("keeps typing and marks it when the tab goes behind another", async () => {
    await mount();
    await type("b.rs", "b typed\n");
    expect(tab("•b.rs")).toBeDefined();
    await act(async () => tab("a.rs").click());
    expect(boxFor("b.rs").value).toBe("b typed\n");
  });

  it("saves a file whose tab has gone behind another", async () => {
    await mount();
    await type("b.rs", "b typed\n");
    await act(async () => tab("a.rs").click());
    await act(async () => vi.advanceTimersByTimeAsync(2_000));
    expect(saves).toEqual([{ path: "b.rs", text: "b typed\n" }]);
  });

  it("writes what's typed when a tab closes", async () => {
    await mount();
    await type("b.rs", "b typed\n");
    await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="Close b.rs"]')!.click());
    await act(async () => vi.advanceTimersByTimeAsync(0));
    expect(saves).toEqual([{ path: "b.rs", text: "b typed\n" }]);
    expect(panels()).toHaveLength(1);
  });

  it("flushes every file, and answers for the one showing", async () => {
    await mount();
    await type("a.rs", "a typed\n");
    await type("b.rs", "b typed\n");
    let answer: boolean | undefined;
    await act(async () => {
      answer = await flushRef.current!();
    });
    expect(answer).toBe(true);
    expect(saves.map((s) => s.path).sort()).toEqual(["a.rs", "b.rs"]);
  });

  it("reloads a file changed by a turn though its tab is hidden", async () => {
    await mount();
    disk["a.rs"] = { ...disk["a.rs"], text: "turn wrote\n", fingerprint: "a2" };
    await act(async () => useStore.setState({ wrote: { paths: ["a.rs"], at: 1 } }));
    await act(async () => vi.advanceTimersByTimeAsync(0));
    expect(boxFor("a.rs").value).toBe("turn wrote\n");
  });

  it("notices a change made by something else when the tab comes to the front", async () => {
    await mount();
    disk["a.rs"] = { ...disk["a.rs"], text: "git checkout\n", fingerprint: "a2" };
    await act(async () => tab("a.rs").click());
    await act(async () => vi.advanceTimersByTimeAsync(0));
    expect(boxFor("a.rs").value).toBe("git checkout\n");
  });

  it("raises no conflict over typing when nothing moved on disk", async () => {
    await mount();
    await type("b.rs", "b typed\n");
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(host.querySelector(".canvas-conflict")).toBeNull();
    expect(boxFor("b.rs").value).toBe("b typed\n");
  });

  it("says only what's selected in the file showing", async () => {
    await mount();
    onSelect.mockClear();
    const hidden = boxFor("a.rs");
    await act(async () => {
      hidden.setSelectionRange(0, 1);
      hidden.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    });
    expect(onSelect).not.toHaveBeenCalled();
    await act(async () => tab("a.rs").click());
    expect(onSelect).toHaveBeenLastCalledWith({ from: 1, to: 1, text: "a" });
  });

  it("draws nothing while Changes has the column, and keeps the documents", async () => {
    await mount(two(), false);
    expect(panels()).toHaveLength(0);
    expect(typeof flushRef.current).toBe("function");
  });
});
