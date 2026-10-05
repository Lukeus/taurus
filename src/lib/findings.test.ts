import { describe, expect, it } from "vitest";
import { hasFindings, reviewParts, withFinding } from "./findings";

describe("reviewParts", () => {
  it("cuts a numbered review into its findings and the prose around them", () => {
    const review = [
      "Two problems, hardest first.",
      "",
      "1. **src/auth.rs:42**: an empty token passes the check.",
      "   It's compared with `starts_with`, which is true for \"\".",
      "",
      "2. **src/auth.rs:60**: the error is swallowed.",
      "",
      "Claims:",
      "",
      "- \"the tests pass\": contradicted, the recorded run failed.",
    ].join("\n");

    expect(reviewParts(review)).toEqual([
      { text: "Two problems, hardest first.", finding: false },
      {
        text: '1. **src/auth.rs:42**: an empty token passes the check.\n   It\'s compared with `starts_with`, which is true for "".',
        finding: true,
      },
      { text: "2. **src/auth.rs:60**: the error is swallowed.", finding: true },
      { text: "Claims:", finding: false },
      { text: '- "the tests pass": contradicted, the recorded run failed.', finding: true },
    ]);
  });

  it("keeps a finding's lazy continuation and its indented code with it", () => {
    const review = [
      "- the loop never ends",
      "when the list is empty.",
      "",
      "  ```rust",
      "",
      "- not a new item inside a fence",
      "  ```",
      "- the next finding, after the fence closed",
    ].join("\n");
    const parts = reviewParts(review);
    expect(parts).toHaveLength(2);
    expect(parts[0].finding).toBe(true);
    expect(parts[0].text).toContain("not a new item inside a fence");
    expect(parts[1].text).toBe("- the next finding, after the fence closed");
  });

  it("finds nothing in a review that answered in paragraphs", () => {
    const parts = reviewParts("The change looks correct.\n\nNothing else to say.");
    expect(hasFindings(parts)).toBe(false);
    expect(parts.map((p) => p.text)).toEqual([
      "The change looks correct.\n\nNothing else to say.",
    ]);
  });
});

describe("withFinding", () => {
  it("adds the finding after the question, without its list marker", () => {
    expect(withFinding("Add a login check", "1. **auth.rs:42**: empty tokens pass.")).toBe(
      "Add a login check\n\nA review of an earlier attempt at this found the following. Take it into account:\n\n**auth.rs:42**: empty tokens pass.",
    );
  });
});
