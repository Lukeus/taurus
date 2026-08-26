// @vitest-environment jsdom
//
// Mounted rather than rendered to a string, for the reason `AgentsDrawer` is:
// this component reads the store, and a string render does not see a zustand
// subscription — the bug that once kept a whole drawer from opening was
// invisible to every string test in the suite.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ContextMeter } from "./ContextMeter";
import { useStore } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const into = (onOpen: () => void = () => {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => root.render(<ContextMeter onOpen={onOpen} />));
  return host;
};

const mount = (onOpen: () => void = () => {}) => into(onOpen).innerHTML;

/** Puts a reading in the store the way a running turn does. */
const reading = (used: number, window: number | null) =>
  useStore.setState({ context: window === null ? null : { used, window } });

beforeEach(() => reading(0, null));
afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("ContextMeter", () => {
  it("says nothing before a turn has measured anything", () => {
    expect(mount()).toBe("");
  });

  it("stays quiet while there is plenty of room", () => {
    reading(20_000, 128_000);
    expect(mount()).toBe("");
  });

  it("names the window as well as the fraction", () => {
    // The window is the half that earns its place: an OpenAI-compatible
    // backend cannot be asked how much it holds, so a session filling
    // implausibly fast is a misconfiguration you can only recognize if the
    // number it is filling up is on screen.
    reading(80_000, 128_000);
    const html = mount();
    expect(html).toContain("63%");
    expect(html).toContain("128k");
    expect(html).not.toContain("summarized");
  });

  it("says when the harness has started summarizing on its own", () => {
    reading(104_000, 128_000);
    const html = mount();
    expect(html).toContain("summarized");
    expect(html).toContain("full");
  });

  it("does not divide by a window of zero", () => {
    reading(1_000, 0);
    expect(mount()).toBe("");
  });

  it("reads a million-token window in millions", () => {
    reading(900_000, 1_000_000);
    expect(mount()).toContain("1M");
  });

  it("never draws a bar past the end of the track", () => {
    // A measurement can exceed the window: the estimate is an estimate, and
    // the last one taken can be of a request the model then answered into.
    reading(200_000, 128_000);
    expect(mount()).toContain("width: 100%");
  });

  it("opens the account when it is pressed", () => {
    // The shape of the feature: this says how much, and the panel it opens
    // says what it went on. A reading that makes you ask a question should be
    // the thing you press to answer it.
    reading(90_000, 128_000);
    const opened = vi.fn();
    const host = into(opened);
    const meter = host.querySelector(".context-meter") as HTMLButtonElement;
    expect(meter.tagName).toBe("BUTTON");
    act(() => meter.click());
    expect(opened).toHaveBeenCalledOnce();
  });
});
