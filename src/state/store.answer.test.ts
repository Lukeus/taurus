// The two answers a parked turn is waiting on, and what happens when the call
// carrying one does not land. Actions rather than the reducer, so Tauri has to
// be stood in for.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { useStore } from "./store";

const PERMISSION = {
  id: "p1",
  tool: "write_file",
  title: "Write src/main.rs",
  detail: null,
  diff: null,
  rule: null,
};

beforeEach(() => {
  invoke.mockReset();
  useStore.setState({ permission: PERMISSION as never, error: null });
});

describe("answering a permission prompt", () => {
  it("dismisses the dialog once the decision is away", async () => {
    invoke.mockResolvedValue(undefined);
    await useStore.getState().answerPermission("allow_once" as never);

    expect(useStore.getState().permission).toBeNull();
    expect(useStore.getState().error).toBeNull();
    expect(invoke.mock.calls[0][0]).toBe("respond_permission");
  });

  it("puts the dialog back when the decision does not land", async () => {
    /*
     * The bug this exists for: the dialog was cleared before the call and the
     * call was not guarded, so a rejection left the turn parked on a decision
     * with nothing on screen able to give it one — and no banner to say why.
     * A prompt that vanishes and a conversation that never resumes is the
     * worst pair of symptoms in the app, because neither points at the other.
     */
    invoke.mockRejectedValue(new Error("the harness is gone"));
    await useStore.getState().answerPermission("allow_once" as never);

    expect(useStore.getState().permission).toEqual(PERMISSION);
    expect(useStore.getState().error).toContain("the harness is gone");
  });
});

describe("answering a question card", () => {
  it("says nothing when the answers are away", async () => {
    invoke.mockResolvedValue(undefined);
    await useStore.getState().answerQuestions("c1", []);

    expect(useStore.getState().error).toBeNull();
  });

  it("raises the banner and rethrows so the card can be answered again", async () => {
    // Both halves matter: the banner is the only thing that says anything went
    // wrong, and the rejection is what `QuestionsCard` catches to take back its
    // optimistic "Answered."
    invoke.mockRejectedValue(new Error("the call is gone"));

    await expect(useStore.getState().answerQuestions("c1", [])).rejects.toThrow(
      "the call is gone",
    );
    expect(useStore.getState().error).toContain("the call is gone");
  });
});
