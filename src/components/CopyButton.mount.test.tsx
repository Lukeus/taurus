// @vitest-environment jsdom
//
// The acknowledgement is the point of this button, both ways.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CopyButton } from "./CopyButton";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

afterEach(() => {
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});

async function press(writeText: () => Promise<void>) {
  vi.stubGlobal("navigator", { clipboard: { writeText } });
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => root.render(<CopyButton text="hello" />));
  const button = host.querySelector("button")!;
  await act(async () => button.click());
  return button;
}

describe("the copy button", () => {
  it("says it copied", async () => {
    const button = await press(() => Promise.resolve());
    expect(button.textContent).toBe("copied");
  });

  it("says when the clipboard refused, rather than nothing", async () => {
    const button = await press(() => Promise.reject(new Error("denied")));
    expect(button.textContent).toBe("couldn't copy");
  });
});
