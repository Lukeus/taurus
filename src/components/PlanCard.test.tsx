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

const allDone = STEPS.map((s) => ({ ...s, state: "done" as const }));

describe("the plan card", () => {
  it("shows every step in the order the model sent them", () => {
    // Measured over the list alone: the live step is also named in the summary
    // above it, which is earlier in the markup than any row.
    const html = renderToStaticMarkup(<PlanCard view={plan(STEPS)} />);
    const list = html.slice(html.indexOf("<ol"));
    const order = STEPS.map((s) => list.indexOf(s.text));
    expect(order.every((i) => i >= 0)).toBe(true);
    expect(order).toEqual([...order].sort((a, b) => a - b));
  });

  it("counts what is done against what there is", () => {
    // The one number worth having in the header: it is the answer to "how far
    // through is this" without reading the list.
    const html = renderToStaticMarkup(<PlanCard view={plan(STEPS)} />);
    expect(html).toContain("1 / 3");
  });

  it("names the live step above the list", () => {
    // So the card answers "what is it doing" when it is read out of context —
    // a screenshot, a glance, a scroll past.
    const html = renderToStaticMarkup(<PlanCard view={plan(STEPS)} />);
    expect(html).toContain("Working on: Add the token type");
    expect(html).toContain("step 2 of 3 running");
  });

  it("leaves a step's text exactly as the model wrote it", () => {
    // The mockup lower-cases the step in the summary line. A step is as likely
    // to begin with an identifier as with a verb, and recasing one is a
    // wrong answer that reads like a right one.
    const html = renderToStaticMarkup(
      <PlanCard view={plan([{ text: "Rename providerKey", state: "active" }])} />,
    );
    expect(html).toContain("Working on: Rename providerKey");
  });

  it("says the live step in the model's own running phrasing when it wrote one", () => {
    // The whole point of `active_form`: a status line the model authored,
    // rather than one this card derived by mangling the imperative.
    const html = renderToStaticMarkup(
      <PlanCard
        view={plan([
          {
            text: "Rename providerKey",
            state: "active",
            active_form: "Renaming providerKey",
          },
        ])}
      />,
    );
    expect(html).toContain("Renaming providerKey");
    expect(html).not.toContain("Working on:");
    // The list itself stays imperative, so rows do not rewrite themselves as
    // the plan advances.
    expect(html.slice(html.indexOf("<ol"))).toContain("Rename providerKey");
  });

  it("ignores an active form on a step that is not the live one", () => {
    const html = renderToStaticMarkup(
      <PlanCard
        view={plan([
          { text: "Read the parser", state: "done", active_form: "Reading the parser" },
          { text: "Add the token type", state: "todo", active_form: "Adding it" },
        ])}
      />,
    );
    expect(html).toContain("No step in progress.");
    expect(html).not.toContain("Reading the parser");
    expect(html).not.toContain("Adding it");
  });

  it("says so plainly when every step is finished", () => {
    // "3 / 3" is arithmetic the reader should not have to do.
    const html = renderToStaticMarkup(<PlanCard view={plan(allDone)} />);
    expect(html).toContain("All steps complete.");
    expect(html).toContain("3 steps complete");
  });

  it("fills the bar by the share of steps that are done", () => {
    expect(renderToStaticMarkup(<PlanCard view={plan(STEPS)} />)).toContain(
      "width:33.33",
    );
    expect(renderToStaticMarkup(<PlanCard view={plan(allDone)} />)).toContain(
      "width:100%",
    );
  });

  it("does not claim a step is running when none is", () => {
    // Legal: the model can mark a step done and not start the next one in the
    // same call. Saying "paused" there would name a feature that does not
    // exist; the card just reports what it has.
    const html = renderToStaticMarkup(
      <PlanCard view={plan(STEPS.map((s) => ({ ...s, state: "todo" as const })))} />,
    );
    expect(html).toContain("No step in progress.");
    expect(html).not.toContain("running");
  });

  it("states every step's state in words, not only in styling", () => {
    // The mark is a shape and the tint is a colour; both fail in a screenshot,
    // at a glance, and for a reader who cannot pick the contrast out. The word
    // on each row is the rendering that does not.
    const html = renderToStaticMarkup(<PlanCard view={plan(STEPS)} />);
    expect(html).toContain("done");
    expect(html).toContain("running");
    expect(html).toContain("queued");
    expect(html).toContain('aria-hidden="true"');
  });

  it("renders a single-step plan without falling over", () => {
    // Legal, and what a model produces the moment before it adds the rest.
    const html = renderToStaticMarkup(
      <PlanCard view={plan([{ text: "Just the one", state: "todo" }])} />,
    );
    expect(html).toContain("Just the one");
    expect(html).toContain("0 / 1");
  });
});
