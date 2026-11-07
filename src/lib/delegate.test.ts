import { describe, expect, it } from "vitest";

import { reportFromText, reportLabel, takeBackgroundReports } from "./delegate";

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
      notes: [],
    });
  });

  it("reads back where a copy's work is kept, and the notes above it", () => {
    const report = reportFromText(
      [
        "Status: done.",
        "",
        "Changed a.rs.",
        "",
        "Note: Copied in because .worktreeinclude lists them: .env.",
        "",
        'Kept as run 0123456789ab, started from deadbeef00. It worked in a copy of the workspace, so nothing in the workspace has changed yet. Apply it with `apply_delegate` and run "0123456789ab", or leave it for the user to apply from its card. It also changed files outside the workspace, which are kept but never applied: ../top.txt, ../b.txt.',
        "",
        "Files changed: a.rs",
        "",
        "[sub-agent used: write_file ×1]",
      ].join("\n"),
    );
    expect(report?.summary).toBe("Changed a.rs.");
    expect(report?.notes).toEqual(["Copied in because .worktreeinclude lists them: .env."]);
    expect(report?.kept).toEqual({
      run: "0123456789ab",
      base: "deadbeef00",
      files: ["a.rs"],
      outside: ["../top.txt", "../b.txt"],
    });
  });

  it("doesn't take a summary's own mention of a run for the harness's", () => {
    const text = "Status: done.\n\nKept as run 0123456789ab, started from aa.\nThen more prose.";
    const report = reportFromText(text);
    expect(report?.kept).toBeUndefined();
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
      reportLabel({ disposition: "blocked", owner: "user", summary: "", files: [], notes: [] }),
    ).toBe("Needs you");
    expect(
      reportLabel({ disposition: "blocked", owner: "parent", summary: "", files: [], notes: [] }),
    ).toBe("Blocked");
  });
});

describe("takeBackgroundReports", () => {
  const block = (call: string) =>
    `<background-report call="${call}" agent="explorer">\nStatus: done.\n\nFound it.\n</background-report>`;

  it("takes every report out and leaves nothing when that was all", () => {
    const { reports, rest } = takeBackgroundReports(`${block("a")}\n\n${block("b")}`);
    expect(reports.map((r) => r.call)).toEqual(["a", "b"]);
    expect(reports[0]).toEqual({
      call: "a",
      agent: "explorer",
      text: "Status: done.\n\nFound it.",
    });
    expect(rest).toBe("");
  });

  it("leaves text that isn't a report alone", () => {
    const text = "Please look at <background-report> handling.";
    expect(takeBackgroundReports(text)).toEqual({ reports: [], rest: text });
  });
});
