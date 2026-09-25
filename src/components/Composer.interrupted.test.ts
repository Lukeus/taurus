import { describe, expect, it } from "vitest";

import { interruptedText } from "./Composer";

describe("interruptedText", () => {
  it("says how many calls have unknown outcomes", () => {
    expect(interruptedText({ unanswered: 0, attempts: 1, max_attempts: 3 })).toBe(
      "This turn stopped when Taurus did.",
    );
    expect(interruptedText({ unanswered: 2, attempts: 1, max_attempts: 3 })).toBe(
      "This turn stopped when Taurus did. 2 calls were running; their outcome is unknown.",
    );
  });

  it("says why there's no Continue once the request is out of turns", () => {
    expect(interruptedText({ unanswered: 1, attempts: 3, max_attempts: 3 })).toContain(
      "It's had 3 turns, so send a message to go on.",
    );
  });
});
