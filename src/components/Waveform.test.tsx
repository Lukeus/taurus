// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Waveform, waveFor } from "./Waveform";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

/** jsdom has no `matchMedia`; the app's own guard treats that as "no
 *  preference", so a test that wants one has to say so. */
function reduceMotion(reduce: boolean) {
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => ({
      matches: reduce,
      addEventListener: () => {},
      removeEventListener: () => {},
    })),
  );
}

beforeEach(() => {
  vi.unstubAllGlobals();
});

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  vi.unstubAllGlobals();
});

function mount(node: React.ReactNode) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => root.render(node));
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return host;
}

describe("which shape belongs to which work", () => {
  /*
   * The mapping is the whole adaptation. The motion spec cycles its four
   * shapes on a timer because a mockup has no agent; the app has one, so the
   * shape is a fact about the turn. A reader who learns the four can tell
   * reading from writing without reading a word — which is the only thing this
   * does that a spinner does not.
   */
  it("gives reading the sweeping peak, which is what a scan looks like", () => {
    expect(waveFor("read")).toBe("peak");
  });

  it("gives writing the ripple, which reads as something being produced", () => {
    expect(waveFor("wrote")).toBe("ripple");
    expect(waveFor("kept")).toBe("ripple");
  });

  it("gives a command the scattered ticks, the one shape that is not periodic", () => {
    expect(waveFor("ran")).toBe("ticks");
    expect(waveFor("skill")).toBe("ticks");
  });

  it("gives thinking, and anything unclassified, the travelling wave", () => {
    expect(waveFor(null)).toBe("wave");
    expect(waveFor("net")).toBe("wave");
    expect(waveFor("other")).toBe("wave");
  });
});

describe("the waveform", () => {
  it("draws one bar per bar and nothing for a screen reader", () => {
    const host = mount(<Waveform mode="wave" bars={6} />);
    const row = host.querySelector(".waveform") as HTMLElement;
    expect(row.children).toHaveLength(6);
    // The text beside it already says the turn is working. This is a picture
    // of the same fact and announcing it twice is worse than not at all.
    expect(row.getAttribute("aria-hidden")).toBe("true");
  });

  /*
   * The one animation in the app that CSS cannot switch off, because CSS did
   * not start it. Honouring the preference here means not running the loop at
   * all — hiding a frame loop's output while still paying for it would be the
   * worst of both.
   */
  it("runs no frame loop at all when less motion was asked for", () => {
    reduceMotion(true);
    const frame = vi.spyOn(globalThis, "requestAnimationFrame");
    const host = mount(<Waveform mode="wave" bars={4} />);
    expect(frame).not.toHaveBeenCalled();
    // Still, but not flat. A row of even ticks says something is here without
    // claiming to be measuring it.
    const first = host.querySelector(".waveform span") as HTMLElement;
    expect(first.style.transform).toBe("scaleY(0.5)");
  });

  it("drives the bars from a frame loop when it may", () => {
    reduceMotion(false);
    const frame = vi.spyOn(globalThis, "requestAnimationFrame");
    mount(<Waveform mode="wave" bars={4} />);
    expect(frame).toHaveBeenCalled();
  });

  it("stops the loop when the turn ends and the strip goes away", () => {
    reduceMotion(false);
    const cancel = vi.spyOn(globalThis, "cancelAnimationFrame");
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    act(() => root.render(<Waveform mode="wave" />));
    act(() => root.unmount());
    host.remove();
    expect(cancel).toHaveBeenCalled();
  });
});
