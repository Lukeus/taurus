import { useState } from "react";

import { QuestionIcon } from "./icons";
import type { Answer, TranscriptView } from "../lib/api";

type QuestionsView = Extract<TranscriptView, { type: "questions" }>;

/**
 * An `ask_user` card.
 *
 * Unlike the other two views, a turn is parked behind this one: the tool call
 * that drew it is blocked until an answer comes back. That is what makes the
 * skip affordances load-bearing rather than polite. Every question can be left
 * alone, "You decide" answers all of them at once, and both send — they do not
 * dismiss the card, because a dismissed card would leave the turn waiting on
 * something no longer on screen.
 *
 * The card goes read-only the instant it is sent rather than when the harness
 * releases the call. Those are milliseconds apart, but they are milliseconds in
 * which a second click would answer a call that is no longer listening.
 */
export function QuestionsCard({
  view,
  /** The state of the call behind it: `running` means still waiting. */
  status,
  /** What the call returned, for a conversation reopened after the fact. */
  output,
  onAnswer,
}: {
  view: QuestionsView;
  status: "running" | "ok" | "error";
  output?: string;
  onAnswer: (id: string, answers: Answer[]) => void;
}) {
  const { id, questions } = view;
  const [picked, setPicked] = useState<Record<number, string[]>>({});
  const [typed, setTyped] = useState<Record<number, string>>({});
  const [sent, setSent] = useState(false);

  const live = status === "running" && !sent;
  const answers = questions.map<Answer>((question, i) => ({
    picked: picked[i] ?? [],
    other: question.allow_other ? (typed[i] ?? null) : null,
  }));
  const answered = answers.filter(
    (a) => a.picked.length > 0 || (a.other ?? "").trim() !== "",
  ).length;

  const send = (all: Answer[]) => {
    setSent(true);
    onAnswer(id, all);
  };

  const toggle = (index: number, label: string, multi: boolean) =>
    setPicked((current) => {
      const chosen = current[index] ?? [];
      if (multi) {
        return {
          ...current,
          [index]: chosen.includes(label)
            ? chosen.filter((l) => l !== label)
            : [...chosen, label],
        };
      }
      // Clicking the chosen option again clears it, which is how a
      // single-choice question is un-answered without a "none of these" row.
      return { ...current, [index]: chosen[0] === label ? [] : [label] };
    });

  return (
    <div className={`questions${live ? " live" : ""}`}>
      <div className="questions-head">
        <span className="questions-glyph">
          <QuestionIcon />
        </span>
        <div>
          <h3>
            {questions.length === 1
              ? "One thing before I start"
              : "A few things before I start"}
          </h3>
          <p className="view-caption">
            {live
              ? "Pick what fits — anything you skip, I'll decide and say what I picked."
              : "Answered."}
          </p>
        </div>
        <div className="spacer" />
        {live && (
          <span className="micro">
            {answered} / {questions.length}
          </span>
        )}
      </div>

      {questions.map((question, i) => {
        const multi = question.kind === "multi";
        const chosen = picked[i] ?? [];
        return (
          <div className="question" key={i}>
            <div className="question-prompt">
              <span className="micro question-no">
                {String(i + 1).padStart(2, "0")}
              </span>
              <div>
                <span>{question.prompt}</span>
                <span className="micro">
                  {multi ? "pick any that apply" : "pick one"}
                </span>
              </div>
            </div>
            <div className="question-options">
              {question.options.map((option) => {
                const on = chosen.includes(option.label);
                return (
                  <button
                    key={option.label}
                    className={`question-option${on ? " on" : ""}`}
                    disabled={!live}
                    onClick={() => toggle(i, option.label, multi)}
                  >
                    <span className={`pick${multi ? " multi" : ""}`}>
                      {on ? (multi ? "✓" : "●") : ""}
                    </span>
                    <span className="question-label">{option.label}</span>
                    {option.note && <span className="option-note">{option.note}</span>}
                  </button>
                );
              })}
              {question.allow_other && live && (
                <input
                  className="question-other"
                  placeholder="or type your own answer"
                  value={typed[i] ?? ""}
                  onChange={(e) =>
                    setTyped((current) => ({ ...current, [i]: e.target.value }))
                  }
                />
              )}
            </div>
          </div>
        );
      })}

      {live ? (
        <div className="questions-foot">
          <span className="micro">
            {answered === questions.length
              ? "all answered"
              : `${questions.length - answered} left — optional`}
          </span>
          <div className="spacer" />
          {/* Sends every question empty rather than clearing the form: the
              label is a decision, not an undo, and the turn behind the card is
              waiting for one either way. */}
          <button className="quiet" onClick={() => send(questions.map(() => empty()))}>
            You decide
          </button>
          <button className="primary" onClick={() => send(answers)}>
            Send answers
          </button>
        </div>
      ) : (
        // A conversation reopened later has the picks only as the text the
        // model was given, since nothing about the card itself is saved. Shown
        // verbatim rather than reverse-engineered back into selections: what
        // the model acted on is the useful record, and guessing at it from
        // parsed prose is how a transcript starts telling a slightly different
        // story than the turn did.
        sent || !output ? null : <pre className="questions-record">{output}</pre>
      )}
    </div>
  );
}

function empty(): Answer {
  return { picked: [], other: null };
}
