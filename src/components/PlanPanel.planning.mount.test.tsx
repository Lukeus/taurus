// @vitest-environment jsdom
//
// A plan written while planning is a proposal waiting on the person: it opens
// on its own, takes a comment on any step not yet done, and has Approve.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Step } from "../lib/api";
import { PlanPanel } from "./PlanPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const STEPS: Step[] = [
  { text: "Read the parser", state: "todo" },
  { text: "Split the parser in two", state: "todo", check: "cargo build" },
];

let unmount: (() => void) | null = null;
afterEach(() => {
  unmount?.();
  unmount = null;
  document.body.innerHTML = "";
});

const mount = (props: Partial<Parameters<typeof PlanPanel>[0]> = {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const render = (more: Partial<Parameters<typeof PlanPanel>[0]> = {}) =>
    act(() => root.render(<PlanPanel view={{ type: "plan", steps: STEPS }} {...props} {...more} />));
  render();
  unmount = () => act(() => root.unmount());
  return { host, render };
};

const button = (host: HTMLElement, label: string) =>
  [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (b) => b.textContent === label || b.getAttribute("aria-label") === label,
  );

describe("a plan waiting for approval", () => {
  it("opens on its own and says what it is waiting for", () => {
    const { host } = mount({ planning: true, onApprove: vi.fn() });
    expect(host.querySelector(".plan-summary")?.getAttribute("aria-expanded")).toBe("true");
    expect(host.querySelector(".plan-live")?.textContent).toBe("Waiting for your approval");
    expect(host.querySelector(".plan-foot")?.textContent).toContain("Nothing changes until you approve");
  });

  it("is approved by Approve, and not while it is still being written", () => {
    const onApprove = vi.fn();
    const { host, render } = mount({ planning: true, onApprove, busy: true });
    expect(button(host, "Approve")?.disabled).toBe(true);
    expect(host.querySelector(".plan-live")?.textContent).toBe("Planning…");

    render({ busy: false });
    act(() => button(host, "Approve")!.click());
    expect(onApprove).toHaveBeenCalledOnce();
  });

  it("hands over a comment with the step it is about", () => {
    const onComment = vi.fn();
    const { host } = mount({ planning: true, onApprove: vi.fn(), onComment });

    act(() => button(host, "Comment on step 2")!.click());
    const box = host.querySelector<HTMLTextAreaElement>(".plan-comment-box textarea")!;
    act(() => {
      const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
      set.call(box, "keep the old module as a re-export");
      box.dispatchEvent(new Event("input", { bubbles: true }));
    });
    act(() => {
      box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    });

    expect(onComment).toHaveBeenCalledWith(STEPS[1], "keep the old module as a re-export");
    expect(host.querySelector(".plan-comment-box")).toBeNull();
  });

  it("marks a step that has a comment waiting", () => {
    const { host } = mount({
      planning: true,
      onApprove: vi.fn(),
      onComment: vi.fn(),
      commented: (step) => step === STEPS[0],
    });
    const marked = host.querySelectorAll(".plan-step.commented");
    expect(marked).toHaveLength(1);
    expect(marked[0].textContent).toContain("commented");
  });

  it("is an ordinary plan when nothing is planning", () => {
    const { host } = mount();
    expect(host.querySelector(".plan-summary")?.getAttribute("aria-expanded")).toBe("false");
    act(() => host.querySelector<HTMLButtonElement>(".plan-summary")!.click());
    expect(button(host, "Approve")).toBeUndefined();
    expect(button(host, "Comment on step 1")).toBeUndefined();
    expect(host.querySelector(".plan-status")?.textContent).toBe("queued");
  });
});
