// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { onTabKeys } from "./tabs";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

afterEach(() => {
  document.body.innerHTML = "";
});

function Switch() {
  const [on, setOn] = useState("one");
  return (
    <div role="tablist" aria-label="View" onKeyDown={onTabKeys}>
      {["one", "two", "three", "four"].map((name) => (
        <button
          key={name}
          role="tab"
          aria-selected={on === name}
          tabIndex={on === name ? 0 : -1}
          disabled={name === "two"}
          onClick={() => setOn(name)}
        >
          {name}
        </button>
      ))}
    </div>
  );
}

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  act(() => createRoot(host).render(<Switch />));
  const tab = (name: string) =>
    [...host.querySelectorAll<HTMLButtonElement>('[role="tab"]')].find(
      (t) => t.textContent === name,
    )!;
  const press = (key: string) =>
    act(() => {
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key, bubbles: true }),
      );
    });
  const selected = () =>
    host.querySelector('[role="tab"][aria-selected="true"]')?.textContent;
  return { tab, press, selected };
}

describe("a row of tabs", () => {
  it("moves with the arrows, selecting as it goes and stepping over a disabled tab", () => {
    const ui = mount();
    ui.tab("one").focus();
    ui.press("ArrowRight");
    expect(ui.selected()).toBe("three");
    expect(document.activeElement).toBe(ui.tab("three"));
    ui.press("ArrowRight");
    ui.press("ArrowRight");
    // Off the end wraps to the front.
    expect(ui.selected()).toBe("one");
    ui.press("ArrowLeft");
    expect(ui.selected()).toBe("four");
  });

  it("jumps to either end with Home and End", () => {
    const ui = mount();
    ui.tab("one").focus();
    ui.press("End");
    expect(ui.selected()).toBe("four");
    ui.press("Home");
    expect(ui.selected()).toBe("one");
  });

  it("is reached once by Tab, on the selected tab", () => {
    const ui = mount();
    expect(ui.tab("one").tabIndex).toBe(0);
    expect(ui.tab("three").tabIndex).toBe(-1);
  });
});
