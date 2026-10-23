import { describe, expect, it } from "vitest";

import { spliceSpoken } from "./Composer";

describe("spliceSpoken", () => {
  it("fills an empty box", () => {
    expect(spliceSpoken("", 0, 0, "run the tests")).toEqual({
      text: "run the tests",
      caret: 13,
    });
  });

  it("joins a sentence already started with a space", () => {
    expect(spliceSpoken("fix the", 7, 7, "login test")).toEqual({
      text: "fix the login test",
      caret: 18,
    });
  });

  it("doesn't double a space that's already there", () => {
    expect(spliceSpoken("fix the ", 8, 8, "login test").text).toBe("fix the login test");
  });

  it("spaces off what follows the caret", () => {
    expect(spliceSpoken("then commit", 0, 0, "Run the tests,")).toEqual({
      text: "Run the tests, then commit",
      caret: 14,
    });
  });

  it("replaces a selection, which is how dictation corrects itself", () => {
    expect(spliceSpoken("open the canvas file", 9, 15, "composer").text).toBe(
      "open the composer file",
    );
  });
});
