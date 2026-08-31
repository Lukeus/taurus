// @vitest-environment jsdom
//
// The most-pressed control in the app, and the only place in it where a chord
// is bound to a button. A first paint can show the keys printed on them; only a
// mounted dialog can show that pressing one decides anything — or, more to the
// point, that pressing the wrong thing decides nothing.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { PermissionDialog } from "./PermissionDialog";
import type { PermissionDecision, PermissionRequest } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

const WRITE: PermissionRequest = {
  id: "p1",
  effect: "write",
  preview: "Edit crates/taurus-core/src/agent.rs",
  offer_always: true,
  always_scope: "Allowed for this workspace from now on",
  always_global_scope: null,
  diff: null,
} as unknown as PermissionRequest;

function mount(request: PermissionRequest = WRITE) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const decided: PermissionDecision[] = [];
  const render = (next: PermissionRequest) =>
    root.render(
      <PermissionDialog
        request={next}
        onDecide={(decision) => decided.push(decision)}
      />,
    );
  act(() => render(request));
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return {
    host,
    decided,
    rerender: (next: PermissionRequest) => act(() => render(next)),
    press: (init: KeyboardEventInit) =>
      act(() => {
        window.dispatchEvent(
          new KeyboardEvent("keydown", { bubbles: true, ...init }),
        );
      }),
  };
}

describe("deciding a permission from the keyboard", () => {
  it("allows once on the platform's modifier and Enter", () => {
    const { decided, press } = mount();
    press({ key: "Enter", metaKey: true });
    press({ key: "Enter", ctrlKey: true });
    // One of the two is this platform's; the other must do nothing at all.
    expect(decided).toEqual(["allow_once"]);
  });

  it("denies on the modifier and Backspace", () => {
    const { decided, press } = mount();
    press({ key: "Backspace", metaKey: true });
    press({ key: "Backspace", ctrlKey: true });
    expect(decided).toEqual(["deny"]);
  });

  it("ignores a bare Enter and a bare Escape", () => {
    /*
     * The whole reason these are chords. This dialog can appear under somebody
     * mid-sentence, which is also why nothing here is autofocused: a stray
     * keystroke must never become a grant. Escape is refused for the reason
     * the component's own note gives — it would have to mean one of the
     * buttons, and none of them is what a person pressing Escape decided.
     */
    const { decided, press } = mount();
    press({ key: "Enter" });
    press({ key: "Escape" });
    press({ key: "Backspace" });
    expect(decided).toEqual([]);
  });

  it("binds no key to either standing grant", () => {
    // "Always" is a decision about every future call, and a decision about
    // every future call is worth a deliberate press.
    const { decided, press } = mount();
    press({ key: "Enter", metaKey: true, shiftKey: true });
    press({ key: "Enter", ctrlKey: true, shiftKey: true });
    press({ key: "a", metaKey: true });
    press({ key: "a", ctrlKey: true });
    expect(decided).toEqual([]);
  });

  it("answers the call on screen and not the one before it", () => {
    // The handler closes over which call is being decided. One left over from
    // the previous request would release the wrong tool call — and the tool
    // calls are a file write and a shell command.
    const { decided, rerender, press } = mount();
    rerender({ ...WRITE, id: "p2", preview: "Run: rm -rf target" });
    press({ key: "Enter", metaKey: true });
    press({ key: "Enter", ctrlKey: true });
    expect(decided).toEqual(["allow_once"]);
  });

  it("stops listening once it is gone", () => {
    // A parked listener would answer a permission that is no longer being
    // asked, with nothing on screen to say it had.
    const { decided, press } = mount();
    cleanup.pop()?.();
    press({ key: "Enter", metaKey: true });
    press({ key: "Enter", ctrlKey: true });
    expect(decided).toEqual([]);
  });

  it("prints the key on the button it fires", () => {
    // The only discovery surface a chord in a modal has, and the reason it
    // needs no row in the palette.
    const { host } = mount();
    const keys = [...host.querySelectorAll(".dialog-actions .key")];
    expect(keys).toHaveLength(2);
    expect(keys.map((k) => k.textContent)).toEqual([
      expect.stringContaining("↵"),
      expect.stringContaining("⌫"),
    ]);
  });
});
