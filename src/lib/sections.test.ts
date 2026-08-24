// @vitest-environment jsdom
//
// The hook's whole job is a round trip through `localStorage`, so there is no
// version of this that a string render can see: the value is read once on
// mount and written on every toggle, and both halves need a real one.
import { act } from "react";
import { createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useSections, type Sections } from "./sections";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const KEY = "taurus.railSections";
let root: Root | null = null;

/** Mounts the hook and hands back the live value, the way a rail would see it. */
async function mount(): Promise<{ current: Sections }> {
  const handle: { current: Sections } = { current: null as never };
  const Probe = () => {
    handle.current = useSections();
    return null;
  };
  const host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root!.render(createElement(Probe)));
  return handle;
}

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = "";
});

describe("the rail's fold state", () => {
  beforeEach(() => localStorage.clear());

  it("starts with everything open", async () => {
    const rail = await mount();
    expect(rail.current.collapsed("tools")).toBe(false);
    expect(rail.current.collapsed("today")).toBe(false);
  });

  it("folds and unfolds a section by name", async () => {
    const rail = await mount();
    await act(async () => rail.current.toggle("tools"));
    expect(rail.current.collapsed("tools")).toBe(true);
    // And only that one. A fold that took its neighbours with it would be a
    // single "sections collapsed" flag wearing a name.
    expect(rail.current.collapsed("today")).toBe(false);

    await act(async () => rail.current.toggle("tools"));
    expect(rail.current.collapsed("tools")).toBe(false);
  });

  it("remembers the fold across a restart", async () => {
    const first = await mount();
    await act(async () => first.current.toggle("tools"));
    await act(async () => root?.unmount());
    root = null;

    // A cold start, reading what the last one wrote.
    const second = await mount();
    expect(second.current.collapsed("tools")).toBe(true);
  });

  it("opens a section it has never heard of", async () => {
    /*
     * The reason the stored value is the *collapsed* set rather than the open
     * one. A section added in a later version is absent from everybody's
     * stored state, and absent has to mean open — the alternative ships a
     * feature invisible to every existing user, in a way none of them can
     * report, because a section they have never seen is not one they can
     * notice is missing.
     */
    localStorage.setItem(KEY, JSON.stringify(["tools"]));
    const rail = await mount();
    expect(rail.current.collapsed("tools")).toBe(true);
    expect(rail.current.collapsed("a-section-from-a-later-version")).toBe(false);
  });

  it("opens everything when the stored value is not what this version writes", async () => {
    // Hand-edited, half-written, or left by a version that meant something
    // else by the key. All of them are answered by the state the app ships in,
    // rather than by throwing on the first render of the rail.
    for (const junk of ['{"tools":true}', "not json at all", "[1,2,3]", ""]) {
      localStorage.setItem(KEY, junk);
      const rail = await mount();
      expect(rail.current.collapsed("tools"), junk).toBe(false);
      await act(async () => root?.unmount());
      root = null;
    }
  });
});
