import { describe, expect, it } from "vitest";

import { clampContextLimit, MIN_CONTEXT_LIMIT } from "./limits";

describe("working context as typed", () => {
  it("reads the forms people write six-digit numbers in", () => {
    expect(clampContextLimit("200000", 1)).toBe(200_000);
    expect(clampContextLimit("200,000", 1)).toBe(200_000);
    expect(clampContextLimit("200k", 1)).toBe(200_000);
    expect(clampContextLimit(" 150.5K ", 1)).toBe(150_500);
  });

  it("keeps zero, which means the whole window, and lifts anything else to the floor", () => {
    expect(clampContextLimit("0", 1)).toBe(0);
    expect(clampContextLimit("4k", 1)).toBe(MIN_CONTEXT_LIMIT);
  });

  it("keeps what was there for anything it cannot read", () => {
    expect(clampContextLimit("", 200_000)).toBe(200_000);
    expect(clampContextLimit("lots", 200_000)).toBe(200_000);
    expect(clampContextLimit("-5", 200_000)).toBe(200_000);
  });
});
