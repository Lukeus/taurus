// @vitest-environment jsdom
//
// The dropdown has to behave like the native one it replaced, and one thing
// better: closing it never closes the drawer it was opened in.
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Modal } from "./Modal";
import { Select, type SelectOption } from "./Select";
import { choose, optionsOf, shown } from "../test/select";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const MODELS: SelectOption[] = [
  { value: "qwen3:8b", label: "qwen3:8b", hint: "40k", group: "ollama" },
  { value: "llama3.1", label: "llama3.1", disabled: true, group: "ollama" },
  { value: "gpt-4o", label: "gpt-4o", hint: "128k", group: "apim" },
  { value: "gemma3", label: "gemma3", group: "apim" },
];

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

function Harness({ onChange }: { onChange?: (v: string) => void }) {
  const [value, setValue] = useState("qwen3:8b");
  return (
    <>
      <label htmlFor="model">Model</label>
      <Select
        id="model"
        value={value}
        options={MODELS}
        onChange={(v) => {
          setValue(v);
          onChange?.(v);
        }}
      />
    </>
  );
}

const trigger = () => host.querySelector<HTMLButtonElement>('[role="combobox"]')!;
const list = () => document.querySelector('[role="listbox"]');
const key = (k: string) =>
  act(() => {
    trigger().dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));
  });

describe("the dropdown", () => {
  it("shows the chosen label and is named by its label", () => {
    act(() => root.render(<Harness />));
    expect(shown(trigger())).toBe("qwen3:8b");
    expect(trigger().labels?.[0]?.textContent).toBe("Model");
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
  });

  it("lists every option under its group, with its hint", () => {
    act(() => root.render(<Harness />));
    act(() => trigger().click());
    expect(optionsOf(trigger())).toEqual(["qwen3:8b", "llama3.1", "gpt-4o", "gemma3"]);
    const groups = [...document.querySelectorAll(".select-group")].map((g) => g.textContent);
    expect(groups).toEqual(["ollama", "apim"]);
    expect(document.querySelector('[data-value="gpt-4o"] .select-hint')?.textContent).toBe("128k");
  });

  it("walks past disabled rows with the arrows and chooses with Enter", () => {
    const onChange = vi.fn();
    act(() => root.render(<Harness onChange={onChange} />));
    act(() => trigger().focus());
    key("ArrowDown"); // opens on the chosen row
    expect(list()).not.toBeNull();
    key("ArrowDown"); // skips llama3.1
    expect(trigger().getAttribute("aria-activedescendant")).toBe(
      document.querySelector('[data-value="gpt-4o"]')!.id,
    );
    key("Enter");
    expect(onChange).toHaveBeenCalledWith("gpt-4o");
    expect(list()).toBeNull();
    expect(shown(trigger())).toBe("gpt-4o");
  });

  it("jumps to a name as it is typed", () => {
    const onChange = vi.fn();
    act(() => root.render(<Harness onChange={onChange} />));
    key("g");
    key("e");
    key("Enter");
    expect(onChange).toHaveBeenCalledWith("gemma3");
  });

  it("does not report a choice of the row that was already chosen", () => {
    const onChange = vi.fn();
    act(() => root.render(<Harness onChange={onChange} />));
    choose(trigger(), "qwen3:8b");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("closes on Escape without closing the drawer it is in", () => {
    const onClose = vi.fn();
    act(() =>
      root.render(
        <Modal onClose={onClose}>
          <aside>
            <Harness />
          </aside>
        </Modal>,
      ),
    );
    act(() => trigger().click());
    expect(list()).not.toBeNull();
    act(() => {
      trigger().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(list()).toBeNull();
    expect(onClose).not.toHaveBeenCalled();

    // With the list gone, Escape is the drawer's again.
    act(() => {
      trigger().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("closes on a press outside the list, and the press goes no further", () => {
    const onClose = vi.fn();
    act(() =>
      root.render(
        <Modal onClose={onClose}>
          <aside>
            <Harness />
          </aside>
        </Modal>,
      ),
    );
    act(() => trigger().click());
    const layer = document.querySelector(".select-layer")!;
    act(() => {
      layer.dispatchEvent(new Event("pointerdown", { bubbles: true, cancelable: true }));
    });
    expect(list()).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("reads <option> and <optgroup> children the way a native select does", () => {
    const extra: string | null = "custom";
    act(() =>
      root.render(
        <Select value="b" onChange={() => {}} aria-label="Pick">
          <option value="">First in the list</option>
          <optgroup label="Letters">
            {["a", "b"].map((v) => (
              <option key={v} value={v}>
                Letter {v}
              </option>
            ))}
          </optgroup>
          {extra && <option value={extra}>{extra} (not in the list)</option>}
          {false && <option value="never">never</option>}
        </Select>,
      ),
    );
    expect(shown(trigger())).toBe("Letter b");
    expect(optionsOf(trigger())).toEqual(["", "a", "b", "custom"]);
    act(() => trigger().click());
    expect(document.querySelector('[data-value="custom"]')?.textContent).toContain(
      "custom (not in the list)",
    );
    expect([...document.querySelectorAll(".select-group")].map((g) => g.textContent)).toEqual([
      "Letters",
    ]);
  });

  it("does not open while disabled", () => {
    act(() =>
      root.render(<Select value="a" options={[{ value: "a", label: "A" }]} onChange={() => {}} disabled />),
    );
    act(() => trigger().click());
    expect(list()).toBeNull();
  });

  it("says what it is set to when nothing listed matches", () => {
    act(() => root.render(<Select value="" options={[]} onChange={() => {}} placeholder="no models" />));
    expect(shown(trigger())).toBe("no models");
  });
});
