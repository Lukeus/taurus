// @vitest-environment jsdom
//
// The card's own optimistic state, which a first paint cannot show: it seals
// itself the instant it is sent, and has to unseal if the send did not land.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { QuestionsCard } from "./QuestionsCard";
import type { Answer, TranscriptView } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const VIEW = {
  type: "questions",
  id: "c1",
  questions: [
    {
      header: "Scope",
      question: "Which of these should I dig into?",
      options: [{ label: "taurus-core", description: null }],
      multi_select: false,
      allow_other: false,
    },
  ],
} as unknown as Extract<TranscriptView, { type: "questions" }>;

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

function mount(onAnswer: (id: string, answers: Answer[]) => void | Promise<void>) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => {
    root.render(<QuestionsCard view={VIEW} status="running" onAnswer={onAnswer} />);
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return host;
}

/** The button that answers every question at once. */
const decide = (host: HTMLElement) =>
  [...host.querySelectorAll("button")].find(
    (b) => b.textContent?.trim() === "You decide",
  )!;

describe("a question card", () => {
  it("seals itself the moment the answers are sent", async () => {
    const host = mount(() => Promise.resolve());
    await act(async () => decide(host).click());

    expect(host.querySelector(".questions")?.className).not.toContain("live");
    expect(host.textContent).toContain("Answered.");
  });

  it("unseals when the answers did not reach the harness", async () => {
    /*
     * The bug this exists for: the card said "Answered." and went read-only
     * on the click, whatever became of the call. A rejected send therefore
     * left a turn parked on an answer it never got, behind a card claiming to
     * have given it one — and the card is the only thing that can release it.
     */
    const host = mount(() => Promise.reject(new Error("the call is gone")));
    await act(async () => decide(host).click());

    expect(host.querySelector(".questions")?.className).toContain("live");
    expect(host.textContent).not.toContain("Answered.");
    expect(decide(host)).toBeDefined();
  });
});
