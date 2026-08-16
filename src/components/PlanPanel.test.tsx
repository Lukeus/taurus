import { describe, expect, it } from "vitest";

import type { Step } from "../lib/api";
import { summarize } from "./PlanPanel";

const STEPS: Step[] = [
  { text: "Read the parser", state: "done" },
  { text: "Add the token type", state: "active" },
  { text: "Update the tests", state: "todo" },
];

const allDone = STEPS.map((s) => ({ ...s, state: "done" as const }));

describe("what the plan panel says about a plan", () => {
  it("counts what is done against what there is", () => {
    // The one number worth having on the collapsed row: it is the answer to
    // "how far through is this" without opening anything.
    expect(summarize(STEPS).done).toBe(1);
    expect(summarize(allDone).done).toBe(3);
  });

  it("names the live step", () => {
    // So the row answers "what is it doing" when it is read out of context —
    // a screenshot, a glance, a scroll past.
    expect(summarize(STEPS).live).toBe("Working on: Add the token type");
    expect(summarize(STEPS).foot).toBe("step 2 of 3 running");
  });

  it("leaves a step's text exactly as the model wrote it", () => {
    // The mockup lower-cases the step in the summary line. A step is as likely
    // to begin with an identifier as with a verb, and recasing one is a wrong
    // answer that reads like a right one.
    const one: Step[] = [{ text: "Rename providerKey", state: "active" }];
    expect(summarize(one).live).toBe("Working on: Rename providerKey");
  });

  it("uses the model's own running phrasing when it wrote one", () => {
    // The whole point of `active_form`: a status line the model authored,
    // rather than one derived by mangling the imperative.
    const one: Step[] = [
      {
        text: "Rename providerKey",
        state: "active",
        active_form: "Renaming providerKey",
      },
    ];
    expect(summarize(one).live).toBe("Renaming providerKey");
  });

  it("ignores an active form on a step that is not the live one", () => {
    const steps: Step[] = [
      {
        text: "Read the parser",
        state: "done",
        active_form: "Reading the parser",
      },
      { text: "Add the token type", state: "todo", active_form: "Adding it" },
    ];
    expect(summarize(steps).live).toBe("No step in progress.");
  });

  it("says so plainly when every step is finished", () => {
    // "3 / 3" is arithmetic the reader should not have to do.
    expect(summarize(allDone).live).toBe("All steps complete.");
    expect(summarize(allDone).foot).toBe("3 steps complete");
  });

  it("does not claim a step is running when none is", () => {
    // Legal: the model can mark a step done and not start the next one in the
    // same call. Saying "paused" there would name a feature that does not
    // exist; the panel just reports what it has.
    const steps = STEPS.map((s) => ({ ...s, state: "todo" as const }));
    expect(summarize(steps).live).toBe("No step in progress.");
    expect(summarize(steps).foot).toBe("0 of 3 complete");
  });

  it("handles a single-step plan without falling over", () => {
    // Legal, and what a model produces the moment before it adds the rest.
    const one: Step[] = [{ text: "Just the one", state: "todo" }];
    expect(summarize(one)).toEqual({
      done: 0,
      live: "No step in progress.",
      foot: "0 of 1 complete",
    });
  });
});
