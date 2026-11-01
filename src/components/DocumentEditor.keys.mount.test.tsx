// @vitest-environment jsdom
//
// Tab and Enter reach the box as code editing, and stay out of the way otherwise.
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { DocumentEditor } from "./DocumentEditor";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root;

function Harness({ start, path }: { start: string; path: string }) {
  const [text, setText] = useState(start);
  return (
    <DocumentEditor
      text={text}
      path={path}
      reveal={null}
      flash={null}
      onSelect={() => {}}
      onChange={setText}
    />
  );
}

const box = () => document.querySelector<HTMLTextAreaElement>(".doc-input")!;
const mount = async (start: string, path = "a.rs") => {
  await act(async () => root.render(<Harness start={start} path={path} />));
};
const press = async (key: string, at: [number, number], more: KeyboardEventInit = {}) => {
  box().setSelectionRange(...at);
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...more });
  await act(async () => box().dispatchEvent(event));
  return event;
};

beforeEach(() => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  document.body.innerHTML = "";
});

describe("keys in the editor", () => {
  it("indents on Tab, in the file's own indent, and keeps focus", async () => {
    await mount("a\n  b\n");
    const event = await press("Tab", [0, 0]);
    expect(event.defaultPrevented).toBe(true);
    expect(box().value).toBe("  a\n  b\n");
    expect(box().selectionStart).toBe(2);
  });

  it("outdents on Shift-Tab", async () => {
    await mount("a\n  b\n");
    await press("Tab", [4, 4], { shiftKey: true });
    expect(box().value).toBe("a\nb\n");
  });

  it("does nothing for Shift-Tab on a line with no indent, but still keeps focus", async () => {
    await mount("a\n");
    const event = await press("Tab", [0, 0], { shiftKey: true });
    expect(event.defaultPrevented).toBe(true);
    expect(box().value).toBe("a\n");
  });

  it("opens a block on Enter between brackets", async () => {
    await mount("fn a() {}\n  x\n");
    await press("Enter", [8, 8]);
    expect(box().value).toBe("fn a() {\n  \n}\n  x\n");
    expect(box().selectionStart).toBe(11);
  });

  it("leaves chords with a modifier alone", async () => {
    await mount("a\n");
    const event = await press("Tab", [0, 0], { ctrlKey: true });
    expect(event.defaultPrevented).toBe(false);
    expect(box().value).toBe("a\n");
  });

  it("leaves other keys to the browser", async () => {
    await mount("a\n");
    expect((await press("a", [0, 0])).defaultPrevented).toBe(false);
  });
});
