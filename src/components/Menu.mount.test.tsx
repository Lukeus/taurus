// @vitest-environment jsdom
//
// A card's ⋯ behaves as a menu button: keyboard in, keyboard out, and never
// closes the drawer it sits in.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Menu } from "./Menu";
import { Modal } from "./Modal";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});
afterEach(() => {
  act(() => root.unmount());
  document.body.innerHTML = "";
});

const button = () => host.querySelector<HTMLButtonElement>(".menu-button")!;
const items = () => [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
const key = (el: Element, k: string) =>
  act(() => {
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));
  });

function draw(edit = vi.fn(), remove = vi.fn()) {
  act(() =>
    root.render(
      <Menu
        label="Actions for github"
        items={[
          { label: "Edit", onSelect: edit },
          false,
          { label: "Disable", onSelect: () => {}, disabled: true },
          { label: "Delete…", onSelect: remove, danger: true },
        ]}
      />,
    ),
  );
  return { edit, remove };
}

describe("a card's menu", () => {
  it("is named for its card, and lists only the items that apply", () => {
    draw();
    expect(button().getAttribute("aria-label")).toBe("Actions for github");
    act(() => button().click());
    expect(items().map((i) => i.textContent)).toEqual(["Edit", "Disable", "Delete…"]);
    expect(document.querySelector(".menu-item.danger")?.textContent).toBe("Delete…");
  });

  it("opens on the first item and walks past a disabled one", () => {
    draw();
    act(() => button().focus());
    key(button(), "ArrowDown");
    expect(document.activeElement?.textContent).toBe("Edit");
    key(document.activeElement!, "ArrowDown");
    expect(document.activeElement?.textContent).toBe("Delete…");
    key(document.activeElement!, "ArrowDown");
    expect(document.activeElement?.textContent).toBe("Edit");
  });

  it("opens on the last item from ↑", () => {
    draw();
    key(button(), "ArrowUp");
    expect(document.activeElement?.textContent).toBe("Delete…");
  });

  it("runs the chosen item and closes", () => {
    const { remove } = draw();
    act(() => button().click());
    act(() => items()[2].click());
    expect(remove).toHaveBeenCalledTimes(1);
    expect(items()).toEqual([]);
  });

  it("closes on Escape, hands focus back, and leaves the drawer open", () => {
    const onClose = vi.fn();
    act(() =>
      root.render(
        <Modal onClose={onClose}>
          <aside>
            <Menu label="Actions" items={[{ label: "Edit", onSelect: () => {} }]} />
          </aside>
        </Modal>,
      ),
    );
    act(() => button().click());
    key(document.activeElement!, "Escape");
    expect(items()).toEqual([]);
    expect(document.activeElement).toBe(button());
    expect(onClose).not.toHaveBeenCalled();
  });

  it("draws nothing when no item applies", () => {
    act(() => root.render(<Menu label="Actions" items={[false, null]} />));
    expect(host.querySelector(".menu-button")).toBeNull();
  });
});
