// @vitest-environment jsdom
//
// Editing the pinned plan. What these pin down is what the editor sends and
// when it refuses to: the plan it started from goes with every save, finished
// steps can't be touched, and a plan the model rewrote mid-edit is caught
// before Save rather than after.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PlanEdited, Step } from "../lib/api";
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

type OnEdit = (seen: Step[], steps: Step[]) => Promise<PlanEdited | null>;

const mount = (view: PlanView, onEdit: OnEdit, yours: string[] = []) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const render = (next: PlanView) =>
    act(() => root.render(<PlanPanel view={next} yours={yours} onEdit={onEdit} />));
  render(view);
  const $ = <T extends Element>(selector: string) => host.querySelector<T>(selector);
  const button = (label: string) =>
    [...host.querySelectorAll<HTMLButtonElement>("button")].find(
      (b) => b.textContent === label || b.getAttribute("aria-label") === label,
    );
  return {
    host,
    $,
    button,
    rerender: render,
    click: (label: string) => act(() => button(label)!.click()),
    open: () => act(() => $<HTMLButtonElement>(".plan-summary")!.click()),
    type: (input: HTMLInputElement, value: string) =>
      act(() => {
        // React tracks the value itself; setting it through the prototype is
        // what makes the change event carry the new text.
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(
          input,
          value,
        );
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }),
    save: async () => {
      await act(async () => {
        button("Save")!.click();
      });
    },
  };
};

afterEach(() => {
  document.body.innerHTML = "";
});

describe("editing the pinned plan", () => {
  it("offers Edit only when there is somewhere to send the edit", () => {
    const panel = mount(plan(), vi.fn());
    panel.open();
    expect(panel.button("Edit")).toBeDefined();

    const host = document.createElement("div");
    document.body.appendChild(host);
    act(() => createRoot(host).render(<PlanPanel view={plan()} />));
    act(() => host.querySelector<HTMLButtonElement>(".plan-summary")!.click());
    expect(host.textContent).not.toContain("Edit");
  });

  it("sends the plan it started from along with the new list", async () => {
    const onEdit = vi.fn<OnEdit>().mockResolvedValue({
      outcome: "unchanged",
    });
    const panel = mount(plan(), onEdit);
    panel.open();
    panel.click("Edit");

    panel.type(panel.$<HTMLInputElement>('input[aria-label="New step"]')!, "Update the changelog");
    panel.click("Add");
    panel.type(panel.$<HTMLInputElement>('input[aria-label="Step 3"]')!, "Update the parser tests");
    await panel.save();

    expect(onEdit).toHaveBeenCalledWith(STEPS, [
      STEPS[0],
      STEPS[1],
      { text: "Update the parser tests", state: "todo" },
      { text: "Update the changelog", state: "todo" },
    ]);
    // Closed again: the list reads, it doesn't edit.
    expect(panel.$(".plan-editing")).toBeNull();
  });

  it("leaves finished steps as text, and won't move a step above them", () => {
    const panel = mount(plan(), vi.fn());
    panel.open();
    panel.click("Edit");

    expect(panel.$('input[aria-label="Step 1"]')).toBeNull();
    expect(panel.$(".plan-step.done")?.textContent).toContain("Read the parser");
    expect(panel.button("Move step 2 up")!.disabled).toBe(true);
    expect(panel.button("Move step 3 up")!.disabled).toBe(false);
  });

  it("can't save until something changed", () => {
    const panel = mount(plan(), vi.fn());
    panel.open();
    panel.click("Edit");
    expect(panel.button("Save")!.disabled).toBe(true);

    panel.click("Remove step 3");
    expect(panel.button("Save")!.disabled).toBe(false);
  });

  it("says so before Save when the model rewrote the plan mid-edit", () => {
    const onEdit = vi.fn<OnEdit>();
    const panel = mount(plan(), onEdit);
    panel.open();
    panel.click("Edit");
    panel.click("Remove step 3");

    const moved: Step[] = [
      STEPS[0],
      { text: "Add the token type", state: "done" },
      { text: "Update the tests", state: "active" },
    ];
    panel.rerender(plan(moved));
    expect(panel.host.textContent).toContain("changed the plan since you started");
    expect(panel.button("Save")!.disabled).toBe(true);

    panel.click("Start again from the current plan");
    expect(panel.$<HTMLInputElement>('input[aria-label="Step 3"]')!.value).toBe(
      "Update the tests",
    );
    expect(panel.host.textContent).not.toContain("changed the plan since you started");
  });

  it("keeps a refused edit open with the reason", async () => {
    const onEdit = vi.fn<OnEdit>().mockResolvedValue({
      outcome: "invalid",
      message: "'Update the tests' is on the plan twice.",
    });
    const panel = mount(plan(), onEdit);
    panel.open();
    panel.click("Edit");
    panel.type(panel.$<HTMLInputElement>('input[aria-label="Step 2"]')!, "Update the tests");
    await panel.save();

    expect(panel.$('[role="alert"]')?.textContent).toBe(
      "'Update the tests' is on the plan twice.",
    );
    expect(panel.$(".plan-editing")).not.toBeNull();
  });

  it("marks the steps the person wrote", () => {
    const panel = mount(plan(), vi.fn(), ["Update the tests"]);
    panel.open();
    const rows = panel.host.querySelectorAll(".plan-step");
    expect(rows[2].textContent).toContain("added by you");
    expect(rows[1].textContent).not.toContain("added by you");
  });
});
