// @vitest-environment jsdom
//
// The recent folders: what an empty box starts on, what typing narrows it to,
// and that a running turn keeps the rows from switching.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const RECENT = ["/code/taurus", "/code/site", "/notes/journal"];
let root: Root;
let onSwitch: ReturnType<typeof vi.fn>;
let onBrowse: ReturnType<typeof vi.fn>;

const mount = (blocked: string | null = null) =>
  act(async () =>
    root.render(
      <WorkspaceSwitcher
        current="/code/taurus"
        recent={RECENT}
        blocked={blocked}
        onSwitch={onSwitch}
        onBrowse={onBrowse}
        onForget={() => {}}
        onClose={() => {}}
      />,
    ),
  );
const box = () => document.querySelector<HTMLInputElement>('[aria-label="Switch workspace"]')!;
const chosen = () => document.querySelector('[role="option"][aria-selected="true"]')?.textContent;
const type = (text: string) =>
  act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(box(), text);
    box().dispatchEvent(new Event("input", { bubbles: true }));
  });
const enter = () =>
  act(async () => {
    box().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  });

beforeEach(() => {
  onSwitch = vi.fn();
  onBrowse = vi.fn();
  const host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  document.body.innerHTML = "";
});

it("narrows to what's typed, and keeps Open folder… last", async () => {
  await mount();
  await type("jour");
  const rows = [...document.querySelectorAll('[role="option"]')].map((o) => o.textContent);
  expect(rows).toHaveLength(2);
  expect(rows[0]).toContain("journal");
  expect(rows[1]).toContain("Open folder…");
  await enter();
  expect(onSwitch).toHaveBeenCalledWith("/notes/journal");
});

it("opens the dialog from its own row", async () => {
  await mount();
  await type("zzz");
  expect(chosen()).toContain("Open folder…");
  await enter();
  expect(onBrowse).toHaveBeenCalled();
});

it("won't switch while a turn runs, and says why", async () => {
  await mount("Stop the running turn before switching workspace");
  expect(document.body.textContent).toContain("Stop the running turn");
  expect(chosen()).toContain("site");
  await enter();
  expect(onSwitch).not.toHaveBeenCalled();
});
