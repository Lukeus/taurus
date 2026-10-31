// @vitest-environment jsdom
//
// Coming off a file. The canvas saves a moment after typing stops, so there is
// always a moment when the screen and the disk disagree, and a save can still be
// out when the editor moves on. None of that may cost typing or touch the wrong
// file.
import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { CanvasSlot, keptIn } from "./CanvasSlot";
import { SAVE_AFTER_MS } from "../lib/document";
import { useStore } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const pane = { size: 400, dragging: false, min: 200, max: 800, start() {}, nudge() {} };
const doc = (path: string, text: string, fingerprint = "1") => ({
  path,
  text,
  lines: 1,
  fingerprint,
});

/** What each file reads as. A test changes it to change the disk. */
let disk: Record<string, ReturnType<typeof doc>>;
/** Saves in the order asked, with what each said. */
let saves: { path: string; text: string; fingerprint: string }[];
/** Replaces how a save answers. By default it writes. */
let answer: (args: { path: string; text: string }) => Promise<unknown>;

const FOLDER = "/src/project";
let root: Root;
let host: HTMLElement;
let props: Partial<ComponentProps<typeof CanvasSlot>> & { path: string };

const render = () =>
  root.render(
    <CanvasSlot
      reveal={null}
      workspace={FOLDER}
      showing
      pane={pane}
      onUnsaved={() => {}}
      onSelect={() => {}}
      onAsk={() => {}}
      onClose={() => {}}
      {...props}
    />,
  );

const open = async (path: string, more: Partial<typeof props> = {}) => {
  props = { path, ...more };
  await act(async () => render());
  await settle();
};
const close = () => act(async () => root.render(null));
const settle = () => act(async () => vi.advanceTimersByTimeAsync(0));
const box = () => host.querySelector(".doc-input") as HTMLTextAreaElement;

const type = (text: string) =>
  act(async () => {
    const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
    set.call(box(), text);
    box().dispatchEvent(new Event("input", { bubbles: true }));
  });

beforeEach(() => {
  vi.useFakeTimers();
  disk = { "a.rs": doc("a.rs", "a\n"), "b.rs": doc("b.rs", "b\n") };
  saves = [];
  answer = ({ path, text }) => {
    disk[path] = doc(path, text, String(saves.length + 1));
    return Promise.resolve({ type: "saved", document: disk[path] });
  };
  invoke.mockImplementation((command: string, raw: unknown) => {
    const args = raw as { path: string; text: string; fingerprint: string };
    if (command === "open_document") return Promise.resolve(disk[args.path]);
    if (command === "save_document") {
      saves.push({ path: args.path, text: args.text, fingerprint: args.fingerprint });
      return answer(args);
    }
    return Promise.resolve([]);
  });
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  useStore.setState({ error: null, wrote: null });
});

afterEach(async () => {
  await close();
  await settle();
  vi.useRealTimers();
  document.body.innerHTML = "";
  invoke.mockReset();
  useStore.setState({ error: null, wrote: null });
});

describe("a save answered after the editor moved on", () => {
  it("doesn't make the old file current, or write the new one's text into it", async () => {
    let land: (value: unknown) => void = () => {};
    answer = ({ path, text }) =>
      new Promise((resolve) => {
        land = () => resolve({ type: "saved", document: doc(path, text, "2") });
      });
    await open("a.rs");
    await type("a edited\n");
    await act(async () => vi.advanceTimersByTimeAsync(SAVE_AFTER_MS));
    expect(saves).toHaveLength(1);

    await open("b.rs");
    expect(box().value).toBe("b\n");
    await act(async () => land(undefined));
    await act(async () => vi.advanceTimersByTimeAsync(SAVE_AFTER_MS * 2));

    expect(saves).toHaveLength(1);
    expect(box().value).toBe("b\n");
  });

  it("ignores a re-read of the old file that lands under the new one", async () => {
    await open("a.rs");
    let land: (value: unknown) => void = () => {};
    const read = invoke.getMockImplementation()!;
    invoke.mockImplementation((command: string, raw: unknown) =>
      command === "open_document" && (raw as { path: string }).path === "a.rs"
        ? new Promise((resolve) => (land = resolve))
        : read(command, raw),
    );
    await act(async () => useStore.setState({ wrote: { paths: ["a.rs"], at: 1 } }));
    await open("b.rs");

    await act(async () => land(doc("a.rs", "a rewritten\n", "9")));
    await settle();

    expect(box().value).toBe("b\n");
  });
});

describe("leaving with typing not yet written", () => {
  it("writes it when the editor is closed inside the debounce", async () => {
    await open("a.rs");
    await type("a edited\n");
    await close();
    await settle();

    expect(saves).toEqual([{ path: "a.rs", text: "a edited\n", fingerprint: "1" }]);
  });

  it("writes it to the file being left when another one opens", async () => {
    await open("a.rs");
    await type("a edited\n");
    await open("b.rs");

    expect(saves).toEqual([{ path: "a.rs", text: "a edited\n", fingerprint: "1" }]);
    expect(box().value).toBe("b\n");
  });

  it("waits for a save still out, and sends the rest with its fingerprint", async () => {
    let land: () => void = () => {};
    answer = ({ path, text }) =>
      new Promise((resolve) => {
        land = () => resolve({ type: "saved", document: doc(path, text, "2") });
      });
    await open("a.rs");
    await type("first\n");
    await act(async () => vi.advanceTimersByTimeAsync(SAVE_AFTER_MS));
    await type("first and second\n");
    await close();
    expect(saves).toHaveLength(1);

    await act(async () => land());
    await settle();

    expect(saves[1]).toEqual({ path: "a.rs", text: "first and second\n", fingerprint: "2" });
  });

  it("keeps the newest typing when the save it queued behind fails", async () => {
    let fail: () => void = () => {};
    answer = () => new Promise((_, reject) => (fail = () => reject("disk full")));
    await open("a.rs");
    await type("first\n");
    await act(async () => vi.advanceTimersByTimeAsync(SAVE_AFTER_MS));
    await type("first and second\n");
    await close();
    await act(async () => fail());
    await settle();

    answer = ({ path, text }) => Promise.resolve({ type: "saved", document: doc(path, text, "3") });
    await open("a.rs");
    expect(box().value).toBe("first and second\n");
  });
});

describe("leaving what can't be written", () => {
  it("keeps the buffer when closed with a conflict open, and asks again on reopening", async () => {
    answer = () => Promise.resolve({ type: "stale", current: doc("a.rs", "theirs\n", "2") });
    await open("a.rs");
    await type("mine\n");
    await act(async () => vi.advanceTimersByTimeAsync(SAVE_AFTER_MS));
    expect(host.querySelector(".canvas-conflict")).not.toBeNull();
    const asked = saves.length;

    disk["a.rs"] = doc("a.rs", "theirs\n", "2");
    await close();
    expect(saves).toHaveLength(asked);
    expect(useStore.getState().error).toContain("a.rs");

    await open("a.rs");
    expect(box().value).toBe("mine\n");
    expect(host.querySelector(".canvas-conflict")).not.toBeNull();
  });

  it("keeps the buffer when the last save failed and the one on the way out does too", async () => {
    answer = () => Promise.reject("disk full");
    await open("a.rs");
    await type("mine\n");
    await act(async () => vi.advanceTimersByTimeAsync(SAVE_AFTER_MS));
    expect(host.querySelector(".canvas-state.failed")).not.toBeNull();

    await close();
    await settle();
    expect(useStore.getState().error).toContain("disk full");
    expect(useStore.getState().error).toContain("kept");

    // Nothing has written it since, so the kept version simply saves.
    answer = ({ path, text }) => Promise.resolve({ type: "saved", document: doc(path, text, "3") });
    await open("a.rs");
    expect(box().value).toBe("mine\n");
    await act(async () => vi.advanceTimersByTimeAsync(SAVE_AFTER_MS));
    expect(saves[saves.length - 1].text).toBe("mine\n");
  });

  it("keeps the buffer when a save on the way out is refused as stale", async () => {
    answer = () => Promise.resolve({ type: "stale", current: doc("a.rs", "theirs\n", "2") });
    await open("a.rs");
    await type("mine\n");
    disk["a.rs"] = doc("a.rs", "theirs\n", "2");
    await close();
    await settle();

    await open("a.rs");
    expect(box().value).toBe("mine\n");
    expect(host.querySelector(".canvas-conflict")).not.toBeNull();
  });

  it("forgets a kept version once it has been put back", async () => {
    answer = () => Promise.reject("disk full");
    await open("a.rs");
    await type("mine\n");
    await close();
    await settle();
    expect(keptIn(FOLDER)).toBe(1);

    answer = ({ path, text }) => Promise.resolve({ type: "saved", document: doc(path, text, "3") });
    await open("a.rs");
    expect(keptIn(FOLDER)).toBe(0);
  });
});

describe("the workspace moving", () => {
  it("never writes to the same path in the new folder, and keeps the text for the old one", async () => {
    await open("a.rs");
    await type("mine\n");
    await open("a.rs", { workspace: "/src/other" });

    expect(saves).toEqual([]);
    expect(keptIn(FOLDER)).toBe(1);
    expect(box().value).toBe("a\n");

    await open("a.rs", { workspace: FOLDER });
    expect(box().value).toBe("mine\n");
  });

  it("writes on being asked, before the switch", async () => {
    const flushRef: { current: (() => Promise<void>) | null } = { current: null };
    await open("a.rs", { flushRef });
    await type("mine\n");

    await act(async () => flushRef.current!());

    expect(saves).toEqual([{ path: "a.rs", text: "mine\n", fingerprint: "1" }]);
  });
});
