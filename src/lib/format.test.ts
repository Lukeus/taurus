import { describe, expect, it } from "vitest";

import { clip, csvOf, duration, plural, when } from "./format";

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

describe("an older date", () => {
  it("says the month by name, and the year only when it isn't this one", () => {
    const now = new Date();
    const lastMonth = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 40);
    const older = new Date(now.getFullYear() - 1, 9, 9);
    const said = when(lastMonth.getTime() / 1000);
    expect(said).not.toMatch(/\d+\/\d+/);
    if (lastMonth.getFullYear() === now.getFullYear()) {
      expect(said).not.toContain(String(now.getFullYear()));
    }
    expect(when(older.getTime() / 1000)).toContain(String(now.getFullYear() - 1));
  });
});

describe("clip", () => {
  it("doesn't end halfway through a character", () => {
    expect(clip("ab😀cd", 3)).toBe("ab");
    expect(clip("ab😀cd", 4)).toBe("ab😀");
    expect(clip("abcd", 2)).toBe("ab");
    expect(clip("", 2)).toBe("");
  });
});
