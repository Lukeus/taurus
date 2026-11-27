// @vitest-environment jsdom
//
// A part that throws while drawing takes itself down, and nothing else.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Boundary } from "./Boundary";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  // React and the boundary both report a caught error to the console; the
  // test is about what is on screen.
  vi.spyOn(console, "error").mockImplementation(() => {});
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

let broken = true;
function Fragile({ text }: { text: string }) {
  if (broken) throw new Error(`cannot draw ${text}`);
  return <p className="drawn">{text}</p>;
}

describe("a boundary", () => {
  it("keeps its neighbours on screen when what it holds throws", () => {
    broken = true;
    act(() =>
      root.render(
        <>
          <p className="neighbour">still here</p>
          <Boundary label="This step">
            <Fragile text="a diagram" />
          </Boundary>
        </>,
      ),
    );
    expect(host.querySelector(".neighbour")?.textContent).toBe("still here");
    const alert = host.querySelector('[role="alert"]')!;
    expect(alert.textContent).toContain("This step could not be shown.");
    expect(alert.textContent).toContain("cannot draw a diagram");
  });

  it("draws again on Try again, once the cause is gone", () => {
    broken = true;
    act(() =>
      root.render(
        <Boundary label="This step">
          <Fragile text="a table" />
        </Boundary>,
      ),
    );
    broken = false;
    const retry = [...host.querySelectorAll("button")].find((b) => b.textContent === "Try again")!;
    act(() => retry.click());
    expect(host.querySelector(".drawn")?.textContent).toBe("a table");
  });

  it("clears itself when its reset key changes", () => {
    broken = true;
    const draw = (key: string) =>
      act(() =>
        root.render(
          <Boundary label="The open file" resetKey={key}>
            <Fragile text={key} />
          </Boundary>,
        ),
      );
    draw("a.md");
    expect(host.querySelector('[role="alert"]')).not.toBeNull();
    broken = false;
    draw("b.md");
    expect(host.querySelector('[role="alert"]')).toBeNull();
    expect(host.querySelector(".drawn")?.textContent).toBe("b.md");
  });

  it("fills the window and offers a reload at the root", () => {
    broken = true;
    act(() =>
      root.render(
        <Boundary label="The window" whole>
          <Fragile text="the app" />
        </Boundary>,
      ),
    );
    expect(host.querySelector(".boundary.whole")).not.toBeNull();
    const labels = [...host.querySelectorAll("button")].map((b) => b.textContent);
    expect(labels).toEqual(["Reload the window", "Copy details"]);
  });
});
