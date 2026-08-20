import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  AppStatus,
  Message,
  SessionMeta,
  Switch,
  UiEvent,
} from "../lib/api";
import {
  batchEvents,
  entriesFromMessages,
  mergeChanged,
  mergeSession,
  pinnedPlan,
  reduce,
  viewFromCall,
  type Entry,
} from "./store";

/** Folds a whole event sequence, as a real turn would arrive. */
const run = (...events: UiEvent[]): Entry[] => events.reduce(reduce, []);

const text = (t: string): UiEvent => ({ type: "text_delta", text: t });

describe("transcript reducer", () => {
  it("merges consecutive text deltas into one entry", () => {
    const entries = run(text("Hello "), text("there"));
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({ kind: "assistant", text: "Hello there" });
  });

  it("keeps reasoning in the same entry as the answer but in its own field", () => {
    // One bubble with a collapsible reasoning section, not two bubbles.
    const entries = run(
      { type: "thinking_delta", text: "let me think" },
      text("the answer"),
    );
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      kind: "assistant",
      thinking: "let me think",
      text: "the answer",
    });
  });

  it("opens a tool entry and completes it in place", () => {
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "read_file", preview: "Read a.txt" },
      { type: "tool_call_finished", id: "t1", ok: true, output: "contents" },
    );
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      kind: "tool",
      id: "t1",
      name: "read_file",
      preview: "Read a.txt",
      status: "ok",
      output: "contents",
    });
  });

  it("times the call so a run of steps can report how long it took", () => {
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "run_command", preview: "run" },
      { type: "tool_call_finished", id: "t1", ok: true, output: "" },
    );
    const tool = entries[0] as Extract<Entry, { kind: "tool" }>;
    expect(tool.startedAt).toEqual(expect.any(Number));
    expect(tool.endedAt).toBeGreaterThanOrEqual(tool.startedAt!);
  });

  it("collects a delegation's progress under the call it belongs to", () => {
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "spawn_subagent", preview: "Delegate" },
      { type: "tool_progress", id: "t1", label: "read_file config.rs" },
      { type: "tool_progress", id: "t1", label: "grep load_config" },
    );
    const tool = entries[0] as Extract<Entry, { kind: "tool" }>;
    expect(tool.steps).toEqual(["read_file config.rs", "grep load_config"]);
  });

  it("keeps a command's streamed output as a bounded scrollback", () => {
    // A build emits tens of thousands of lines. Holding all of them would grow
    // without limit and re-render the lot on every batch that arrives.
    const flood = Array.from({ length: 500 }, (_, i) => ({
      type: "tool_progress" as const,
      id: "t1",
      label: `line ${i}\n`,
    }));
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "run_command", preview: "Run: cargo build" },
      ...flood,
    );
    const tool = entries[0] as Extract<Entry, { kind: "tool" }>;

    expect(tool.steps.length).toBeLessThanOrEqual(200);
    // The tail is what a running command is watched for.
    expect(tool.steps[tool.steps.length - 1]).toBe("line 499\n");
  });

  it("does not attach progress to a different call", () => {
    // Two delegations can run at once, and steps landing on the wrong card
    // would read as one agent doing all the work.
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "spawn_subagent", preview: "A" },
      { type: "tool_call_started", id: "t2", name: "spawn_subagent", preview: "B" },
      { type: "tool_progress", id: "t2", label: "only B did this" },
    );
    const [a, b] = entries as Extract<Entry, { kind: "tool" }>[];
    expect(a.steps).toEqual([]);
    expect(b.steps).toEqual(["only B did this"]);
  });

  it("leaves a resumed call untimed rather than inventing a duration", () => {
    // The transcript on disk records what happened, not when. A run of
    // replayed steps must show its shape without claiming a wall-clock time.
    const [tool] = entriesFromMessages([
      {
        role: "assistant",
        content: [{ type: "tool_use", id: "t1", name: "read_file", input: {} }],
      },
    ]) as Extract<Entry, { kind: "tool" }>[];
    expect(tool.startedAt).toBeUndefined();
    expect(tool.endedAt).toBeUndefined();
  });

  it("marks a failed tool call as an error", () => {
    const entries = run(
      { type: "tool_call_started", id: "t1", name: "read_file", preview: "Read x" },
      { type: "tool_call_finished", id: "t1", ok: false, output: "not found" },
    );
    expect(entries[0]).toMatchObject({ status: "error" });
  });

  it("completes the right call when several are in flight", () => {
    const entries = run(
      { type: "tool_call_started", id: "a", name: "glob", preview: "glob" },
      { type: "tool_call_started", id: "b", name: "grep", preview: "grep" },
      { type: "tool_call_finished", id: "b", ok: true, output: "hit" },
    );
    expect(entries.find((e) => e.id === "a")).toMatchObject({ status: "running" });
    expect(entries.find((e) => e.id === "b")).toMatchObject({ status: "ok" });
  });

  it("starts a new text entry after a tool call rather than reopening the old one", () => {
    const entries = run(
      text("first, let me look"),
      { type: "tool_call_started", id: "t1", name: "list_dir", preview: "list" },
      { type: "tool_call_finished", id: "t1", ok: true, output: "files" },
      text("here is what I found"),
    );
    const assistants = entries.filter((e) => e.kind === "assistant");
    expect(assistants).toHaveLength(2);
    expect(assistants[0]).toMatchObject({ text: "first, let me look" });
    expect(assistants[1]).toMatchObject({ text: "here is what I found" });
  });

  it("reports compaction to the user", () => {
    const entries = run({ type: "compacted", messages_removed: 12 });
    expect(entries[0]).toMatchObject({ kind: "notice", tone: "info" });
    expect((entries[0] as { text: string }).text).toContain("12");
  });

  it("reports trimming with what it recovered", () => {
    const entries = run({
      type: "context_trimmed",
      results: 6,
      tokens_saved: 12400,
    });
    expect(entries[0]).toMatchObject({ kind: "notice", tone: "info" });
    const text = (entries[0] as { text: string }).text;
    expect(text).toContain("6");
    expect(text).toContain("12,400");
  });

  it("surfaces errors as an error notice", () => {
    const entries = run({ type: "error", message: "provider unreachable" });
    expect(entries[0]).toMatchObject({
      kind: "notice",
      tone: "error",
      text: "provider unreachable",
    });
  });

  it("ignores events that carry no transcript content", () => {
    const entries = run(
      { type: "iteration_started", iteration: 1 },
      {
        type: "turn_finished",
        stop_reason: "end_turn",
        usage: { input_tokens: 10, output_tokens: 5 },
      },
    );
    expect(entries).toEqual([]);
  });

  it("handles a full multi-step turn in order", () => {
    const entries = run(
      { type: "iteration_started", iteration: 1 },
      text("Checking."),
      { type: "tool_call_started", id: "t1", name: "read_file", preview: "Read a" },
      { type: "tool_call_finished", id: "t1", ok: true, output: "body" },
      { type: "iteration_started", iteration: 2 },
      text("Done."),
      {
        type: "turn_finished",
        stop_reason: "end_turn",
        usage: { input_tokens: 1, output_tokens: 1 },
      },
    );
    expect(entries.map((e) => e.kind)).toEqual(["assistant", "tool", "assistant"]);
  });
});

describe("replaying a saved conversation", () => {
  const saved: Message[] = [
    { role: "user", content: [{ type: "text", text: "check the readme" }] },
    {
      role: "assistant",
      content: [
        { type: "thinking", text: "I should read it first." },
        { type: "text", text: "Checking." },
        {
          type: "tool_use",
          id: "t1",
          name: "read_file",
          input: { path: "README.md" },
        },
      ],
    },
    {
      role: "user",
      content: [
        { type: "tool_result", tool_use_id: "t1", content: "body", is_error: false },
      ],
    },
    { role: "assistant", content: [{ type: "text", text: "Done." }] },
  ];

  it("produces the same shape a streamed turn would have", () => {
    // A resumed conversation must be indistinguishable from a live one, or the
    // transcript visibly changes the moment you reopen it.
    const entries = entriesFromMessages(saved);
    expect(entries.map((e) => e.kind)).toEqual([
      "user",
      "assistant",
      "tool",
      "assistant",
    ]);
  });

  it("reunites a tool call with the result that answers it", () => {
    const entries = entriesFromMessages(saved);
    expect(entries[2]).toMatchObject({
      kind: "tool",
      name: "read_file",
      status: "ok",
      output: "body",
    });
  });

  it("marks a failed tool call as failed", () => {
    const entries = entriesFromMessages([
      saved[1],
      {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "t1", content: "nope", is_error: true },
        ],
      },
    ]);
    expect(entries.find((e) => e.kind === "tool")).toMatchObject({
      status: "error",
      output: "nope",
    });
  });

  it("leaves a call whose result never reached disk running", () => {
    // What a crash mid-turn looks like on reload; it must not read as success.
    const entries = entriesFromMessages([saved[0], saved[1]]);
    expect(entries.find((e) => e.kind === "tool")).toMatchObject({
      status: "running",
    });
  });

  it("keeps reasoning attached to the answer it belongs to", () => {
    const entries = entriesFromMessages(saved);
    expect(entries[1]).toMatchObject({
      kind: "assistant",
      thinking: "I should read it first.",
      text: "Checking.",
      open: false,
    });
  });

  it("closes every assistant entry so the next turn opens a new one", () => {
    const entries = entriesFromMessages(saved);
    const open = entries.filter((e) => e.kind === "assistant" && e.open);
    expect(open).toHaveLength(0);
  });

  it("replays an empty transcript as an empty view", () => {
    expect(entriesFromMessages([])).toEqual([]);
  });
});

describe("drawn tool results", () => {
  const TABLE = {
    title: "Crates by build time",
    caption: "cargo build --timings",
    columns: [{ label: "Crate", kind: "text" }],
    rows: [["taurus-core"]],
  };

  const started = (view?: UiEvent & { type: "tool_call_started" }): UiEvent => ({
    type: "tool_call_started",
    id: "t1",
    name: "show_table",
    preview: "Table: Crates by build time",
    view: { type: "table", ...TABLE } as never,
    ...view,
  });

  it("carries the view onto the entry the moment the call is announced", () => {
    // A question card that only appeared once its call finished would be
    // waiting on the answer it exists to ask for.
    const entries = run(started());
    expect(entries[0]).toMatchObject({ status: "running", view: { type: "table" } });
  });

  it("drops the view when the call turns out to have failed", () => {
    // The view goes out before the call runs, so a chart the harness then
    // refuses is already on screen — and a wrong chart beside the word
    // "failed" is still a wrong chart.
    const entries = run(
      started(),
      { type: "tool_call_finished", id: "t1", ok: false, output: "row 1 has 2 cells" },
    );
    expect(entries[0]).toMatchObject({ status: "error", view: undefined });
  });

  it("redraws a table from a reopened conversation", () => {
    // Only possible because the drawing tools take their view payload as
    // their input, unchanged. Nothing about the rendering is saved.
    const saved: Message[] = [
      {
        role: "assistant",
        content: [{ type: "tool_use", id: "t1", name: "show_table", input: TABLE }],
      },
      {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "t1", content: "Drew it.", is_error: false },
        ],
      },
    ];

    expect(entriesFromMessages(saved)[0]).toMatchObject({
      kind: "tool",
      status: "ok",
      view: { type: "table", title: "Crates by build time", rows: [["taurus-core"]] },
    });
  });

  it("keys a reopened question card to the call it belonged to", () => {
    const saved: Message[] = [
      {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "call-7",
            name: "ask_user",
            input: { questions: [{ prompt: "Which?", kind: "single", options: [], allow_other: false }] },
          },
        ],
      },
    ];

    expect(entriesFromMessages(saved)[0]).toMatchObject({
      view: { type: "questions", id: "call-7" },
    });
  });

  it("draws nothing for a saved call that failed", () => {
    const saved: Message[] = [
      {
        role: "assistant",
        content: [{ type: "tool_use", id: "t1", name: "show_table", input: TABLE }],
      },
      {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "t1", content: "refused", is_error: true },
        ],
      },
    ];

    expect(entriesFromMessages(saved)[0]).toMatchObject({ view: undefined });
  });

  it("ignores a payload an older build wrote in a shape this one cannot draw", () => {
    // A card that throws mid-render takes the whole transcript with it,
    // including the parts that were fine.
    expect(viewFromCall("t1", "show_table", { title: "t", columns: [] })).toBeUndefined();
    expect(viewFromCall("t1", "show_table", "not an object")).toBeUndefined();
    expect(viewFromCall("t1", "read_file", { path: "a.rs" })).toBeUndefined();
  });
});

describe("the plan supersedes itself", () => {
  const planEvent = (id: string, steps: unknown[]): UiEvent => ({
    type: "tool_call_started",
    id,
    name: "update_plan",
    preview: `Plan: ${steps.length} steps`,
    view: { type: "plan", steps } as never,
  });

  it("draws only the newest plan while a turn runs", () => {
    // The model rewrites the whole list every time a step starts or finishes,
    // so a six-step task ends with seven calls. Seven cards would turn the
    // checklist that says *where you are* into a history of where you have been.
    const entries = run(
      planEvent("p1", [{ text: "One", state: "active" }]),
      { type: "tool_call_finished", id: "p1", ok: true, output: "ok" },
      planEvent("p2", [
        { text: "One", state: "done" },
        { text: "Two", state: "active" },
      ]),
    );

    const drawn = entries.filter(
      (e) => e.kind === "tool" && e.view?.type === "plan",
    );
    expect(drawn).toHaveLength(1);
    expect(drawn[0]).toMatchObject({ id: "p2" });
  });

  it("keeps the superseded call's row rather than removing it", () => {
    // It happened, and the run header still counts it. Only the drawing stops.
    const entries = run(
      planEvent("p1", [{ text: "One", state: "active" }]),
      planEvent("p2", [{ text: "One", state: "done" }]),
    );

    const rows = entries.filter(
      (e) => e.kind === "tool" && e.name === "update_plan",
    );
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({ id: "p1", view: undefined });
  });

  it("leaves other views alone", () => {
    // A table and a chart are each a thing that happened once, so two of them
    // are two facts and both belong on screen.
    const entries = run(
      {
        type: "tool_call_started",
        id: "t1",
        name: "show_table",
        preview: "Table: A",
        view: {
          type: "table",
          title: "A",
          caption: null,
          columns: [{ label: "x", kind: "text" }],
          rows: [["1"]],
        } as never,
      },
      planEvent("p1", [{ text: "One", state: "active" }]),
      {
        type: "tool_call_started",
        id: "t2",
        name: "show_table",
        preview: "Table: B",
        view: {
          type: "table",
          title: "B",
          caption: null,
          columns: [{ label: "x", kind: "text" }],
          rows: [["2"]],
        } as never,
      },
    );

    const tables = entries.filter(
      (e) => e.kind === "tool" && e.view?.type === "table",
    );
    expect(tables).toHaveLength(2);
  });

  it("applies the same rule to a reopened conversation", () => {
    // A resumed transcript has every update in it at once, and must be
    // indistinguishable from one that was streamed.
    const entries = entriesFromMessages([
      {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "p1",
            name: "update_plan",
            input: { steps: [{ text: "One", state: "active" }] },
          },
        ],
      },
      {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "p1", content: "ok", is_error: false },
        ],
      },
      {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "p2",
            name: "update_plan",
            input: { steps: [{ text: "One", state: "done" }] },
          },
        ],
      },
    ]);

    const drawn = entries.filter(
      (e) => e.kind === "tool" && e.view?.type === "plan",
    );
    expect(drawn).toHaveLength(1);
    expect(drawn[0]).toMatchObject({ id: "p2" });
  });

  it("rebuilds a saved plan from the call that made it", () => {
    // Only possible because the tool takes its view payload as its input.
    expect(
      viewFromCall("p1", "update_plan", {
        steps: [{ text: "Read the parser", state: "done" }],
      }),
    ).toEqual({
      type: "plan",
      steps: [{ text: "Read the parser", state: "done" }],
    });
  });

  it("draws nothing for a saved call whose steps are missing", () => {
    // Written by whichever build was running at the time. A card that throws
    // mid-render takes the whole transcript with it.
    expect(viewFromCall("p1", "update_plan", { steps: "nope" })).toBeUndefined();
  });

  it("reads the field names the model actually used", () => {
    // The saved call is what the *model* typed, not what Rust made of it, and
    // Rust is forgiving about the names — `content`/`status` is a plan it
    // accepts. Read literally, that reopens as three rows with no text and a
    // bar stuck at zero, which is a plan card that looks broken.
    expect(
      viewFromCall("p1", "update_plan", {
        steps: [
          { content: "List the files", status: "completed" },
          { task: "Count the extensions", status: "in_progress" },
          { title: "Draw the chart" },
        ],
      }),
    ).toEqual({
      type: "plan",
      steps: [
        { text: "List the files", state: "done" },
        { text: "Count the extensions", state: "active" },
        // No state given is the same default the tool applies.
        { text: "Draw the chart", state: "todo" },
      ],
    });
  });

  it("keeps the running phrasing whatever the model called it", () => {
    // `activeForm` is the name Claude Code's checklist uses, and a model that
    // has seen that one reaches for it. It is the summary line's only source.
    expect(
      viewFromCall("p1", "update_plan", {
        steps: [
          { text: "Count the extensions", state: "active", activeForm: "Counting" },
        ],
      }),
    ).toEqual({
      type: "plan",
      steps: [
        { text: "Count the extensions", state: "active", active_form: "Counting" },
      ],
    });
  });

  it("reads a bare string as a step with nothing said about its state", () => {
    expect(
      viewFromCall("p1", "update_plan", { steps: ["Read the parser"] }),
    ).toEqual({
      type: "plan",
      steps: [{ text: "Read the parser", state: "todo" }],
    });
  });

  it("draws nothing when a saved step has no text under any name", () => {
    // A row with no text is not a step, and half a plan is worse than none:
    // the count and the bar would both be measuring a list that is missing an
    // item nobody can see.
    expect(
      viewFromCall("p1", "update_plan", {
        steps: [{ text: "Read the parser" }, { state: "todo" }],
      }),
    ).toBeUndefined();
  });

  it("reads a state it does not recognize as the default rather than giving up", () => {
    // It can only have come from a build whose alias list differed. One wrong
    // word on one row beats losing the card.
    expect(
      viewFromCall("p1", "update_plan", {
        steps: [{ text: "Read the parser", state: "blocked" }],
      }),
    ).toEqual({
      type: "plan",
      steps: [{ text: "Read the parser", state: "todo" }],
    });
  });
});

describe("the plan pinned above the composer", () => {
  const planEvent = (id: string, steps: unknown[]): UiEvent => ({
    type: "tool_call_started",
    id,
    name: "update_plan",
    preview: `Plan: ${steps.length} steps`,
    view: { type: "plan", steps } as never,
  });

  it("pins nothing when the conversation has no plan in it", () => {
    expect(pinnedPlan(run(text("no checklist here")))).toBeNull();
  });

  it("pins the newest plan, not the one the turn started with", () => {
    const entries = run(
      planEvent("p1", [{ text: "One", state: "active" }]),
      planEvent("p2", [
        { text: "One", state: "done" },
        { text: "Two", state: "active" },
      ]),
    );
    expect(pinnedPlan(entries)?.steps).toHaveLength(2);
  });

  it("keeps an unfinished plan up after the turn ends", () => {
    // The case the pinning exists for: work left undone, and twenty tool calls
    // between it and the bottom of the transcript.
    const entries = [
      ...run(
        planEvent("p1", [
          { text: "One", state: "done" },
          { text: "Two", state: "todo" },
        ]),
      ),
      { kind: "user" as const, id: "u1", text: "any news?" },
    ];
    expect(pinnedPlan(entries)).not.toBeNull();
  });

  it("keeps a finished plan up until something else is asked for", () => {
    // "Done" is a thing worth getting to see. Last hour's completed checklist
    // sitting over an unrelated question is not.
    const finished = run(
      planEvent("p1", [{ text: "One", state: "done" }]),
    );
    expect(pinnedPlan(finished)).not.toBeNull();

    expect(
      pinnedPlan([
        ...finished,
        { kind: "user", id: "u1", text: "something else entirely" },
      ]),
    ).toBeNull();
  });

  it("pins nothing for a plan with no steps in it", () => {
    // Legal on the wire, says nothing worth a panel, and every proportion
    // drawn from it would be a division by zero.
    expect(pinnedPlan(run(planEvent("p1", [])))).toBeNull();
  });

  it("finds the plan in a reopened conversation", () => {
    // Derived from the transcript rather than held beside it, so a resumed
    // session pins what it left off with and no extra bookkeeping is needed.
    const entries = entriesFromMessages([
      {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "p1",
            name: "update_plan",
            input: {
              steps: [
                { text: "One", state: "done" },
                { text: "Two", state: "active" },
              ],
            },
          },
        ],
      },
    ]);
    expect(pinnedPlan(entries)?.steps[1]).toMatchObject({ state: "active" });
  });
});

describe("images on a user message", () => {
  it("attaches a resumed message's images to the bubble that asked about them", () => {
    // Images precede the text in the message they belong to, so a resumed
    // conversation has to show the screenshot beside the question rather than
    // a question referring to one that is nowhere on screen.
    const entries = entriesFromMessages([
      {
        role: "user",
        content: [
          { type: "image", mime_type: "image/png", data: "AAAA" },
          { type: "text", text: "what is wrong with this?" },
        ],
      },
    ]);

    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      kind: "user",
      text: "what is wrong with this?",
      images: [{ mime_type: "image/png", data: "AAAA" }],
    });
  });

  it("keeps several images in the order they were sent", () => {
    const [entry] = entriesFromMessages([
      {
        role: "user",
        content: [
          { type: "image", mime_type: "image/png", data: "FIRST" },
          { type: "image", mime_type: "image/jpeg", data: "SECOND" },
          { type: "text", text: "compare these" },
        ],
      },
    ]) as Extract<Entry, { kind: "user" }>[];

    expect(entry.images?.map((i) => i.data)).toEqual(["FIRST", "SECOND"]);
  });

  it("leaves an ordinary message without an images field", () => {
    // Every turn takes this path, and an empty array on each one would be a
    // strip rendered for nothing.
    const [entry] = entriesFromMessages([
      { role: "user", content: [{ type: "text", text: "just a question" }] },
    ]) as Extract<Entry, { kind: "user" }>[];

    expect(entry.images).toBeUndefined();
  });

  it("does not repeat the images under a second text block", () => {
    // A user message has one text block, but a hand-written transcript could
    // have two, and repeating the strip would double the pictures.
    const entries = entriesFromMessages([
      {
        role: "user",
        content: [
          { type: "image", mime_type: "image/png", data: "AAAA" },
          { type: "text", text: "first" },
          { type: "text", text: "second" },
        ],
      },
    ]) as Extract<Entry, { kind: "user" }>[];

    expect(entries[0].images).toHaveLength(1);
    expect(entries[1].images).toBeUndefined();
  });
});

describe("redrawing a sequence diagram from a saved call", () => {
  const call = (input: unknown) => viewFromCall("t1", "show_sequence", input);

  it("rebuilds the lanes and arrows the model sent", () => {
    const view = call({
      title: "Placing an order",
      participants: ["Client", "API"],
      messages: [
        { from: "Client", to: "API", text: "POST /orders" },
        { from: "API", to: "Client", text: "201", kind: "return" },
      ],
    });
    expect(view).toMatchObject({
      type: "sequence",
      title: "Placing an order",
      participants: ["Client", "API"],
    });
    // An omitted kind is a call, matching the default Rust applies on the way
    // in. Read as anything else, a replayed diagram would have no solid arrows.
    expect(view).toMatchObject({
      messages: [
        { from: "Client", to: "API", text: "POST /orders", kind: "call" },
        { from: "API", to: "Client", text: "201", kind: "return" },
      ],
    });
  });

  it("drops an arrow naming a lane the call never declared", () => {
    // The tool refuses these, so a transcript holding one came from a build
    // whose rules differed. One arrow short beats a blank where the answer was.
    const view = call({
      title: "Placing an order",
      participants: ["Client", "API"],
      messages: [
        { from: "Client", to: "Ghost", text: "nowhere" },
        { from: "Client", to: "API", text: "POST /orders" },
      ],
    });
    expect(view).toMatchObject({ messages: [{ text: "POST /orders" }] });
  });

  it("refuses a payload with no participants rather than drawing an empty box", () => {
    expect(call({ title: "Nothing", participants: [], messages: [] })).toBeUndefined();
    expect(call({ title: "Nothing", messages: [] })).toBeUndefined();
  });
});

describe("redrawing a flow diagram from a saved call", () => {
  const call = (input: unknown) => viewFromCall("t1", "show_flow", input);

  it("rebuilds the stages, nodes and edges the model sent", () => {
    const view = call({
      title: "Request path",
      stages: [
        { name: "Edge", nodes: [{ label: "Client" }] },
        { nodes: [{ label: "API", note: "axum" }] },
      ],
      edges: [{ from: "Client", to: "API", label: "POST" }],
    });
    expect(view).toMatchObject({
      type: "flow",
      title: "Request path",
      stages: [
        { name: "Edge", nodes: [{ label: "Client", note: null }] },
        { name: null, nodes: [{ label: "API", note: "axum" }] },
      ],
      edges: [{ from: "Client", to: "API", label: "POST" }],
    });
  });

  it("drops an edge naming a node the call never declared", () => {
    const view = call({
      title: "Request path",
      stages: [{ nodes: [{ label: "Client" }] }, { nodes: [{ label: "API" }] }],
      edges: [
        { from: "Client", to: "Ghost" },
        { from: "Client", to: "API" },
      ],
    });
    expect(view).toMatchObject({ edges: [{ from: "Client", to: "API" }] });
  });

  it("refuses the card when a stage lost a node rather than drawing a gap", () => {
    // A missing box takes every arrow into it with it, so what would be drawn
    // is a diagram that is quietly wrong about the shape — worse than none.
    expect(
      call({
        title: "Request path",
        stages: [{ nodes: [{ label: "Client" }, { note: "no label" }] }],
        edges: [{ from: "Client", to: "Client" }],
      }),
    ).toBeUndefined();
  });

  it("refuses a payload with no stages", () => {
    expect(call({ title: "Nothing", stages: [], edges: [] })).toBeUndefined();
    expect(call({ title: "Nothing", edges: [] })).toBeUndefined();
  });
});

describe("a delegation's own transcript", () => {
  const started: UiEvent = {
    type: "tool_call_started",
    id: "d1",
    name: "spawn_subagent",
    preview: "Delegate to explorer: find the parser",
  };

  it("is attached to the call it belongs to", () => {
    const entries = run(started, {
      type: "tool_transcript",
      id: "d1",
      session: "child1",
      agent: "explorer",
    });
    expect(entries[0]).toMatchObject({
      kind: "tool",
      transcript: { session: "child1", agent: "explorer" },
    });
  });

  it("ignores a reference to a call that is not there", () => {
    // Events can outlive the entry they name — a cleared transcript, a resumed
    // conversation — and an unknown id must be nothing rather than a new row.
    const entries = run(started, {
      type: "tool_transcript",
      id: "gone",
      session: "child1",
      agent: "explorer",
    });
    expect(entries).toHaveLength(1);
    expect(entries[0]).not.toHaveProperty("transcript");
  });
});

/**
 * A turn arrives a token at a time and every one of them used to be a render
 * of the whole app. Batching is only safe if it changes when the screen catches
 * up and nothing else — so what these check is that the events come out in the
 * order they went in, and that none of them can be left behind.
 */
describe("batching stream events", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("holds events for a frame and delivers them in order", () => {
    const batches: UiEvent[][] = [];
    const stream = batchEvents((events) => batches.push(events));

    stream.push(text("Hel"));
    stream.push(text("lo"));
    expect(batches).toEqual([]);

    vi.runAllTimers();
    expect(batches).toHaveLength(1);
    expect(run(...batches[0])).toEqual([
      expect.objectContaining({ kind: "assistant", text: "Hello" }),
    ]);
  });

  it("keeps a tool call in its place among the text around it", () => {
    // The ordering that matters: a batch is only sound if a call that arrived
    // between two sentences still lands between them.
    const batches: UiEvent[][] = [];
    const stream = batchEvents((events) => batches.push(events));

    stream.push(text("before "));
    stream.push({
      type: "tool_call_started",
      id: "t1",
      name: "read_file",
      preview: "src/main.rs",
    });
    stream.push(text("after"));
    vi.runAllTimers();

    const entries = run(...batches[0]);
    expect(entries.map((e) => e.kind)).toEqual([
      "assistant",
      "tool",
      "assistant",
    ]);
  });

  it("delivers what is waiting when the turn ends", () => {
    // The last few tokens of a turn have nothing behind them to trigger a
    // frame, so without this they would sit in the queue until the next turn.
    const batches: UiEvent[][] = [];
    const stream = batchEvents((events) => batches.push(events));

    stream.push(text("the last word"));
    stream.flush();

    expect(batches).toHaveLength(1);
    expect(run(...batches[0])).toEqual([
      expect.objectContaining({ text: "the last word" }),
    ]);
  });

  it("does nothing when there is nothing waiting", () => {
    // `send` flushes on the way out of both the success and the failure path,
    // so the second one has to be free.
    const batches: UiEvent[][] = [];
    const stream = batchEvents((events) => batches.push(events));

    stream.push(text("done"));
    stream.flush();
    stream.flush();
    vi.runAllTimers();

    expect(batches).toHaveLength(1);
  });

  it("starts a new frame after one has been flushed", () => {
    const batches: UiEvent[][] = [];
    const stream = batchEvents((events) => batches.push(events));

    stream.push(text("one"));
    stream.flush();
    stream.push(text("two"));
    vi.runAllTimers();

    expect(batches.map((b) => b.length)).toEqual([1, 1]);
  });
});

describe("live file changes", () => {
  const changed = (...paths: string[]): UiEvent => ({
    type: "files_changed",
    paths,
  });

  it("unions what a turn reports into what the conversation already changed", () => {
    // The report covers the running turn; the set on screen covers the whole
    // conversation, including turns restored from checkpoints on reopening.
    const before = ["docs/old.md"];
    const after = [changed("a.rs"), changed("a.rs", "b.rs")].reduce(
      mergeChanged,
      before,
    );
    expect(after).toEqual(["a.rs", "b.rs", "docs/old.md"]);
  });

  it("hands back the same array when a report adds nothing", () => {
    // Identity, not just equality: the header reads this on every frame of a
    // turn, and a fresh array would redraw it to say the same number.
    const before = ["a.rs"];
    expect(mergeChanged(before, changed("a.rs"))).toBe(before);
    expect(mergeChanged(before, { type: "iteration_started", iteration: 2 })).toBe(
      before,
    );
  });

  it("leaves the transcript alone", () => {
    // The changed set is the state of the workspace, not something that
    // happened in the conversation.
    expect(run(changed("a.rs"))).toEqual([]);
  });
});

describe("pushed conversation entries", () => {
  const meta = (over: Partial<SessionMeta> = {}): SessionMeta => ({
    id: "s1",
    workspace: "/w",
    model: "test-model",
    started: 1,
    updated: 100,
    title: "a question",
    ...over,
  });
  const status = (workspace = "/w") => ({ workspace }) as AppStatus;

  it("puts a conversation it has not seen at the front", () => {
    const list = mergeSession([], meta(), status());
    expect(list.map((s) => s.id)).toEqual(["s1"]);
  });

  it("replaces the entry it already had rather than doubling it", () => {
    const list = mergeSession(
      [meta({ title: "a question", updated: 100 })],
      meta({ title: "Renamed", updated: 100 }),
      status(),
    );
    expect(list).toHaveLength(1);
    expect(list[0].title).toBe("Renamed");
  });

  it("keeps the list newest first however the entry arrives", () => {
    const list = mergeSession(
      [meta({ id: "s2", updated: 300 }), meta({ id: "s3", updated: 50 })],
      meta({ id: "s1", updated: 200 }),
      status(),
    );
    expect(list.map((s) => s.id)).toEqual(["s2", "s1", "s3"]);
  });

  it("ignores a conversation belonging to another workspace", () => {
    // A turn still finishing when the window moved folders must not put its
    // conversation into the new folder's rail.
    const existing = [meta({ id: "s2" })];
    expect(
      mergeSession(existing, meta({ id: "s1", workspace: "/elsewhere" }), status()),
    ).toBe(existing);
  });

  it("accepts anything before a workspace is known", () => {
    // Startup, where there is nothing yet to disagree with.
    expect(mergeSession([], meta(), null)).toHaveLength(1);
  });
});

describe("a conversation that changed model", () => {
  const said = (role: "user" | "assistant", text: string): Message => ({
    role,
    content: [{ type: "text", text }],
  });
  const moved = (after: number, model: string): Switch => ({
    after,
    provider: "anthropic",
    model,
    at: 1_700_000_000,
  });

  const rules = (entries: Entry[]) =>
    entries
      .filter((e) => e.kind === "notice")
      .map((e) => (e as Extract<Entry, { kind: "notice" }>).rule?.note);

  it("draws the change where it happened, not at the end", () => {
    // The reason to want the line is to explain the answers after it. At the
    // bottom it would explain nothing.
    const entries = entriesFromMessages(
      [
        said("user", "first question"),
        said("assistant", "first answer"),
        said("user", "second question"),
        said("assistant", "second answer"),
      ],
      [moved(2, "claude-opus-5")],
    );
    expect(entries.map((e) => e.kind)).toEqual([
      "user",
      "assistant",
      "notice",
      "user",
      "assistant",
    ]);
  });

  it("names what it moved to, with the backend serving it", () => {
    const entries = entriesFromMessages([said("user", "hi")], [moved(1, "claude-opus-5")]);
    expect(rules(entries)).toEqual(["anthropic · claude-opus-5"]);
  });

  it("draws both when it moved twice with nothing asked in between", () => {
    // Two clicks of the picker. Neither is a lie about what happened, and
    // collapsing them would hide a backend the conversation passed through.
    const entries = entriesFromMessages(
      [said("user", "hi"), said("assistant", "hello")],
      [moved(0, "claude-opus-5"), moved(0, "qwen3.6:27b")],
    );
    expect(rules(entries)).toEqual([
      "anthropic · claude-opus-5",
      "anthropic · qwen3.6:27b",
    ]);
    expect(entries[0].kind).toBe("notice");
    expect(entries[1].kind).toBe("notice");
  });

  it("still draws a change made after the last turn", () => {
    // Moved and then closed without asking anything since. Dropped, the line
    // would reappear from nowhere the next time a question was asked.
    const entries = entriesFromMessages(
      [said("user", "hi"), said("assistant", "hello")],
      [moved(2, "claude-opus-5")],
    );
    expect(entries[entries.length - 1].kind).toBe("notice");
  });

  it("draws nothing for a conversation that never moved", () => {
    const entries = entriesFromMessages([said("user", "hi")]);
    expect(entries.every((e) => e.kind !== "notice")).toBe(true);
  });
});
