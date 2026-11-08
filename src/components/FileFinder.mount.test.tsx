// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { FileFinder } from "./FileFinder";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const PATHS = ["README.md", "src/App.tsx", "src/components/Canvas.tsx", "src/lib/mod.rs"];
let root: Root;
let onOpen: Mock<(path: string) => void>;
let onClose: Mock<() => void>;

const mount = async (open: string[] = []) => {
  await act(async () =>
    root.render(<FileFinder open={open} onOpen={onOpen} onClose={onClose} />),
  );
};
const input = () => document.querySelector<HTMLInputElement>(".palette-input")!;
const rows = () =>
  [...document.querySelectorAll<HTMLElement>('[role="option"]')].map((row) => row.id.slice(5));
const type = (text: string) =>
  act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input(), text);
    input().dispatchEvent(new Event("input", { bubbles: true }));
  });
const press = (key: string) =>
  act(async () => {
    input().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
  });

beforeEach(() => {
  onOpen = vi.fn();
  onClose = vi.fn();
  invoke.mockImplementation((command: string) =>
    command === "workspace_files"
      ? Promise.resolve({ paths: PATHS, cut: false })
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

describe("the file finder", () => {
  it("offers what's already open first when nothing is typed", async () => {
    await mount(["src/lib/mod.rs"]);
    expect(rows()[0]).toBe("src/lib/mod.rs");
    expect(rows()).toHaveLength(4);
    expect(document.querySelector(".palette-when")?.textContent).toBe("open");
  });

  it("narrows as you type and opens the best match on Enter", async () => {
    await mount();
    await type("canv");
    expect(rows()).toEqual(["src/components/Canvas.tsx"]);
    await press("Enter");
    expect(onOpen).toHaveBeenCalledWith("src/components/Canvas.tsx");
    expect(onClose).toHaveBeenCalled();
  });

  it("moves with the arrows and opens what's highlighted", async () => {
    await mount();
    await press("ArrowDown");
    await press("Enter");
    expect(onOpen).toHaveBeenCalledWith("src/App.tsx");
  });

  it("does nothing on Enter when nothing matches", async () => {
    await mount();
    await type("zzz");
    expect(document.querySelector(".palette-empty")?.textContent).toBe("No file matches that.");
    await press("Enter");
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("says when the list isn't every file", async () => {
    invoke.mockResolvedValue({ paths: PATHS, cut: true });
    await mount();
    expect(document.querySelector(".palette-searching")?.textContent).toContain("too large");
  });

  it("says why when the list can't be read", async () => {
    invoke.mockRejectedValue("Could not list the files in this workspace: gone");
    await mount();
    expect(document.querySelector(".palette-empty")?.textContent).toContain("Could not list");
  });
});
