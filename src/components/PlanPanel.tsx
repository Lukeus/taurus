import { useEffect, useRef, useState } from "react";
import { isComposing } from "../lib/keys";

import type { PlanEdited, Step } from "../lib/api";
import type { PlanView } from "../state/store";

/** Mirrors `MAX_STEPS` and `MAX_STEP_CHARS` in `taurus_tools::builtin::plan`. */
const MAX_STEPS = 12;
const MAX_STEP_CHARS = 120;

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
 *
 * Opened, it can also be edited: a step added, reworded, moved or taken off,
 * while the turn runs or between turns. Finished steps are history and stay as
 * they are. `yours` names the steps the person wrote, which the model has to
 * keep — the backend refuses a list from it that drops one — so they carry a
 * word saying so. Without `onEdit` the panel only reads.
 */
export function PlanPanel({
  view,
  yours = [],
  onEdit,
}: {
  view: PlanView;
  yours?: string[];
  onEdit?: (seen: Step[], steps: Step[]) => Promise<PlanEdited | null>;
}) {
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<Step[] | null>(null);
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

      {open && editing && onEdit && (
        <PlanEditor
          from={editing}
          current={steps}
          onEdit={onEdit}
          onRestart={() => setEditing(steps)}
          onClose={() => setEditing(null)}
        />
      )}

      {open && !editing && (
        <div className="plan-body">
          <ol className="plan-list">
            {steps.map((step, i) => (
              <li key={i} className={`plan-step ${step.state}`}>
                {/* The mark is a shape, and a shape cannot be read aloud. The
                    status beside it is the same state in words. */}
                <span className="plan-mark" aria-hidden="true">
                  {step.state === "done" ? "✓" : ""}
                </span>
                <span className="plan-text">
                  {step.text}
                  {yours.includes(step.text.trim()) && (
                    <span className="plan-yours micro">added by you</span>
                  )}
                  {/* What the model said would prove the step, which is what
                      it's asked to run before calling the work done. */}
                  {step.check && (
                    <span className="plan-check block font-mono text-11-5 text-faint wrap-anywhere">
                      check: {step.check}
                    </span>
                  )}
                </span>
                <span className="plan-status">{STATUS[step.state]}</span>
              </li>
            ))}
          </ol>
          <div className="plan-foot">
            <span>{foot}</span>
            {onEdit && (
              <button type="button" className="plan-edit" onClick={() => setEditing(steps)}>
                Edit
              </button>
            )}
          </div>
        </div>
      )}
    </section>
  );
}

/**
 * The plan as a list of boxes, and what saving it does.
 *
 * Works on a copy taken when it opened (`from`), and sends that copy back as
 * what it started from. If the model writes a new list meanwhile, `current`
 * stops matching it: the editor says so before Save rather than after, and
 * the backend refuses the save anyway — applying a list drawn from an older
 * plan would quietly undo what the model just did.
 *
 * Finished steps are drawn but not editable, and new steps go after them. That
 * is the same ordering rule the model is held to, kept here by construction
 * rather than by an error after the fact.
 */
function PlanEditor({
  from,
  current,
  onEdit,
  onRestart,
  onClose,
}: {
  from: Step[];
  current: Step[];
  onEdit: (seen: Step[], steps: Step[]) => Promise<PlanEdited | null>;
  onRestart: () => void;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState<Step[]>(from);
  const [adding, setAdding] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const addRef = useRef<HTMLInputElement>(null);

  // A restart hands in a new `from`; the draft starts again from it. Whatever
  // was said about the last attempt stays up, since a refused save is the
  // usual reason for the restart.
  useEffect(() => {
    setDraft(from);
  }, [from]);

  const finished = draft.filter((step) => step.state === "done").length;
  const moved = !sameSteps(from, current);
  const changed = !sameSteps(from, draft);
  const blank = draft.some((step) => !step.text.trim());
  const full = draft.length >= MAX_STEPS;

  const update = (at: number, text: string) =>
    setDraft((steps) => steps.map((step, i) => (i === at ? { ...step, text } : step)));
  const remove = (at: number) => setDraft((steps) => steps.filter((_, i) => i !== at));
  const move = (at: number, by: -1 | 1) =>
    setDraft((steps) => {
      const to = at + by;
      if (to < finished || to >= steps.length) return steps;
      const next = [...steps];
      [next[at], next[to]] = [next[to], next[at]];
      return next;
    });
  const add = () => {
    const text = adding.trim();
    if (!text || full) return;
    setDraft((steps) => [...steps, { text, state: "todo" }]);
    setAdding("");
    addRef.current?.focus();
  };

  const save = async () => {
    if (!changed || blank || saving) return;
    setSaving(true);
    setProblem(null);
    try {
      const result = await onEdit(from, draft);
      if (!result) return;
      switch (result.outcome) {
        case "saved":
        case "unchanged":
          onClose();
          break;
        case "stale":
          setProblem(result.message);
          onRestart();
          break;
        case "invalid":
          setProblem(result.message);
          break;
      }
    } catch (e) {
      setProblem(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      className="plan-body plan-editing"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          onClose();
        } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          void save();
        }
      }}
    >
      <ol className="plan-list">
        {draft.map((step, i) =>
          step.state === "done" ? (
            <li key={`done-${i}`} className="plan-step done">
              <span className="plan-mark" aria-hidden="true">
                ✓
              </span>
              <span className="plan-text">{step.text}</span>
              <span className="plan-status">done</span>
            </li>
          ) : (
            <li key={i} className={`plan-step plan-step-edit ${step.state}`}>
              <span className="plan-mark" aria-hidden="true" />
              <input
                className="plan-input"
                value={step.text}
                maxLength={MAX_STEP_CHARS}
                aria-label={`Step ${i + 1}`}
                onChange={(e) => update(i, e.target.value)}
              />
              <span className="plan-tools">
                <button
                  type="button"
                  aria-label={`Move step ${i + 1} up`}
                  disabled={i <= finished}
                  onClick={() => move(i, -1)}
                >
                  ↑
                </button>
                <button
                  type="button"
                  aria-label={`Move step ${i + 1} down`}
                  disabled={i >= draft.length - 1}
                  onClick={() => move(i, 1)}
                >
                  ↓
                </button>
                <button
                  type="button"
                  aria-label={`Remove step ${i + 1}`}
                  disabled={draft.length <= 1}
                  onClick={() => remove(i)}
                >
                  ×
                </button>
              </span>
            </li>
          ),
        )}
      </ol>

      <div className="plan-add">
        <input
          ref={addRef}
          className="plan-input"
          value={adding}
          maxLength={MAX_STEP_CHARS}
          placeholder={full ? `A plan holds ${MAX_STEPS} steps at most` : "Add a step"}
          aria-label="New step"
          disabled={full}
          onChange={(e) => setAdding(e.target.value)}
          onKeyDown={(e) => {
            if (isComposing(e)) return;
            if (e.key === "Enter" && !e.metaKey && !e.ctrlKey) {
              e.preventDefault();
              add();
            }
          }}
        />
        <button type="button" disabled={!adding.trim() || full} onClick={add}>
          Add
        </button>
      </div>

      {moved && !problem && (
        <p className="plan-problem" role="status">
          Taurus has changed the plan since you started editing, so this can&apos;t be saved.{" "}
          <button type="button" className="plan-edit" onClick={onRestart}>
            Start again from the current plan
          </button>
        </p>
      )}
      {problem && (
        <p className="plan-problem" role="alert">
          {problem}
        </p>
      )}

      <div className="plan-foot">
        <span>Taurus keeps the steps you add, and reads the change before its next step.</span>
        <button type="button" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="primary"
          disabled={!changed || blank || saving || moved}
          onClick={() => void save()}
        >
          Save
        </button>
      </div>
    </div>
  );
}

/** Text and state, ends trimmed: the comparison the backend makes. */
function sameSteps(a: Step[], b: Step[]): boolean {
  return (
    a.length === b.length &&
    a.every((step, i) => step.state === b[i].state && step.text.trim() === b[i].text.trim())
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
