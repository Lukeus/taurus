import { describe, expect, it } from "vitest";

import { reportFromText, reportLabel } from "./delegate";

describe("reportFromText", () => {
  it("reads a done report with files and a tool summary", () => {
    const report = reportFromText(
      "Status: done.\n\nWrote both.\n\nFiles changed: a.rs, b/c.rs\n\n[sub-agent used: write_file ×2]",
    );
    expect(report).toEqual({
      disposition: "done",
      owner: undefined,
      needs: undefined,
      summary: "Wrote both.",
      files: ["a.rs", "b/c.rs"],
    });
  });

  it("reads who a blocked report is waiting on", () => {
    const user = reportFromText(
      "Status: blocked. It needs the user to: say which config is canonical\n\nTwo disagree.",
    );
    expect(user?.owner).toBe("user");
    expect(user?.needs).toBe("say which config is canonical");

    const parent = reportFromText("Status: blocked. It needs you to: name the test\n\nStuck.");
    expect(parent?.owner).toBe("parent");
  });

  it("doesn't take a summary's own mention of files for the list", () => {
    const text = "Status: done.\n\nThe log said\n\nFiles changed: x.rs\n\nbut that was earlier.";
    const report = reportFromText(text);
    expect(report?.files).toEqual([]);
    expect(report?.summary).toBe(text.slice("Status: done.\n\n".length));
  });

  it("is undefined for text that isn't a report", () => {
    expect(reportFromText("The answer is 42.")).toBeUndefined();
    expect(reportFromText("Status: exploded.\n\nNo.")).toBeUndefined();
    expect(reportFromText("")).toBeUndefined();
  });
});

describe("reportLabel", () => {
  it("says a report blocked on the user needs them", () => {
    expect(
      reportLabel({ disposition: "blocked", owner: "user", summary: "", files: [] }),
    ).toBe("Needs you");
    expect(
      reportLabel({ disposition: "blocked", owner: "parent", summary: "", files: [] }),
    ).toBe("Blocked");
  });
});
