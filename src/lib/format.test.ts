import { describe, expect, it } from "vitest";

import { csvOf, duration, plural } from "./format";

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

describe("how long something took", () => {
  it("counts in tenths, then seconds, then minutes", () => {
    expect(duration(400)).toBe("0.4s");
    expect(duration(2_400)).toBe("2s");
    expect(duration(90_000)).toBe("1m 30s");
    expect(duration(120_000)).toBe("2m");
  });

  it("switches to hours, where the seconds have stopped saying anything", () => {
    // A turn can now run for as long as the work takes, so this is a length
    // the app has to be able to print. `127m 4s` is arithmetic, not a reading.
    expect(duration(3_600_000)).toBe("1h");
    expect(duration(7_624_000)).toBe("2h 7m");
  });
});
