import { useState } from "react";

import type { Step } from "../lib/api";
import type { PlanView } from "../state/store";

/**
 * The checklist the model is working through, pinned above the composer.
 *
 * The only card here that is also read by its author. A table and a chart are
 * shown to the person and then forgotten; this one goes back into the system
 * prompt on every iteration, which is what stops a small model losing the
 * thread of a six-step task. So the two renderings have to say the same thing —
 * what you read here is what the model reads.
 *
 * It sits above the composer rather than in the transcript because of what it
 * is: not a thing that happened at a moment, but where the work is *now*. Drawn
 * in the flow, the checklist that exists to answer "where are we" scrolls away
 * behind the twenty tool calls it was written to organize. Pinned, it is on
 * screen at the moment anyone would ask.
 *
 * That position is also why it is one line by default. The transcript is the
 * window's whole purpose, and a seven-step list nailed permanently across the
 * bottom of it would take a third of the reading area to say what a bar and a
 * count say in 30px. Opened, it is the full list — and it stays open, because
 * someone who opened it was watching the steps and the model rewriting the plan
 * is not a reason to stop.
 *
 * Three things carry progress, deliberately overlapping. The bar and the count
 * answer "how far through is this" without reading the list. The summary line
 * names the live step, so the answer to "what is it doing" survives being read
 * out of context — a screenshot, a glance, a scroll past; it prefers the step's
 * own `active_form` when the model wrote one, which is the only way that line
 * reads as a status rather than as an order. And every row states its own
 * status in a word, which is the rendering that still works when the marks and
 * the tint do not: at a glance, in a screenshot, and for anyone who cannot pick
 * the contrast out. Striking the finished ones through is the fast read, not
 * the only one.
 */
export function PlanPanel({ view }: { view: PlanView }) {
  const [open, setOpen] = useState(false);
  const { steps } = view;
  const { done, live, foot } = summarize(steps);

  return (
    <section className={`plan-panel${open ? " open" : ""}`}>
      <button
        type="button"
        className="plan-summary"
        aria-expanded={open}
        onClick={() => setOpen((was) => !was)}
      >
        {/* The same two glyphs the run header discloses with, so one gesture is
            learned once. Decorative next to `aria-expanded`, which is the same
            fact said in the one place a screen reader looks for it. */}
        <span className="plan-chevron" aria-hidden="true">
          {open ? "⌄" : "›"}
        </span>
        <span className="plan-label">Plan</span>
        {/* Decorative: the count beside it is the same fact in a form that can
            be read out. */}
        <span className="plan-bar" aria-hidden="true">
          <span
            className="plan-bar-fill"
            style={{ width: `${(done / steps.length) * 100}%` }}
          />
        </span>
        <span className="plan-live">{live}</span>
        <span className="micro plan-count">
          {done} / {steps.length}
        </span>
      </button>

      {open && (
        <div className="plan-body">
          <ol className="plan-list">
            {steps.map((step, i) => (
              <li key={i} className={`plan-step ${step.state}`}>
                {/* The mark is a shape, and a shape cannot be read aloud. The
                    status beside it is the same state in words. */}
                <span className="plan-mark" aria-hidden="true">
                  {step.state === "done" ? "✓" : ""}
                </span>
                <span className="plan-text">{step.text}</span>
                <span className="plan-status">{STATUS[step.state]}</span>
              </li>
            ))}
          </ol>
          <p className="plan-foot">{foot}</p>
        </div>
      )}
    </section>
  );
}

/**
 * What the panel says about the plan, in words.
 *
 * Pure and exported because these are the rules worth pinning down: which step
 * is called live, what is said when none is, and how the model's own phrasing
 * wins over anything derived. Mounting the component proves none of that.
 */
export function summarize(steps: Step[]): {
  done: number;
  /** The status line: what is happening, in one phrase. */
  live: string;
  /** Where the list is up to, in the same words the run header uses. */
  foot: string;
} {
  const done = steps.filter((step) => step.state === "done").length;
  const complete = done === steps.length;
  // The model is told exactly one step may be active, and the tool refuses a
  // plan where more than one is. So the first is the only.
  const at = steps.findIndex((step) => step.state === "active");
  const active = at >= 0 ? steps[at] : undefined;

  return {
    done,
    // The mockup gets this line by lower-casing the step, which turns
    // `providerKey` into `providerkey`. The model writes the running phrasing
    // itself instead, and when it has not, the step is quoted verbatim behind a
    // prefix that makes the imperative read as a status. Never a transform of
    // the text — a recased identifier is a wrong answer that looks like a right
    // one.
    live: active
      ? (active.active_form ?? `Working on: ${active.text}`)
      : complete
        ? "All steps complete."
        : "No step in progress.",
    foot: active
      ? `step ${at + 1} of ${steps.length} running`
      : complete
        ? `${steps.length} steps complete`
        : `${done} of ${steps.length} complete`,
  };
}

/**
 * The word each state wears on its row.
 *
 * Deliberately the running commentary rather than the checklist vocabulary the
 * model reads back (`[ ]`, `[>]`, `[x]` — see `StepState::marker` in Rust).
 * They are the same three states; this panel is being read while the work
 * happens, and "running" is what a reader watching it wants to know.
 */
const STATUS: Record<Step["state"], string> = {
  todo: "queued",
  active: "running",
  done: "done",
};
