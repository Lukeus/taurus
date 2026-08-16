import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { Step, TranscriptView } from "../lib/api";
import { PlanCard } from "./PlanCard";

const plan = (steps: Step[]): Extract<TranscriptView, { type: "plan" }> => ({
  type: "plan",
  steps,
});

const STEPS: Step[] = [
  { text: "Read the parser", state: "done" },
  { text: "Add the token type", state: "active" },
  { text: "Update the tests", state: "todo" },
];

describe("the plan card", () => {
  it("shows every step in the order the model sent them", () => {
    const html = renderToStaticMarkup(<PlanCard view={plan(STEPS)} />);
    const order = STEPS.map((s) => html.indexOf(s.text));
    expect(order.every((i) => i >= 0)).toBe(true);
    expect(order).toEqual([...order].sort((a, b) => a - b));
  });

  it("counts what is done against what there is", () => {
    // The one number worth having in the header: it is the answer to "how far
    // through is this" without reading the list.
    const html = renderToStaticMarkup(<PlanCard view={plan(STEPS)} />);
    expect(html).toContain("1 of 3 done");
  });

  it("says so plainly when every step is finished", () => {
    // "3 of 3 done" is arithmetic the reader should not have to do.
    const html = renderToStaticMarkup(
      <PlanCard view={plan(STEPS.map((s) => ({ ...s, state: "done" as const })))} />,
    );
    expect(html).toContain("all done");
  });

  it("names the live step's state in words, not only in styling", () => {
    // The single fact this card exists to convey. A tint and a bold weight are
    // the fast read and the one that fails in a screenshot, at a glance, and
    // for a reader who cannot pick the contrast out.
    const html = renderToStaticMarkup(<PlanCard view={plan(STEPS)} />);
    expect(html).toContain("in progress");
  });

  it("leaves every step's state readable without the marker", () => {
    // The markers are aria-hidden, so each row has to carry its state as text
    // somewhere — visible for the live one, screen-reader-only for the rest.
    const html = renderToStaticMarkup(<PlanCard view={plan(STEPS)} />);
    expect(html).toContain("done");
    expect(html).toContain("to do");
    expect(html).toContain('aria-hidden="true"');
  });

  it("renders a single-step plan without falling over", () => {
    // Legal, and what a model produces the moment before it adds the rest.
    const html = renderToStaticMarkup(
      <PlanCard view={plan([{ text: "Just the one", state: "todo" }])} />,
    );
    expect(html).toContain("Just the one");
    expect(html).toContain("0 of 1 done");
  });
});
