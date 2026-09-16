// @vitest-environment jsdom
//
// jsdom lays nothing out, so the two numbers a browser would supply — how wide
// the box is and how tall its text wants to be — are supplied here, and the
// observer that reports a resize is a stand-in the test can fire.
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ProseEditor } from "./ProseEditor";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

it("measures itself again when its width changes, not only when its text does", async () => {
  // Measured on a change of text alone, a note narrowed by the canvas split
  // kept its old height: the last lines cut off, the paint drifting off them.
  let resized: () => void = () => {};
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        resized = callback;
      }
      observe() {}
      disconnect() {}
    },
  );
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => {
    createRoot(host).render(<ProseEditor text="A paragraph long enough to wrap." onChange={() => {}} />);
  });
  const box = host.querySelector("textarea")!;
  let width = 600;
  let wants = 40;
  Object.defineProperty(box, "clientWidth", { get: () => width });
  Object.defineProperty(box, "scrollHeight", { get: () => wants });

  resized();
  expect(box.style.height).toBe("40px");

  // Narrower, so the same text wraps onto more lines.
  width = 300;
  wants = 80;
  resized();
  expect(box.style.height).toBe("80px");

  // A resize that is only the height this set is not answered — answering it
  // would be a loop that only happens to settle.
  wants = 120;
  resized();
  expect(box.style.height).toBe("80px");
});

/*
 * The syntax help, pressed the way somebody would press it. `lib/prose.test.ts`
 * covers what is offered and what Enter writes; this covers the keys, and that
 * what is written arrives as the editor's own text. jsdom has no `insertText`,
 * so these run the fallback that sets the text directly — the route a real
 * webview takes is the one that keeps ⌘Z, and it sends the same `input`.
 * Where the list lands is the `notes-complete` screenshot's to check.
 */
describe("help with the syntax", () => {
  /** The editor with somewhere for its text to live, as the pane gives it. */
  function Harness({ start, sketches }: { start: string; sketches: string[] }) {
    const [text, setText] = useState(start);
    return <ProseEditor text={text} onChange={setText} sketches={sketches} />;
  }

  let unmount: (() => void) | null = null;
  afterEach(() => {
    unmount?.();
    unmount = null;
  });

  async function mount(start = "", sketches: string[] = []) {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => {
      root.render(<Harness start={start} sketches={sketches} />);
    });
    unmount = () => act(() => root.unmount());
    return host;
  }

  const box = (host: HTMLElement) => host.querySelector("textarea")!;
  const rows = (host: HTMLElement) =>
    [...host.querySelectorAll(".prose-label")].map((row) => row.textContent);

  /** Types, the way React hears it — see `SqlEditor.mount.test.tsx`. */
  function type(host: HTMLElement, text: string) {
    const area = box(host);
    const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
    act(() => {
      set.call(area, text);
      area.setSelectionRange(text.length, text.length);
      area.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }

  function press(host: HTMLElement, key: string, over: KeyboardEventInit = {}) {
    let kept = true;
    act(() => {
      kept = box(host).dispatchEvent(
        new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...over }),
      );
    });
    // Whether the browser's own handling of the key was left to happen.
    return kept;
  }

  it("opens the block list on a slash and writes the one taken", async () => {
    const host = await mount("Intro\n");
    type(host, "Intro\n/he");
    expect(rows(host)).toEqual(["Heading 1", "Heading 2", "Heading 3"]);
    press(host, "ArrowDown");
    press(host, "Enter");
    expect(box(host).value).toBe("Intro\n## ");
    expect(box(host).selectionStart).toBe("Intro\n## ".length);
    expect(host.querySelector(".prose-menu")).toBeNull();
    // What was written is painted, character for character.
    expect(host.querySelector(".prose-ink")?.textContent).toBe(`${box(host).value}\n`);
  });

  it("goes straight on to the language once a code block is made", async () => {
    const host = await mount();
    type(host, "/code");
    press(host, "Enter");
    expect(box(host).value).toBe("```\n\n```");
    expect(box(host).selectionStart).toBe(3);
    expect(rows(host)[0]).toBe("mermaid");
  });

  it("offers the notebook's sketches inside an image's brackets", async () => {
    const host = await mount("", ["Auth flow", "Token map"]);
    type(host, "See ![");
    expect(rows(host)).toEqual(["Auth flow", "Token map"]);
    press(host, "Tab");
    expect(box(host).value).toBe("See ![Auth flow](<Auth flow.excalidraw>)");
  });

  it("closes on Escape and leaves what was typed alone", async () => {
    const host = await mount();
    type(host, "/");
    expect(host.querySelector(".prose-menu")).not.toBeNull();
    press(host, "Escape");
    expect(host.querySelector(".prose-menu")).toBeNull();
    expect(box(host).value).toBe("/");
  });

  it("opens on ⌃Space on an empty line", async () => {
    const host = await mount();
    press(host, " ", { ctrlKey: true });
    expect(rows(host)[0]).toBe("Heading 1");
  });

  it("carries a list on with Enter, and ends it on an empty item", async () => {
    const host = await mount();
    type(host, "- [x] one");
    expect(press(host, "Enter")).toBe(false);
    expect(box(host).value).toBe("- [x] one\n- [ ] ");
    press(host, "Enter");
    expect(box(host).value).toBe("- [x] one\n");
  });

  it("leaves Enter to the browser off a list, and ⇧Enter always", async () => {
    const host = await mount();
    type(host, "A sentence.");
    expect(press(host, "Enter")).toBe(true);
    type(host, "- an item");
    expect(press(host, "Enter", { shiftKey: true })).toBe(true);
    expect(box(host).value).toBe("- an item");
  });
});
