// @vitest-environment jsdom
//
// What survives the moment an answer finishes streaming.
//
// The static tests in `Markdown.test.tsx` see one render. These see two — the
// last frame of a stream and the finished answer — because the thing at stake
// is whether the second one keeps what the first one drew.
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

/** Every text the parser was handed, in order. */
const parsed: string[] = [];
vi.mock("react-markdown", async (original) => {
  const actual = await original<typeof import("react-markdown")>();
  const Real = actual.default;
  return {
    ...actual,
    default: (props: Parameters<typeof Real>[0]) => {
      parsed.push(String(props.children ?? ""));
      return createElement(Real, props);
    },
  };
});

/** How many times a flowchart was laid out. */
let planned = 0;
vi.mock("../lib/flow", async (original) => {
  const actual = await original<typeof import("../lib/flow")>();
  return {
    ...actual,
    plan: (...args: Parameters<typeof actual.plan>) => {
      planned += 1;
      return actual.plan(...args);
    },
  };
});

const { Markdown } = await import("./Markdown");

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  planned = 0;
});

function mount(text: string, streaming: boolean) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const render = (next: boolean) =>
    act(() => root.render(<Markdown text={text} streaming={next} />));
  render(streaming);
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return { host, render };
}

describe("the end of a stream", () => {
  it("keeps the code blocks it drew rather than drawing them again", () => {
    const view = mount("Here it is.\n\n```rust\nfn main() {}\n```\n", true);
    const block = view.host.querySelector(".md-code");
    expect(block).not.toBeNull();

    view.render(false);

    // The same element, not an equal one. A remount re-highlights the block
    // and throws away anything it was holding.
    expect(view.host.querySelector(".md-code")).toBe(block);
  });
});

describe("a long answer, one frame at a time", () => {
  it("parses only the paragraph being written, not the ones above it", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    cleanup.push(() => {
      act(() => root.unmount());
      host.remove();
    });
    const base = "First paragraph.\n\nSecond, with `code`.\n\nThird ";
    // Finished rather than streaming, so every render is a parse instead of a
    // throttled one — which is what each tick of the throttle pays.
    const draw = (text: string) =>
      act(() => root.render(<Markdown text={text} streaming={false} />));

    draw(base);
    parsed.length = 0;
    draw(`${base}word `);
    draw(`${base}word word `);

    expect(parsed).toEqual(["Third word ", "Third word word "]);
    expect(host.textContent).toContain("First paragraph.");
  });
});

describe("a finished diagram", () => {
  it("is laid out once, however often its source is shown and hidden", () => {
    const view = mount("```mermaid\nflowchart LR\n  a[Client] --> b[API]\n```\n", false);
    expect(view.host.querySelector(".flow")).not.toBeNull();
    const once = planned;
    const toggle = () =>
      [...view.host.querySelectorAll("button")].find((b) =>
        ["source", "diagram"].includes(b.textContent ?? ""),
      )!;

    act(() => toggle().click());
    act(() => toggle().click());

    expect(view.host.querySelector(".flow")).not.toBeNull();
    expect(planned).toBe(once);
  });
});
