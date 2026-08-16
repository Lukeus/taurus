// @vitest-environment jsdom
//
// The panel is the one card with state of its own — it opens — and the state is
// the whole point of pinning it: one line while you are reading the transcript,
// the full list when you want it. Rendering to a string only ever asks what the
// collapsed row looks like.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import type { Step } from "../lib/api";
import type { PlanView } from "../state/store";
import { PlanPanel } from "./PlanPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const STEPS: Step[] = [
  { text: "Read the parser", state: "done" },
  { text: "Add the token type", state: "active" },
  { text: "Update the tests", state: "todo" },
];

const plan = (steps: Step[] = STEPS): PlanView => ({ type: "plan", steps });

const mount = (view: PlanView) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => root.render(<PlanPanel view={view} />));
  return {
    host,
    toggle: () =>
      act(() => {
        host.querySelector<HTMLButtonElement>(".plan-summary")!.click();
      }),
    rerender: (next: PlanView) =>
      act(() => root.render(<PlanPanel view={next} />)),
    unmount: () => act(() => root.unmount()),
  };
};

afterEach(() => {
  document.body.innerHTML = "";
});

describe("the pinned plan panel", () => {
  it("starts as one line: progress, the live step, the count", () => {
    const { host, unmount } = mount(plan());

    expect(host.querySelector(".plan-live")?.textContent).toBe(
      "Working on: Add the token type",
    );
    expect(host.querySelector(".plan-count")?.textContent).toBe("1 / 3");
    expect(host.querySelector(".plan-bar-fill")?.getAttribute("style")).toContain(
      "33.33",
    );
    // The transcript is the window's purpose; the list stays out of it until
    // it is asked for.
    expect(host.querySelector(".plan-list")).toBeNull();
    unmount();
  });

  it("opens to the full list and closes again", () => {
    const { host, toggle, unmount } = mount(plan());
    const summary = () => host.querySelector(".plan-summary")!;

    expect(summary().getAttribute("aria-expanded")).toBe("false");
    toggle();
    expect(summary().getAttribute("aria-expanded")).toBe("true");
    expect(host.querySelectorAll(".plan-step")).toHaveLength(3);
    toggle();
    expect(summary().getAttribute("aria-expanded")).toBe("false");
    expect(host.querySelector(".plan-list")).toBeNull();
    unmount();
  });

  it("shows every step in the order the model sent them", () => {
    const { host, toggle, unmount } = mount(plan());
    toggle();

    const rows = [...host.querySelectorAll(".plan-text")].map(
      (n) => n.textContent,
    );
    expect(rows).toEqual(STEPS.map((s) => s.text));
    unmount();
  });

  it("states every step's state in words, not only in styling", () => {
    // The mark is a shape and the tint is a colour; both fail in a screenshot,
    // at a glance, and for a reader who cannot pick the contrast out. The word
    // on each row is the rendering that does not.
    const { host, toggle, unmount } = mount(plan());
    toggle();

    expect(
      [...host.querySelectorAll(".plan-status")].map((n) => n.textContent),
    ).toEqual(["done", "running", "queued"]);
    expect(host.querySelector(".plan-mark")?.getAttribute("aria-hidden")).toBe(
      "true",
    );
    unmount();
  });

  it("stays open when the model rewrites the plan", () => {
    // The model rewrites the whole list every time a step starts or finishes.
    // Someone who opened the panel was watching the steps; snapping it shut
    // under them on every update is the opposite of what they asked for.
    const { host, toggle, rerender, unmount } = mount(plan());
    toggle();

    rerender(
      plan([
        { text: "Read the parser", state: "done" },
        { text: "Add the token type", state: "done" },
        { text: "Update the tests", state: "active" },
      ]),
    );

    expect(host.querySelector(".plan-summary")?.getAttribute("aria-expanded")).toBe(
      "true",
    );
    expect(host.querySelector(".plan-live")?.textContent).toBe(
      "Working on: Update the tests",
    );
    expect(host.querySelector(".plan-count")?.textContent).toBe("2 / 3");
    unmount();
  });

  it("says so plainly when every step is finished", () => {
    const { host, unmount } = mount(
      plan(STEPS.map((s) => ({ ...s, state: "done" as const }))),
    );
    expect(host.querySelector(".plan-live")?.textContent).toBe(
      "All steps complete.",
    );
    expect(host.querySelector(".plan-bar-fill")?.getAttribute("style")).toContain(
      "100%",
    );
    unmount();
  });
});
