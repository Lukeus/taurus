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
 * State is carried by a word as well as a mark. Striking through the finished
 * ones is the fast read, and it is the one that fails in a screenshot, at a
 * glance, and for anyone who cannot pick the contrast out — which is the same
 * argument the diff gutter makes, and it applies harder here, because the
 * single fact this card exists to convey is which step is live.
 */
export function PlanCard({ view }: { view: PlanView }) {
  const { steps } = view;
  const done = steps.filter((step) => step.state === "done").length;
  const complete = done === steps.length;

  return (
    <div className="view-card">
      <div className="view-head">
        <div>
          <h3>Plan</h3>
        </div>
        <div className="spacer" />
        <span className="micro">
          {complete ? "all done" : `${done} of ${steps.length} done`}
        </span>
      </div>

      <ol className="plan-list">
        {steps.map((step, i) => (
          <li key={i} className={`plan-step ${step.state}`}>
            <span className="plan-mark" aria-hidden="true">
              {MARK[step.state]}
            </span>
            <span className="plan-text">{step.text}</span>
            {/* The accessible name for the marker beside it. Visible only for
                the live step, where it is also the thing worth reading. */}
            <span className={step.state === "active" ? "plan-state" : "sr-only"}>
              {LABEL[step.state]}
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}

/** Matches `StepState::marker` in Rust, which is what the model reads back. */
const MARK: Record<PlanView["steps"][number]["state"], string> = {
  todo: "○",
  active: "◆",
  done: "✓",
};

const LABEL: Record<PlanView["steps"][number]["state"], string> = {
  todo: "to do",
  active: "in progress",
  done: "done",
};
