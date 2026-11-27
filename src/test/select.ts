import { act } from "react";

/**
 * Picks `value` from a `Select`, the way a person does: open it, press the row.
 *
 * Tests used to set `.value` on a native `<select>` and dispatch `change`. The
 * dropdown is a button and a list now, so this is the one place that knows
 * that, and a test reads as what it is doing — choosing — rather than how.
 */
export function choose(trigger: Element | null, value: string) {
  if (!trigger) throw new Error("choose: no dropdown to choose from");
  if (trigger.getAttribute("aria-expanded") !== "true") {
    act(() => (trigger as HTMLElement).click());
  }
  const option = document.querySelector<HTMLElement>(
    `[role="option"][data-value="${CSS.escape(value)}"]`,
  );
  if (!option) {
    throw new Error(`choose: no option ${JSON.stringify(value)} in ${JSON.stringify(optionsOf(trigger))}`);
  }
  act(() => option.click());
}

/** What a `Select` offers, by value — opening it to look if it is shut. */
export function optionsOf(trigger: Element | null): string[] {
  if (!trigger) throw new Error("optionsOf: no dropdown");
  const wasOpen = trigger.getAttribute("aria-expanded") === "true";
  if (!wasOpen) act(() => (trigger as HTMLElement).click());
  const values = [...document.querySelectorAll<HTMLElement>('[role="option"]')].map(
    (o) => o.dataset.value!,
  );
  if (!wasOpen) act(() => (trigger as HTMLElement).click());
  return values;
}

/** The value a `Select` shows as chosen, by its label text. */
export function shown(trigger: Element | null): string {
  return trigger?.querySelector(".select-current")?.textContent ?? "";
}
