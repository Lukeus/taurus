import { describe, expect, it } from "vitest";

import type { BackgroundJob, JobOutput } from "./api";
import { extend, mark, PANE_LIMIT, size, title, tone } from "./jobs";

const job = (patch: Partial<BackgroundJob> = {}): BackgroundJob => ({
  id: 3,
  command: "cargo build",
  running: true,
  stopped: false,
  code: undefined,
  ran_for: 12,
  status: "still running after 12s",
  ...patch,
});

const said = (patch: Partial<JobOutput> = {}): JobOutput => ({
  id: 3,
  text: "",
  missed: 0,
  cursor: 0,
  ...patch,
});

describe("a tab's name", () => {
  it("is the command, when the command fits", () => {
    expect(title("cargo build")).toBe("cargo build");
  });

  it("is one line, whatever the command was", () => {
    // A command built across several lines arrives with the newlines in it,
    // and a tab is one line high — untouched, the strip grows to the tallest
    // command anybody has run.
    expect(title("make\n  all")).toBe("make all");
  });

  it("says where it was cut", () => {
    const cut = title("cargo build --release --all-features --workspace");
    expect(cut.length).toBeLessThanOrEqual(22);
    expect(cut.endsWith("…")).toBe(true);
  });
});

describe("what a finished command's tab shows", () => {
  it("marks a failure as well as colouring it", () => {
    // The diff's argument: a strip where a failure and a run differ only in
    // hue says nothing in a screenshot or to a reader who cannot tell the two
    // apart.
    expect(mark(job({ running: false, code: 1 }))).toBe("✗");
    expect(tone(job({ running: false, code: 1 }))).toBe("failed");
  });

  it("does not read a stopped command as a failed one", () => {
    // A killed process has no meaningful code, and calling that a failure is
    // the one wrong thing this could say about a command the user stopped.
    const halted = job({ running: false, stopped: true, code: null as never });
    expect(tone(halted)).toBe("done");
    expect(mark(halted)).toBe("–");
  });

  it("leaves a running command unmarked", () => {
    expect(mark(job())).toBe("");
    expect(tone(job())).toBe("running");
  });

  it("marks a clean exit", () => {
    expect(mark(job({ running: false, code: 0 }))).toBe("✓");
    expect(tone(job({ running: false, code: 0 }))).toBe("done");
  });
});

describe("collecting the output", () => {
  it("appends what arrived", () => {
    const first = extend("", said({ text: "one\n" }));
    expect(extend(first, said({ text: "two\n" }))).toBe("one\ntwo\n");
  });

  it("writes a gap where the output actually went missing", () => {
    // Not a note in a corner: a reader who is told somewhere else that some
    // lines are absent still has to guess which line does not follow from the
    // one above it.
    const held = extend("early\n", said({ text: "late\n", missed: 4096 }));
    expect(held).toBe("early\n[4 KB of output not shown]\nlate\n");
  });

  it("does not open a pane with a blank line above its first notice", () => {
    expect(extend("", said({ text: "late\n", missed: 99 }))).toBe(
      "[99 bytes of output not shown]\nlate\n",
    );
  });

  it("starts the notice on its own line even mid-line", () => {
    const held = extend("half a line", said({ text: "rest\n", missed: 10 }));
    expect(held.split("\n")[1]).toBe("[10 bytes of output not shown]");
  });

  it("forgets the oldest whole lines rather than half of one", () => {
    // A cut mid-line puts the tail of a sentence at the top of the pane,
    // looking like something the command wrote.
    const long = `${"x".repeat(PANE_LIMIT)}\nlast line\n`;
    const held = extend("", said({ text: long }));
    expect(held.length).toBeLessThanOrEqual(PANE_LIMIT);
    expect(held).toBe("last line\n");
  });

  it("keeps everything that fits", () => {
    const text = "a\n".repeat(100);
    expect(extend("", said({ text }))).toBe(text);
  });
});

describe("a byte count", () => {
  it("stays exact while it is small enough to mean something", () => {
    expect(size(512)).toBe("512 bytes");
  });

  it("rounds once it is not", () => {
    expect(size(2048)).toBe("2 KB");
    expect(size(3 * 1024 * 1024)).toBe("3.0 MB");
  });
});
