import { describe, expect, it } from "vitest";

import { compareNumbers } from "./TableCard";

const sorted = (cells: string[]) => [...cells].sort(compareNumbers);

describe("sorting a number column", () => {
  it("reads thousands separators as part of the number", () => {
    expect(sorted(["1,500", "999", "1,000,000", "12,000"])).toEqual([
      "999",
      "1,500",
      "12,000",
      "1,000,000",
    ]);
  });

  it("goes by value through a currency sign and a sign", () => {
    expect(sorted(["$1,200", "−3,000", "$80"])).toEqual(["−3,000", "$80", "$1,200"]);
  });

  it("doesn't take a decimal comma for a separator", () => {
    expect(sorted(["15", "1,5"])).toEqual(["1,5", "15"]);
  });
});
