import { describe, expect, it } from "vitest";

import { csvOf, plural } from "./format";

describe("rows as csv", () => {
  it("quotes only the cells that need it, and doubles the quotes inside", () => {
    expect(
      csvOf([
        ["a", "b,c"],
        ['say "hi"', "two\nlines"],
      ]),
    ).toBe('a,"b,c"\n"say ""hi""","two\nlines"');
  });

  it("leaves a plain table plain", () => {
    expect(csvOf([["name", "rows"], ["events", "1,240"]])).toBe('name,rows\nevents,"1,240"');
  });
});

describe("a count with its noun", () => {
  it("is singular at one and plural otherwise, zero included", () => {
    expect(plural(1, "file")).toBe("1 file");
    expect(plural(2, "file")).toBe("2 files");
    expect(plural(0, "file")).toBe("0 files");
    expect(plural(3, "directory", "directories")).toBe("3 directories");
  });
});
