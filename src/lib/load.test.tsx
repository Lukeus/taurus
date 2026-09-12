// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { useLoad } from "./load";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

afterEach(() => {
  document.body.innerHTML = "";
});

/** A read the test answers by hand, in whatever order it likes. */
function pending<T>() {
  let answer!: (value: T) => void;
  let refuse!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    answer = yes;
    refuse = no;
  });
  return { promise, answer, refuse };
}

function mount<T>(load: () => Promise<T>) {
  let hook!: ReturnType<typeof useLoad<T>>;
  function Probe() {
    hook = useLoad(load, []);
    return null;
  }
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => root.render(<Probe />));
  return { hook: () => hook, unmount: () => act(() => root.unmount()) };
}

describe("a panel's first read", () => {
  it("says why it failed, rather than showing an empty list", async () => {
    const read = pending<string[]>();
    const { hook } = mount(() => read.promise);
    await act(async () => read.refuse("skills could not be listed"));
    expect(hook().data).toBeNull();
    expect(hook().error).toBe("skills could not be listed");
  });

  it("keeps the newest answer when an older one arrives after it", async () => {
    const reads = [pending<string>(), pending<string>()];
    let n = 0;
    const { hook } = mount(() => reads[n++].promise);
    act(() => void hook().reload());
    await act(async () => reads[1].answer("new"));
    await act(async () => reads[0].answer("old"));
    expect(hook().data).toBe("new");
  });

  it("clears a failure once a read succeeds", async () => {
    const reads = [pending<string>(), pending<string>()];
    let n = 0;
    const { hook } = mount(() => reads[n++].promise);
    await act(async () => reads[0].refuse("offline"));
    expect(hook().error).toBe("offline");
    act(() => void hook().reload());
    await act(async () => reads[1].answer("back"));
    expect(hook().error).toBeNull();
    expect(hook().data).toBe("back");
  });

  it("takes what an action answered with, and drops a read still in flight", async () => {
    const read = pending<string[]>();
    const { hook } = mount(() => read.promise);
    act(() => hook().set(["kept"]));
    await act(async () => read.answer(["stale"]));
    expect(hook().data).toEqual(["kept"]);
  });
});
