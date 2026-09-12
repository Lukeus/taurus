// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ARMED_FOR_MS, useArmed } from "./armed";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

function mount() {
  let hook!: ReturnType<typeof useArmed<string>>;
  function Probe() {
    hook = useArmed<string>();
    return null;
  }
  const host = document.createElement("div");
  document.body.appendChild(host);
  act(() => createRoot(host).render(<Probe />));
  return () => hook;
}

describe("a delete that takes two presses", () => {
  it("lets go on its own when nobody presses again", () => {
    // Left armed, a stray click a minute later answered a question nobody
    // remembered asking.
    vi.useFakeTimers();
    const hook = mount();
    act(() => hook().arm("s1"));
    expect(hook().armed).toBe("s1");
    act(() => vi.advanceTimersByTime(ARMED_FOR_MS - 1));
    expect(hook().armed).toBe("s1");
    act(() => vi.advanceTimersByTime(1));
    expect(hook().armed).toBeNull();
  });

  it("lets go on Escape", () => {
    const hook = mount();
    act(() => hook().arm("s1"));
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(hook().armed).toBeNull();
  });

  it("holds one question at a time: arming another row disarms the first", () => {
    const hook = mount();
    act(() => hook().arm("s1"));
    act(() => hook().arm("s2"));
    expect(hook().armed).toBe("s2");
  });
});
