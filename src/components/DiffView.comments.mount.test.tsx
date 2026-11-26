// @vitest-environment jsdom
//
// Commenting on a line of a diff: offered only where a diff is given somewhere
// to send the comment, written in a box under the line, and handed over with
// the line it is about. What happens to it after that is the store's business,
// and `store.remarks.test.ts` covers it.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DiffLine, FileDiff } from "../lib/api";
import { remarkOn, sameLine } from "./ChangesDrawer";
import { DiffView } from "./DiffView";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const REMOVED: DiffLine = { kind: "removed", text: "    let x = 1;", old_line: 2, new_line: null };
const ADDED: DiffLine = { kind: "added", text: "    let x = 2;", old_line: null, new_line: 2 };

const DIFF: FileDiff = {
  path: "src/widget.rs",
  created: false,
  deleted: false,
  added: 1,
  removed: 1,
  elided: 0,
  hunks: [
    {
      lines: [{ kind: "context", text: "fn widget() {", old_line: 1, new_line: 1 }, REMOVED, ADDED],
    },
  ],
};

let unmount: (() => void) | null = null;
afterEach(() => {
  unmount?.();
  unmount = null;
  document.body.innerHTML = "";
});

const mount = (props: Partial<Parameters<typeof DiffView>[0]> = {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => root.render(<DiffView diff={DIFF} {...props} />));
  unmount = () => act(() => root.unmount());
  return host;
};

/** Types into a controlled textarea the way React hears it. */
const type = (box: HTMLTextAreaElement, text: string) =>
  act(() => {
    const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
    set.call(box, text);
    box.dispatchEvent(new Event("input", { bubbles: true }));
  });

const key = (box: HTMLTextAreaElement, key: string) =>
  act(() => {
    box.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
  });

describe("commenting on a line", () => {
  it("is not offered where nobody takes the comment, like the permission dialog", () => {
    expect(renderToStaticMarkup(<DiffView diff={DIFF} />)).not.toContain("diff-comment-add");
  });

  it("hands over the line and what was written about it", () => {
    const onComment = vi.fn();
    const host = mount({ onComment });

    act(() => host.querySelector<HTMLButtonElement>('[aria-label="Comment on line 2"]')!.click());
    const box = host.querySelector<HTMLTextAreaElement>(".diff-comment-box textarea")!;
    type(box, "  why two?  ");
    key(box, "Enter");

    expect(onComment).toHaveBeenCalledWith(ADDED, "why two?");
    expect(host.querySelector(".diff-comment-box")).toBeNull();
  });

  it("names a removed line as removed", () => {
    const host = mount({ onComment: vi.fn() });
    expect(host.querySelector('[aria-label="Comment on removed line 2"]')).not.toBeNull();
  });

  it("drops the box and what was in it on Escape", () => {
    const onComment = vi.fn();
    const host = mount({ onComment });

    act(() => host.querySelector<HTMLButtonElement>('[aria-label="Comment on line 2"]')!.click());
    const box = host.querySelector<HTMLTextAreaElement>(".diff-comment-box textarea")!;
    type(box, "never mind");
    key(box, "Escape");

    expect(onComment).not.toHaveBeenCalled();
    expect(host.querySelector(".diff-comment-box")).toBeNull();
  });

  it("does not add an empty comment", () => {
    const onComment = vi.fn();
    const host = mount({ onComment });

    act(() => host.querySelector<HTMLButtonElement>('[aria-label="Comment on line 2"]')!.click());
    key(host.querySelector<HTMLTextAreaElement>(".diff-comment-box textarea")!, "Enter");

    expect(onComment).not.toHaveBeenCalled();
  });

  it("marks a line that already has one", () => {
    const host = mount({ onComment: vi.fn(), commented: (line) => line === ADDED });
    const marked = [...host.querySelectorAll(".diff-line.commented")];
    expect(marked).toHaveLength(1);
    expect(marked[0].textContent).toContain("let x = 2;");
  });
});

describe("a comment's anchor", () => {
  it("numbers a removed line by where it was, and any other by where it is", () => {
    expect(remarkOn(3, "src/widget.rs", REMOVED, "x").anchor).toMatchObject({ side: "removed", line: 2 });
    expect(remarkOn(3, "src/widget.rs", ADDED, "x").anchor).toMatchObject({ side: "added", line: 2 });
  });

  it("tells the removed line 2 from the added line 2", () => {
    const remark = remarkOn(3, "src/widget.rs", ADDED, "x");
    expect(sameLine(remark, 3, "src/widget.rs", ADDED)).toBe(true);
    expect(sameLine(remark, 3, "src/widget.rs", REMOVED)).toBe(false);
    expect(sameLine(remark, 4, "src/widget.rs", ADDED)).toBe(false);
  });
});
