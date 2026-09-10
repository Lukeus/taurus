// @vitest-environment jsdom
//
// jsdom lays nothing out, so the two numbers a browser would supply — how wide
// the box is and how tall its text wants to be — are supplied here, and the
// observer that reports a resize is a stand-in the test can fire.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

import { ProseEditor } from "./ProseEditor";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

it("measures itself again when its width changes, not only when its text does", async () => {
  // Measured on a change of text alone, a note narrowed by the canvas split
  // kept its old height: the last lines cut off, the paint drifting off them.
  let resized: () => void = () => {};
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        resized = callback;
      }
      observe() {}
      disconnect() {}
    },
  );
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => {
    createRoot(host).render(<ProseEditor text="A paragraph long enough to wrap." onChange={() => {}} />);
  });
  const box = host.querySelector("textarea")!;
  let width = 600;
  let wants = 40;
  Object.defineProperty(box, "clientWidth", { get: () => width });
  Object.defineProperty(box, "scrollHeight", { get: () => wants });

  resized();
  expect(box.style.height).toBe("40px");

  // Narrower, so the same text wraps onto more lines.
  width = 300;
  wants = 80;
  resized();
  expect(box.style.height).toBe("80px");

  // A resize that is only the height this set is not answered — answering it
  // would be a loop that only happens to settle.
  wants = 120;
  resized();
  expect(box.style.height).toBe("80px");
});
