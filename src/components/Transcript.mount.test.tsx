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

function mount(entries: Entry[], find: string | null = null) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const answered: { id: string; answers: Answer[] }[] = [];
  const opened: { session: string; agent: string }[] = [];
  const render = (next: Entry[], nextFind: string | null = find) =>
    root.render(
      <Transcript
        entries={next}
        busy={false}
        empty={null}
        find={nextFind}
        onAnswer={(id, answers) => {
          answered.push({ id, answers });
        }}
        onOpenDelegate={(transcript) => opened.push(transcript)}
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
    opened,
    click: (element: Element | null) =>
      act(() => {
        (element as HTMLElement).click();
      }),
    rerender: (next: Entry[], nextFind?: string | null) =>
      act(() => render(next, nextFind === undefined ? find : nextFind)),
    found: () => [...host.querySelectorAll(".turn.found")],
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
    const times = [...host.querySelectorAll(".table-line:not(.head)")].map(
      (row) => row.children[1].textContent,
    );

    expect(times).toEqual(["42.1s", "18.4s", "11.9s"]);
  });

  it("reverses on a second click of the same column", () => {
    const { host, click } = mount([drew("show_table", TABLE)]);
    const time = () => host.querySelectorAll(".table-sort")[1];

    click(time());
    click(time());
    const times = [...host.querySelectorAll(".table-line:not(.head)")].map(
      (row) => row.children[1].textContent,
    );

    expect(times).toEqual(["11.9s", "18.4s", "42.1s"]);
  });

  it("tints a delta by direction, treating a rise as the cost", () => {
    const { host } = mount([drew("show_table", TABLE)]);
    const deltas = [...host.querySelectorAll(".table-value.delta")].map(
      (cell) => cell.className,
    );

    expect(deltas).toEqual([
      "table-value delta flat",
      "table-value delta down",
      "table-value delta up",
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

describe("a delegation", () => {
  const delegation = (
    patch: Partial<Extract<Entry, { kind: "tool" }>> = {},
  ): Entry => ({
    kind: "tool",
    id: "d1",
    name: "spawn_subagent",
    preview: "Delegate to explorer: find the parser",
    status: "running",
    steps: [],
    ...patch,
  });

  it("offers its own conversation while it is still running", () => {
    // While it runs is the case that matters: a delegation that looks stuck is
    // the one somebody wants to look into, and an offer that only appeared
    // with the result would arrive after the question.
    const { host, opened, click } = mount([
      delegation({ transcript: { session: "child1", agent: "explorer" } }),
    ]);

    const open = host.querySelector(".run-row-delegate");
    expect(open).not.toBeNull();
    expect(open!.textContent).toContain("explorer");

    click(open);
    expect(opened).toEqual([{ session: "child1", agent: "explorer" }]);
  });

  it("offers nothing when nothing was recorded", () => {
    // No recorder, no transcript, no offer to open one. An affordance that
    // opened an error is worse than no affordance.
    const { host } = mount([delegation({ status: "ok", output: "Found it." })]);
    expect(host.querySelector(".run-row-delegate")).toBeNull();
  });
});

describe("a conversation that has only just started", () => {
  it("survives its first message arriving in an empty transcript", () => {
    // The transcript draws a placeholder while there is nothing in it, and
    // that used to be an early return sitting in front of a hook. React counts
    // hooks per render, so the render where the first token lands called one
    // more than the render before it and the component came down — at the one
    // moment every new conversation passes through.
    const { host, rerender } = mount([]);
    expect(host.querySelector(".transcript.empty")).not.toBeNull();

    rerender([
      { kind: "user", id: "u1", text: "the first question", images: [] },
      { kind: "assistant", id: "a1", text: "the", thinking: "", open: true },
    ]);

    expect(host.querySelector(".transcript.empty")).toBeNull();
    expect(host.textContent).toContain("the first question");
  });
});

describe("landing on a search hit", () => {
  const said = (id: string, text: string): Entry => ({ kind: "user", id, text });
  const answered = (id: string, text: string): Entry => ({
    kind: "assistant",
    id,
    text,
    thinking: "",
    open: false,
  });

  it("marks nothing when there is nothing to find", () => {
    const ui = mount([said("u1", "fix the trust banner")]);
    expect(ui.found()).toHaveLength(0);
  });

  it("marks the turn that holds the text", () => {
    const ui = mount(
      [said("u1", "add a chart"), answered("a1", "done"), said("u2", "fix the trust banner")],
      "trust banner",
    );
    const marked = ui.found();
    expect(marked).toHaveLength(1);
    expect(marked[0].textContent).toContain("fix the trust banner");
  });

  it("marks the first turn that holds it, not every one", () => {
    // One mark, because there is one place the search sent you. A page of
    // marks is a page with no answer on it.
    const ui = mount(
      [said("u1", "widget"), answered("a1", "widget"), said("u2", "widget")],
      "widget",
    );
    expect(ui.found()).toHaveLength(1);
  });

  it("finds what the model said as well as what was asked", () => {
    const ui = mount(
      [said("u1", "why"), answered("a1", "because the freshness check ran")],
      "freshness",
    );
    expect(ui.found()[0].textContent).toContain("freshness");
  });

  it("ignores case, the way the search that sent it here did", () => {
    const ui = mount([said("u1", "The Trust Banner")], "trust banner");
    expect(ui.found()).toHaveLength(1);
  });

  it("marks nothing when the text is not in this conversation", () => {
    // A conversation that has since been compacted is the honest case: the
    // hit was in the transcript and is no longer on screen, and marking the
    // nearest thing instead would point at the wrong turn.
    const ui = mount([said("u1", "add a chart")], "trust banner");
    expect(ui.found()).toHaveLength(0);
  });

  it("takes the mark off again when the caller withdraws it", () => {
    const entries = [said("u1", "fix the trust banner")];
    const ui = mount(entries, "trust banner");
    expect(ui.found()).toHaveLength(1);
    ui.rerender(entries, null);
    expect(ui.found()).toHaveLength(0);
  });
});
