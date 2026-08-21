// @vitest-environment jsdom
//
// Keyboard behaviour, which is the whole of what this component is for and
// none of which a rendered snapshot can show.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { Modal } from "./Modal";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

function mount(onClose?: () => void) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => {
    root.render(
      <Modal onClose={onClose}>
        <aside className="drawer" onClick={(e) => e.stopPropagation()}>
          <button>first</button>
          <button>second</button>
          <button disabled>unreachable</button>
          <button>last</button>
        </aside>
      </Modal>,
    );
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return {
    host,
    panel: host.querySelector("aside")!,
    buttons: [...host.querySelectorAll("button")],
  };
}

const press = (key: string, shiftKey = false) =>
  act(() => {
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key, shiftKey, bubbles: true }),
    );
  });

describe("a modal", () => {
  it("takes focus on open, and the panel rather than a control", () => {
    /*
     * Landing on the first button means a stray Enter has already pressed it,
     * which on the permission prompt is a grant nobody chose. Landing nowhere
     * — which is what happened before — leaves focus on the row behind the
     * scrim, so the first Tab walks the background the user just covered up.
     */
    const { panel, buttons } = mount(() => {});
    expect(document.activeElement).toBe(panel);
    expect(document.activeElement).not.toBe(buttons[0]);
  });

  it("gives focus back to whatever opened it", () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    expect(document.activeElement).toBe(opener);

    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    act(() => {
      root.render(
        <Modal onClose={() => {}}>
          <aside>
            <button>inside</button>
          </aside>
        </Modal>,
      );
    });
    act(() => root.unmount());

    expect(document.activeElement).toBe(opener);
    host.remove();
    opener.remove();
  });

  it("closes on Escape", () => {
    let closed = 0;
    mount(() => closed++);
    press("Escape");
    expect(closed).toBe(1);
  });

  it("has no Escape for a panel that has to be answered", () => {
    // The permission prompt. Escape is neither "allow" nor "deny", so it is
    // not a decision this can make on the user's behalf.
    const { panel } = mount(undefined);
    press("Escape");
    expect(panel.isConnected).toBe(true);
  });

  it("lets the panel in front answer Escape, not the one it opened from", () => {
    /*
     * The editors open on top of the drawer that lists what they edit. Capture
     * listeners on `window` fire in registration order — outermost first — so
     * without a stack the drawer answers and the editor with unsaved changes
     * in it is the thing left on screen.
     */
    let closedOuter = 0;
    let closedInner = 0;

    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    act(() => {
      root.render(
        <Modal onClose={() => closedOuter++}>
          <aside>
            <button>drawer</button>
            <Modal onClose={() => closedInner++} className="scrim modal-scrim">
              <div>
                <button>editor</button>
              </div>
            </Modal>
          </aside>
        </Modal>,
      );
    });
    cleanup.push(() => {
      act(() => root.unmount());
      host.remove();
    });

    press("Escape");
    expect(closedInner).toBe(1);
    expect(closedOuter, "the drawer underneath answered for the editor").toBe(0);
  });

  it("wraps Tab inside the panel rather than letting it reach the page", () => {
    const { buttons } = mount(() => {});
    const [first, second, , last] = buttons;

    // From the panel itself, forward, to the first stop.
    press("Tab");
    expect(document.activeElement).toBe(first);

    second.focus();
    last.focus();
    press("Tab");
    expect(document.activeElement, "Tab off the end escaped the panel").toBe(first);

    first.focus();
    press("Tab", true);
    expect(document.activeElement, "Shift+Tab off the front escaped").toBe(last);
  });
});
