// @vitest-environment jsdom
//
// The find bar, through the keys that open it. What matters is where the caret
// ends up, and that searching says nothing to the model until it stops.
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";

import type { Selection } from "../bindings/Selection";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { CanvasSlot } from "./CanvasSlot";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const TEXT = "one retry\ntwo\nthree retry\nfour RETRY\n";
let root: Root;
let onSelect: Mock<(selection: Selection | null) => void>;
let onClose: Mock<() => void>;

function Harness({ path }: { path: string }) {
  const [mark] = useState(() => () => {});
  return (
    <CanvasSlot
      path={path}
      reveal={null}
      workspace="/w"
      showing
      onMark={mark}
      onSelect={onSelect}
      onAsk={() => {}}
      onClose={onClose}
    />
  );
}

const mount = async (path = "a.rs") => {
  await act(async () => root.render(<Harness path={path} />));
};
const editor = () => document.querySelector<HTMLTextAreaElement>(".doc-input")!;
const input = () => document.querySelector<HTMLInputElement>(".doc-find-input");
const key = async (init: KeyboardEventInit, on: Element = editor()) => {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  await act(async () => {
    on.dispatchEvent(event);
  });
  return event;
};
const openFind = () => key({ key: "f", ctrlKey: true });
const type = (text: string) =>
  act(async () => {
    const box = input()!;
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(box, text);
    box.dispatchEvent(new Event("input", { bubbles: true }));
  });
const count = () => document.querySelector(".doc-find-count")?.textContent;

beforeEach(() => {
  // jsdom has no canvas to measure a character with, and says so on every call.
  HTMLCanvasElement.prototype.getContext = (() => null) as never;
  onSelect = vi.fn();
  onClose = vi.fn();
  invoke.mockImplementation((command: string, args: { path: string }) =>
    command === "open_document"
      ? Promise.resolve({ path: args.path, text: TEXT, lines: 4, fingerprint: "1" })
      : Promise.resolve(),
  );
  const host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  document.body.innerHTML = "";
  invoke.mockReset();
});

describe("find in a file", () => {
  it("opens on the key, counts, and paints every match", async () => {
    await mount();
    const event = await openFind();
    expect(event.defaultPrevented).toBe(true);
    await type("retry");
    expect(count()).toBe("1 of 3");
    expect(document.querySelectorAll(".ink-hit")).toHaveLength(3);
    expect(document.querySelectorAll(".ink-hit.current")).toHaveLength(1);
    // The paint is the same characters, still under the box.
    expect(document.querySelector(".doc-ink")?.textContent?.startsWith(TEXT)).toBe(true);
  });

  it("steps forward and back on Enter, and wraps", async () => {
    await mount();
    await openFind();
    await type("retry");
    await key({ key: "Enter" }, input()!);
    expect(count()).toBe("2 of 3");
    await key({ key: "Enter", shiftKey: true }, input()!);
    await key({ key: "Enter", shiftKey: true }, input()!);
    expect(count()).toBe("3 of 3");
  });

  it("steps on F3 as well", async () => {
    await mount();
    await openFind();
    await type("retry");
    await key({ key: "F3" }, input()!);
    expect(count()).toBe("2 of 3");
  });

  it("says when there's nothing", async () => {
    await mount();
    await openFind();
    await type("nope");
    expect(count()).toBe("No matches");
  });

  it("selects nothing for the model while searching, then the match it stopped on", async () => {
    await mount();
    await openFind();
    onSelect.mockClear();
    await type("retry");
    await key({ key: "Enter" }, input()!);
    expect(onSelect).not.toHaveBeenCalled();
    await key({ key: "Escape" }, input()!);
    expect(input()).toBeNull();
    expect(onSelect).toHaveBeenLastCalledWith({ from: 3, to: 3, text: "retry" });
    expect(document.activeElement).toBe(editor());
  });

  it("closes the bar on Escape without closing the file", async () => {
    await mount();
    await openFind();
    await key({ key: "Escape" }, input()!);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("begins at the caret rather than the top", async () => {
    await mount();
    editor().setSelectionRange(12, 12);
    await openFind();
    await type("retry");
    expect(count()).toBe("2 of 3");
  });

  it("searches for what's selected", async () => {
    await mount();
    editor().setSelectionRange(10, 13);
    await openFind();
    expect(input()?.value).toBe("two");
    expect(count()).toBe("1 of 1");
  });

  it("goes to a line on Control and G", async () => {
    await mount();
    await key({ key: "g", ctrlKey: true });
    expect(input()?.value).toBe(":");
    await type(":3");
    expect(count()).toBe("Go to line 3");
    await key({ key: "Enter" }, input()!);
    expect(input()).toBeNull();
    expect(editor().selectionStart).toBe(TEXT.indexOf("three"));
  });

  it("stops at the last line of a short file", async () => {
    await mount();
    await key({ key: "g", ctrlKey: true });
    await type(":900");
    expect(count()).toBe("Go to line 5");
  });

  it("switches a Markdown preview to source to search it", async () => {
    await mount("a.md");
    expect(editor()).toBeNull();
    await key({ key: "f", ctrlKey: true }, document.body);
    expect(editor()).not.toBeNull();
    expect(input()).not.toBeNull();
  });

  it("leaves the keys alone when focus is somewhere else", async () => {
    await mount();
    const elsewhere = document.body.appendChild(document.createElement("textarea"));
    const event = await key({ key: "f", ctrlKey: true }, elsewhere);
    expect(event.defaultPrevented).toBe(false);
    expect(input()).toBeNull();
  });
});

describe("replace in a file", () => {
  const swap = () => document.querySelector<HTMLInputElement>(".doc-replace-input");
  const typeSwap = (text: string) =>
    act(async () => {
      const box = swap()!;
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(box, text);
      box.dispatchEvent(new Event("input", { bubbles: true }));
    });
  // Each its own file: what a test types is kept per path, as unsaved work is.
  const button = (name: string) =>
    [...document.querySelectorAll<HTMLButtonElement>(".doc-find-action")].find(
      (b) => b.textContent === name,
    )!;
  const click = (el: HTMLElement) =>
    act(async () => {
      el.click();
    });

  it("opens with the replace row on its key", async () => {
    await mount("r1.rs");
    await key({ key: "h", ctrlKey: true });
    expect(input()).not.toBeNull();
    expect(swap()).not.toBeNull();
  });

  it("replaces one match, moves on to the next, and keeps the focus in the bar", async () => {
    await mount("r2.rs");
    await key({ key: "h", ctrlKey: true });
    await type("retry");
    await typeSwap("again");
    await key({ key: "Enter" }, swap()!);
    expect(editor().value).toBe("one again\ntwo\nthree retry\nfour RETRY\n");
    expect(count()).toBe("1 of 2");
    expect(document.activeElement).toBe(swap());
  });

  it("doesn't step onto what it just put in", async () => {
    await mount("r3.rs");
    await key({ key: "h", ctrlKey: true });
    await type("retry");
    await typeSwap("retry retry");
    await click(button("Replace"));
    // The next one is the old second match, not the copy just written.
    expect(count()).toBe("3 of 4");
  });

  it("replaces every match and says how many", async () => {
    await mount("r4.rs");
    await key({ key: "h", ctrlKey: true });
    await type("retry");
    await typeSwap("again");
    await click(button("All"));
    expect(editor().value).toBe("one again\ntwo\nthree again\nfour again\n");
    expect(count()).toBe("Replaced 3");
  });

  it("fills groups in a pattern", async () => {
    await mount("r5.rs");
    await key({ key: "h", ctrlKey: true });
    await click(document.querySelector<HTMLButtonElement>('[aria-label="Regular expression"]')!);
    await type("(\\w+) retry");
    await typeSwap("retry $1");
    await click(button("All"));
    // A pattern ignores case until Match case is on.
    expect(editor().value).toBe("retry one\ntwo\nretry three\nretry four\n");
  });

  it("says a broken pattern is broken", async () => {
    await mount("r6.rs");
    await openFind();
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Regular expression"]')!.click();
    });
    await type("(");
    expect(count()).toBe("Not a valid pattern");
  });

  it("toggles from the keyboard", async () => {
    await mount("r7.rs");
    await openFind();
    await type("retry");
    expect(count()).toBe("1 of 3");
    await key({ key: "c", code: "KeyC", altKey: true }, input()!);
    expect(count()).toBe("1 of 2");
    expect(
      document.querySelector('[aria-label="Match case"]')?.getAttribute("aria-pressed"),
    ).toBe("true");
  });
});

