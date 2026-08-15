// @vitest-environment jsdom
//
// The streaming terminal is stateful in ways a first paint cannot show: it
// follows the output while a command runs, and has to keep showing something
// once it stops. Both need a real document and a component that stays mounted.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { Transcript } from "./Transcript";
import type { Answer, TranscriptView } from "../lib/api";
import type { Entry } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// jsdom lays nothing out and implements neither of these. The transcript pins
// itself to the bottom on every render, so without the stub every test here
// fails on the scroll rather than on what it is testing.
Element.prototype.scrollIntoView = () => {};

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

function mount(entries: Entry[]) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const answered: { id: string; answers: Answer[] }[] = [];
  const render = (next: Entry[]) =>
    root.render(
      <Transcript
        entries={next}
        busy={false}
        empty={null}
        onAnswer={(id, answers) => answered.push({ id, answers })}
      />,
    );

  act(() => render(entries));
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return {
    host,
    answered,
    click: (element: Element | null) =>
      act(() => {
        (element as HTMLElement).click();
      }),
    rerender: (next: Entry[]) => act(() => render(next)),
  };
}

const command = (patch: Partial<Extract<Entry, { kind: "tool" }>> = {}): Entry => ({
  kind: "tool",
  id: "t1",
  name: "run_command",
  preview: "Run: cargo build",
  status: "running",
  steps: [],
  ...patch,
});

describe("a running command", () => {
  it("shows what it has printed before it finishes", () => {
    const { host } = mount([
      command({ steps: ["   Compiling taurus-core\n", "   Compiling taurus-cli\n"] }),
    ]);

    const stream = host.querySelector(".tool-stream");
    expect(stream).not.toBeNull();
    expect(stream!.textContent).toContain("Compiling taurus-core");
    expect(stream!.textContent).toContain("Compiling taurus-cli");
    // Marked live, which is what tells it apart from a finished result.
    expect(stream!.className).toContain("live");
  });

  it("keeps its output on screen once it exits", () => {
    // The row must not go blank at the moment the command ends — that reads as
    // the output having been lost.
    const { host, rerender } = mount([command({ steps: ["building…\n"] })]);
    rerender([
      command({
        status: "ok",
        steps: ["building…\n"],
        output: "building…\nFinished in 3.1s",
      }),
    ]);

    const stream = host.querySelector(".tool-stream")!;
    // The authoritative result replaces the streamed copy in the same place.
    expect(stream.textContent).toContain("Finished in 3.1s");
    expect(stream.className).not.toContain("live");
  });

  it("does not draw an empty terminal before anything is printed", () => {
    const { host } = mount([command()]);
    expect(host.querySelector(".tool-stream")).toBeNull();
  });
});

/** A call carrying a drawn view, which is how all three cards reach the DOM. */
const drew = (name: string, view: TranscriptView, patch: Partial<ToolEntry> = {}): Entry => ({
  kind: "tool",
  id: view.type === "questions" ? view.id : "v1",
  name,
  preview: "…",
  status: "ok",
  steps: [],
  view,
  ...patch,
});

type ToolEntry = Extract<Entry, { kind: "tool" }>;

const TABLE: TranscriptView = {
  type: "table",
  title: "Crates by build time",
  caption: "cargo build --timings",
  columns: [
    { label: "Crate", kind: "text" },
    { label: "Time", kind: "number" },
    { label: "Δ", kind: "delta" },
  ],
  rows: [
    ["taurus-mcp", "18.4s", "—"],
    ["taurus-core", "42.1s", "-8%"],
    ["taurus-agents", "11.9s", "+22%"],
  ],
};

const QUESTIONS: TranscriptView = {
  type: "questions",
  id: "call-7",
  questions: [
    {
      prompt: "Where should the rename land first?",
      kind: "single",
      allow_other: false,
      options: [
        { label: "Settings panel only", note: "2 files" },
        { label: "Every call site at once", note: "11 files" },
      ],
    },
    {
      prompt: "Update what alongside it?",
      kind: "multi",
      allow_other: false,
      options: [
        { label: "Tests", note: "" },
        { label: "Bindings", note: "" },
      ],
    },
  ],
};

describe("a table", () => {
  it("stands on its own rather than folding into the run beside it", () => {
    // Filed under "2 steps" behind a disclosure triangle, the answer to the
    // question is one click further away than the work that produced it.
    const { host } = mount([
      command({ id: "t0", status: "ok", output: "done" }),
      drew("show_table", TABLE),
    ]);

    expect(host.querySelector(".view-card")).not.toBeNull();
    expect(host.querySelectorAll(".run").length).toBe(1);
    expect(host.querySelector(".run")!.textContent).not.toContain("Crates");
  });

  it("sorts by the column that was clicked, numerically", () => {
    const { host, click } = mount([drew("show_table", TABLE)]);

    // Second header is `Time`; its cells read 18.4s / 42.1s / 11.9s unsorted.
    click(host.querySelectorAll(".table-sort")[1]);
    const times = [...host.querySelectorAll(".table-row:not(.head)")].map(
      (row) => row.children[1].textContent,
    );

    expect(times).toEqual(["42.1s", "18.4s", "11.9s"]);
  });

  it("reverses on a second click of the same column", () => {
    const { host, click } = mount([drew("show_table", TABLE)]);
    const time = () => host.querySelectorAll(".table-sort")[1];

    click(time());
    click(time());
    const times = [...host.querySelectorAll(".table-row:not(.head)")].map(
      (row) => row.children[1].textContent,
    );

    expect(times).toEqual(["11.9s", "18.4s", "42.1s"]);
  });

  it("tints a delta by direction, treating a rise as the cost", () => {
    const { host } = mount([drew("show_table", TABLE)]);
    const deltas = [...host.querySelectorAll(".table-cell.delta")].map(
      (cell) => cell.className,
    );

    expect(deltas).toEqual([
      "table-cell delta flat",
      "table-cell delta down",
      "table-cell delta up",
    ]);
  });
});

describe("a chart", () => {
  const chart = (series: TranscriptView & { type: "chart" }) =>
    mount([drew("show_chart", series)]);

  it("scales every bar against the largest value", () => {
    const { host } = chart({
      type: "chart",
      title: "Tool calls per turn",
      caption: null,
      labels: ["t1", "t2"],
      series: [{ name: "tool calls", unit: "", values: [4, 8] }],
    });

    const heights = [...host.querySelectorAll(".chart-fill")].map(
      (bar) => (bar as HTMLElement).style.height,
    );
    expect(heights).toEqual(["50%", "100%"]);
  });

  it("offers a tab per series, and only when there is more than one", () => {
    const one = chart({
      type: "chart",
      title: "t",
      caption: null,
      labels: ["a"],
      series: [{ name: "calls", unit: "", values: [1] }],
    });
    expect(one.host.querySelectorAll(".chart-tabs .pill").length).toBe(0);

    const two = chart({
      type: "chart",
      title: "t",
      caption: null,
      labels: ["a"],
      series: [
        { name: "calls", unit: "", values: [1] },
        { name: "tokens", unit: "k", values: [9] },
      ],
    });
    expect(
      [...two.host.querySelectorAll(".chart-tabs .pill")].map((p) => p.textContent),
    ).toEqual(
      ["calls", "tokens"],
    );
  });

  it("switches the plotted series when a tab is picked", () => {
    const { host, click } = chart({
      type: "chart",
      title: "t",
      caption: null,
      labels: ["a", "b"],
      series: [
        { name: "calls", unit: "", values: [4, 8] },
        { name: "tokens", unit: "k", values: [30, 10] },
      ],
    });

    click(host.querySelectorAll(".chart-tabs .pill")[1]);
    const heights = [...host.querySelectorAll(".chart-fill")].map(
      (bar) => (bar as HTMLElement).style.height,
    );

    expect(heights).toEqual(["100%", "33.33333333333333%"]);
  });
});

describe("a question card", () => {
  it("sends one answer per question, in order", () => {
    const { host, click, answered } = mount([
      drew("ask_user", QUESTIONS, { status: "running" }),
    ]);

    const options = host.querySelectorAll(".question-option");
    click(options[1]); // second option of the single-choice question
    click(options[2]); // first option of the multi-choice question
    click(options[3]); // and the second
    click([...host.querySelectorAll("button")].find((b) => b.textContent === "Send answers")!);

    expect(answered).toEqual([
      {
        id: "call-7",
        answers: [
          { picked: ["Every call site at once"], other: null },
          { picked: ["Tests", "Bindings"], other: null },
        ],
      },
    ]);
  });

  it("replaces a single choice rather than adding to it", () => {
    const { host, click, answered } = mount([
      drew("ask_user", QUESTIONS, { status: "running" }),
    ]);

    const options = host.querySelectorAll(".question-option");
    click(options[0]);
    click(options[1]);
    click([...host.querySelectorAll("button")].find((b) => b.textContent === "Send answers")!);

    expect(answered[0].answers[0].picked).toEqual(["Every call site at once"]);
  });

  it("treats 'You decide' as an answer, not as a way to clear the form", () => {
    // The turn behind the card is blocked either way, so the button has to
    // send something; sending everything skipped is what the label promises.
    const { host, click, answered } = mount([
      drew("ask_user", QUESTIONS, { status: "running" }),
    ]);

    click(host.querySelectorAll(".question-option")[0]);
    click([...host.querySelectorAll("button")].find((b) => b.textContent === "You decide")!);

    expect(answered).toEqual([
      {
        id: "call-7",
        answers: [
          { picked: [], other: null },
          { picked: [], other: null },
        ],
      },
    ]);
  });

  it("cannot be answered twice", () => {
    // The call is released milliseconds after the first click, and a second
    // one in that window would answer a call that is no longer listening.
    const { host, click, answered } = mount([
      drew("ask_user", QUESTIONS, { status: "running" }),
    ]);
    const send = [...host.querySelectorAll("button")].find(
      (b) => b.textContent === "Send answers",
    )!;

    click(send);
    expect(host.querySelector(".questions-foot")).toBeNull();
    expect(answered.length).toBe(1);
  });

  it("shows the recorded answer when the conversation is reopened", () => {
    // Nothing about the card itself is saved, so the text the model was given
    // is the only record of what was picked.
    const { host } = mount([
      drew("ask_user", QUESTIONS, {
        status: "ok",
        output: "The user answered:\n1. Where should the rename land first? — Settings panel only",
      }),
    ]);

    expect(host.querySelector(".questions-foot")).toBeNull();
    expect(host.querySelector(".questions-record")!.textContent).toContain(
      "Settings panel only",
    );
  });
});

describe("every other tool", () => {
  it("keeps its result behind the row rather than streaming it", () => {
    // A read is instant. Streaming every one of them would bury the sentences
    // either side of the run.
    const { host } = mount([
      {
        kind: "tool",
        id: "t2",
        name: "read_file",
        preview: "Read src/main.rs",
        status: "ok",
        steps: [],
        output: "fn main() {}",
      },
    ]);

    expect(host.querySelector(".tool-stream")).toBeNull();
    expect(host.textContent).toContain("Read src/main.rs");
    expect(host.textContent).not.toContain("fn main()");
  });
});
