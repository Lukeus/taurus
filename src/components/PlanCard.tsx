import type { TranscriptView } from "../lib/api";

type PlanView = Extract<TranscriptView, { type: "plan" }>;

/**
 * The checklist the model is working through.
 *
 * The only card here that is also read by its author. A table and a chart are
 * shown to the person and then forgotten; this one goes back into the system
 * prompt on every iteration, which is what stops a small model losing the
 * thread of a six-step task. So the two renderings have to say the same thing —
 * what you read here is what the model reads.
 *
 * Only the newest plan draws. The model rewrites the whole list every time a
 * step starts or finishes, so the superseded calls keep their row in the run
 * header and stop drawing; see `supersedePlans` in the store.
 *
 * Three things carry progress, deliberately overlapping. The bar and the count
 * answer "how far through is this" without reading the list. The summary line
 * names the live step, so the answer to "what is it doing" survives being read
 * out of context — a screenshot, a glance, a scroll past; it prefers the step's
 * own `active_form` when the model wrote one, which is the only way that line
 * reads as a status rather than as an order. And every row states
 * its own status in a word, which is the rendering that still works when the
 * marks and the tint do not: at a glance, in a screenshot, and for anyone who
 * cannot pick the contrast out. Striking the finished ones through is the fast
 * read, not the only one.
 */
export function PlanCard({ view }: { view: PlanView }) {
  const { steps } = view;
  const done = steps.filter((step) => step.state === "done").length;
  const complete = done === steps.length;
  // The model is told exactly one step may be active, and the tool refuses a
  // plan where more than one is. So the first is the only.
  const activeIndex = steps.findIndex((step) => step.state === "active");
  const active = activeIndex >= 0 ? steps[activeIndex] : undefined;

  return (
    <div className="view-card plan-card">
      <div className="view-head">
        <div>
          <h3>Plan</h3>
          {/* The mockup gets this line by lower-casing the step, which turns
              `providerKey` into `providerkey`. The model writes the running
              phrasing itself instead, and when it has not, the step is quoted
              verbatim behind a prefix that makes the imperative read as a
              status. Never a transform of the text — a recased identifier is a
              wrong answer that looks like a right one. */}
          <p className="view-caption">
            {active
              ? (active.active_form ?? `Working on: ${active.text}`)
              : complete
                ? "All steps complete."
                : "No step in progress."}
          </p>
        </div>
        <div className="spacer" />
        <span className="micro">
          {done} / {steps.length}
        </span>
      </div>

      {/* Decorative: the count beside it is the same fact in a form that can be
          read out. */}
      <div className="plan-bar" aria-hidden="true">
        <div
          className="plan-bar-fill"
          style={{ width: `${(done / steps.length) * 100}%` }}
        />
      </div>

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

      <p className="plan-foot">
        {active
          ? `step ${activeIndex + 1} of ${steps.length} running`
          : complete
            ? `${steps.length} steps complete`
            : `${done} of ${steps.length} complete`}
      </p>
    </div>
  );
}

/**
 * The word each state wears on its row.
 *
 * Deliberately the running commentary rather than the checklist vocabulary the
 * model reads back (`[ ]`, `[>]`, `[x]` — see `StepState::marker` in Rust).
 * They are the same three states; this card is being read while the work
 * happens, and "running" is what a reader watching it wants to know.
 */
const STATUS: Record<PlanView["steps"][number]["state"], string> = {
  todo: "queued",
  active: "running",
  done: "done",
};
