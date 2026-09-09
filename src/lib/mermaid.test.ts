import { describe, expect, it } from "vitest";

import { read, type Read } from "./mermaid";
import * as flow from "./flow";
import * as sequence from "./sequence";
import type { TranscriptView } from "./api";

type FlowView = Extract<TranscriptView, { type: "flow" }>;
type SequenceView = Extract<TranscriptView, { type: "sequence" }>;

/** The flow arm, or a failure naming what came back instead. */
function asFlow(source: string) {
  const got = read(source);
  if (got.kind !== "flow") throw new Error(`expected a flow, got ${describeRead(got)}`);
  return got;
}

function asSequence(source: string) {
  const got = read(source);
  if (got.kind !== "sequence") {
    throw new Error(`expected a sequence, got ${describeRead(got)}`);
  }
  return got;
}

function describeRead(got: Read): string {
  return got.kind === "refused" ? `refused: ${got.why}` : got.kind;
}

/** Stage by stage, as labels — the shape most of these cases are about. */
function columns(input: flow.FlowInput): string[][] {
  return input.stages.map((stage) => stage.nodes.map((node) => node.label));
}

function arrows(input: flow.FlowInput): string[] {
  const label = new Map(
    input.stages.flatMap((s) => s.nodes.map((n) => [n.key, n.label] as const)),
  );
  return input.edges.map(
    (e) =>
      `${label.get(e.from)} -> ${label.get(e.to)}${e.label ? ` (${e.label})` : ""}`,
  );
}

describe("reading a flowchart's header", () => {
  it("takes flowchart and graph, in any direction", () => {
    for (const header of [
      "flowchart LR",
      "flowchart TD",
      "graph TB",
      "graph RL",
      "graph",
      "flowchart-elk LR",
    ]) {
      expect(read(`${header}\nA --> B`).kind).toBe("flow");
    }
  });

  it("says when it has re-oriented the picture rather than hiding it", () => {
    // Direction in Mermaid is presentational, so nothing is lost — but a person
    // who wrote TD and got a row of boxes deserves to be told why.
    expect(asFlow("graph TD\nA --> B").skipped).toEqual([
      "Laid out left to right rather than top to bottom",
    ]);
    expect(asFlow("graph LR\nA --> B").skipped).toEqual([]);
  });

  it("names the diagram types it does not draw", () => {
    const got = read("classDiagram\n  Animal <|-- Duck");
    expect(got.kind).toBe("refused");
    expect(got.kind === "refused" && got.why).toContain("class diagrams");
  });

  it("echoes a keyword it has never heard of", () => {
    const got = read("supernovaDiagram\n  a --> b");
    expect(got.kind === "refused" && got.why).toContain("`supernovaDiagram`");
  });

  it("refuses an empty fence and one with no nodes", () => {
    expect(read("   \n\n").kind).toBe("refused");
    expect(read("flowchart LR\n%% nothing here").kind).toBe("refused");
  });
});

describe("reading a flowchart's nodes", () => {
  it("reads the text out of every shape, and draws them all as boxes", () => {
    // A diamond for a decision is a real loss and it is written down as one.
    // Reading the text out is not optional: `A{Is it cached?}` with the shape
    // unparsed is a box labelled `A`.
    const source = [
      "flowchart LR",
      "  a[square] --> b(round)",
      "  b --> c([stadium])",
      "  c --> d[[subroutine]]",
      "  d --> e[(cylinder)]",
      "  e --> f((circle))",
      "  f --> g{diamond}",
      "  g --> h{{hexagon}}",
      "  h --> i[/parallelogram/]",
      "  i --> j>flag]",
    ].join("\n");
    const labels = asFlow(source).input.stages.flatMap((s) =>
      s.nodes.map((n) => n.label),
    );
    expect(labels).toEqual([
      "square",
      "round",
      "stadium",
      "subroutine",
      "cylinder",
      "circle",
      "diamond",
      "hexagon",
      "parallelogram",
      "flag",
    ]);
  });

  it("keeps a bracket that is inside quotes", () => {
    const { input } = asFlow('flowchart LR\n  a["read(0..n]"] --> b');
    expect(input.stages[0].nodes[0].label).toBe("read(0..n]");
  });

  it("takes the id as the label when there is no text", () => {
    const { input } = asFlow("flowchart LR\n  Client --> API");
    expect(columns(input)).toEqual([["Client"], ["API"]]);
  });

  it("lets a later mention supply a label the first one lacked", () => {
    // `A --> B` and then `B[Store]` further down is how people write these.
    const { input } = asFlow("flowchart LR\n  a --> b\n  b[Store]");
    expect(columns(input)).toEqual([["a"], ["Store"]]);
  });

  it("keeps two nodes that read the same apart", () => {
    // The engine change this needed: `show_flow` keys a box by its label, and
    // two ids with one label would have collapsed into a single box.
    const { input } = asFlow('flowchart LR\n  a["User"] --> b["User"]');
    expect(columns(input)).toEqual([["User"], ["User"]]);
    expect(flow.plan(input).boxes).toHaveLength(2);
  });

  it("reads an id with a hyphen or a dot in it", () => {
    const { input } = asFlow("flowchart LR\n  user-service --> order.store");
    expect(columns(input)).toEqual([["user-service"], ["order.store"]]);
  });

  it("splits a node's second line off at its line break", () => {
    // What closes the round trip: `show_flow` gives a node a quieter second
    // line, and the emitter writes it as `Label<br/>note`.
    const { input } = asFlow('flowchart LR\n  a["API<br/>axum"] --> b');
    expect(input.stages[0].nodes[0]).toEqual({
      key: "a",
      label: "API",
      note: "axum",
    });
  });
});

describe("reading a flowchart's links", () => {
  it("takes every arrow people write", () => {
    for (const link of ["-->", "---", "--->", "==>", "===", "-.->", "-.-", "--o", "--x"]) {
      const { input } = asFlow(`flowchart LR\n  a ${link} b`);
      expect(arrows(input), link).toEqual(["a -> b"]);
    }
  });

  it("takes a label after the arrow and inside it", () => {
    expect(arrows(asFlow("flowchart LR\n  a -->|yes| b").input)).toEqual([
      "a -> b (yes)",
    ]);
    expect(arrows(asFlow("flowchart LR\n  a -- yes --> b").input)).toEqual([
      "a -> b (yes)",
    ]);
    expect(arrows(asFlow("flowchart LR\n  a -. later .-> b").input)).toEqual([
      "a -> b (later)",
    ]);
    expect(arrows(asFlow("flowchart LR\n  a == fast ==> b").input)).toEqual([
      "a -> b (fast)",
    ]);
  });

  it("does not read the arrowless middle of a plain link as a label", () => {
    // `-- text -->` is tried first, and `--> b` also matches it with `b` left
    // over. Getting this wrong refuses ordinary diagrams.
    expect(arrows(asFlow("flowchart LR\n  a --> b").input)).toEqual(["a -> b"]);
    expect(arrows(asFlow("flowchart LR\n  a --- b").input)).toEqual(["a -> b"]);
  });

  it("follows a chain", () => {
    const { input } = asFlow("flowchart LR\n  a --> b -->|then| c");
    expect(arrows(input)).toEqual(["a -> b", "b -> c (then)"]);
    expect(columns(input)).toEqual([["a"], ["b"], ["c"]]);
  });

  it("fans an ampersand out on both sides", () => {
    const { input } = asFlow("flowchart LR\n  a & b --> c & d");
    expect(arrows(input)).toEqual([
      "a -> c",
      "a -> d",
      "b -> c",
      "b -> d",
    ]);
  });

  it("makes a two-way arrow two arrows", () => {
    const { input } = asFlow("flowchart LR\n  a <--> b");
    expect(arrows(input)).toEqual(["a -> b", "b -> a"]);
  });

  it("keeps a self-edge, which the engine draws as a ring", () => {
    const { input } = asFlow("flowchart LR\n  a --> a\n  a --> b");
    expect(arrows(input)).toEqual(["a -> a", "a -> b"]);
    expect(columns(input)).toEqual([["a"], ["b"]]);
  });

  it("refuses a line it cannot read, and says which line", () => {
    // Rather than dropping the arrow: a diagram missing a connection nobody was
    // told about is worse than one that says it could not be read.
    const got = read("flowchart LR\n  a --> b\n  a ~~~> b");
    expect(got.kind).toBe("refused");
    expect(got.kind === "refused" && got.why).toContain("Line 3");
    expect(got.kind === "refused" && got.why).toContain("a ~~~> b");
  });
});

describe("working out which column a node goes in", () => {
  it("puts a node one past the furthest thing that reaches it", () => {
    // Longest path, not shortest: `a --> d` must not pull `d` back to column 1
    // and leave the arrow from `c` pointing backwards.
    const { input } = asFlow("flowchart LR\n  a --> b --> c --> d\n  a --> d");
    expect(columns(input)).toEqual([["a"], ["b"], ["c"], ["d"]]);
  });

  it("rejoins the two halves of a diamond", () => {
    const { input } = asFlow(
      "flowchart LR\n  a --> b\n  a --> c\n  b --> d\n  c --> d",
    );
    expect(columns(input)).toEqual([["a"], ["b", "c"], ["d"]]);
  });

  it("lays out a cycle by leaving the edge that closes it out of the layering", () => {
    // A retry loop is the most ordinary shape a flowchart has. Layered with the
    // loop counted, there is no first column at all.
    const { input } = asFlow(
      "flowchart LR\n  a --> b\n  b --> c\n  c -->|retry| a",
    );
    expect(columns(input)).toEqual([["a"], ["b"], ["c"]]);
    // And the engine draws it as the backward loop it is.
    const layout = flow.plan(input);
    expect(layout.arrows[2].back).toBe(true);
  });

  it("is stable in declaration order rather than by however a map iterated", () => {
    const source = "flowchart LR\n  b --> a\n  a --> b";
    expect(columns(asFlow(source).input)).toEqual(columns(asFlow(source).input));
    expect(columns(asFlow(source).input)).toEqual([["b"], ["a"]]);
  });

  it("starts every disconnected part at the left", () => {
    const { input } = asFlow("flowchart LR\n  a --> b\n  c --> d");
    expect(columns(input)).toEqual([
      ["a", "c"],
      ["b", "d"],
    ]);
  });
});

describe("reading a flowchart's subgraphs", () => {
  it("makes them the stages when every node is in one", () => {
    // Which is exactly what this app emits: `show_flow`'s named stages become
    // subgraphs, and that is what carries the layering across.
    const { input, skipped } = asFlow(
      [
        "flowchart LR",
        "  subgraph Edge",
        "    a[Client]",
        "  end",
        "  subgraph Service",
        "    b[API]",
        "    c[Worker]",
        "  end",
        "  a --> b",
        "  a --> c",
      ].join("\n"),
    );
    expect(input.stages.map((s) => s.name)).toEqual(["Edge", "Service"]);
    expect(columns(input)).toEqual([["Client"], ["API", "Worker"]]);
    expect(skipped).toEqual([]);
  });

  it("reads a title out of the bracketed form and drops its id", () => {
    const { input } = asFlow(
      'flowchart LR\n subgraph s0[Edge]\n a\n end\n subgraph s1["The Service"]\n b\n end\n a --> b',
    );
    expect(input.stages.map((s) => s.name)).toEqual(["Edge", "The Service"]);
  });

  it("gives a run of nodes outside them a column of its own", () => {
    // Which is the case this app's own output is: `mermaid()` writes a named
    // stage as a subgraph and an unnamed one as bare nodes after it, so a
    // diagram with a name on some stages and not others has both.
    const { input, skipped } = asFlow(
      "flowchart LR\n subgraph Edge\n a\n end\n b\n a --> b",
    );
    expect(input.stages.map((s) => s.name)).toEqual(["Edge", null]);
    expect(columns(input)).toEqual([["a"], ["b"]]);
    expect(skipped).toEqual([]);
  });

  it("opens a new column for each run, not one shared by all of them", () => {
    const { input } = asFlow(
      [
        "flowchart LR",
        " a",
        " subgraph Middle",
        "  b",
        " end",
        " c",
        " a --> b --> c",
      ].join("\n"),
    );
    expect(input.stages.map((s) => s.name)).toEqual([null, "Middle", null]);
    expect(columns(input)).toEqual([["a"], ["b"], ["c"]]);
  });

  it("flattens them and says so when one is inside another", () => {
    const { skipped } = asFlow(
      [
        "flowchart LR",
        " subgraph Outer",
        "  subgraph Inner",
        "   a",
        "  end",
        "  b",
        " end",
        " a --> b",
      ].join("\n"),
    );
    expect(skipped[0]).toContain("2 subgraphs flattened");
    expect(skipped[0]).toContain("inside another");
  });
});

describe("what a flowchart may carry that is not the diagram", () => {
  it("reads a title out of the frontmatter", () => {
    const { input } = asFlow(
      "---\ntitle: How an order lands\n---\nflowchart LR\n a --> b",
    );
    expect(input.title).toBe("How an order lands");
  });

  it("calls it a diagram when nothing named it", () => {
    expect(asFlow("flowchart LR\n a --> b").input.title).toBe("Diagram");
  });

  it("ignores comments, init directives, styling and direction", () => {
    const { input, skipped } = asFlow(
      [
        "%%{init: {'theme':'dark'}}%%",
        "flowchart LR",
        "  %% the happy path",
        "  direction LR",
        "  a --> b %% and back again",
        "  style a fill:#f9f",
        "  classDef big font-size:20px",
        "  class a big",
        "  linkStyle 0 stroke:red",
        "  click a href 'https://example.com'",
      ].join("\n"),
    );
    expect(arrows(input)).toEqual(["a -> b"]);
    // Styling carries nothing a reader of the picture would miss, so it is not
    // reported — unlike a subgraph, which is content.
    expect(skipped).toEqual([]);
  });

  it("treats a semicolon as the end of a statement, but not inside a label", () => {
    const { input } = asFlow('flowchart LR;a["Wait; then retry"] --> b;b --> c');
    expect(columns(input)).toEqual([["Wait; then retry"], ["b"], ["c"]]);
  });
});

describe("reading a sequence diagram", () => {
  it("takes participants, actors and aliases", () => {
    const { input } = asSequence(
      [
        "sequenceDiagram",
        "  participant c as Client",
        "  actor u as User",
        "  participant API",
        "  c ->> API: POST /orders",
      ].join("\n"),
    );
    expect(input.participants).toEqual([
      { key: "c", label: "Client" },
      { key: "u", label: "User" },
      { key: "API", label: "API" },
    ]);
  });

  it("declares a lane the first time a message mentions it", () => {
    const { input } = asSequence("sequenceDiagram\n a->>b: hi\n c->>a: later");
    expect(input.participants.map((p) => p.label)).toEqual(["a", "b", "c"]);
  });

  it("reads a dashed arrow as a return and a solid one as a call", () => {
    const { input } = asSequence(
      [
        "sequenceDiagram",
        "  a->>b: call",
        "  b-->>a: return",
        "  a->b: plain call",
        "  b-->a: plain return",
        "  a-)b: async",
        "  b--xa: failed",
      ].join("\n"),
    );
    expect(input.messages.map((m) => `${m.text}:${m.kind}`)).toEqual([
      "call:call",
      "return:return",
      "plain call:call",
      "plain return:return",
      "async:call",
      "failed:return",
    ]);
  });

  it("keeps a self-call, which the engine already drew", () => {
    const { input } = asSequence("sequenceDiagram\n a->>a: validate the body");
    expect(sequence.plan(input).rows[0].self).toBe(true);
  });

  it("does not read an arrow out of a message's own text", () => {
    const { input } = asSequence("sequenceDiagram\n a->>b: use --> for edges");
    expect(input.messages[0]).toEqual({
      from: "a",
      to: "b",
      text: "use --> for edges",
      kind: "call",
    });
  });

  it("ignores activation, which these diagrams never drew", () => {
    // `MessageKind` argues at its own definition why a picture somebody reads
    // once carries two kinds of arrow and not four.
    const { input, skipped } = asSequence(
      "sequenceDiagram\n a->>+b: call\n b-->>-a: return\n activate b\n deactivate b",
    );
    expect(input.messages.map((m) => m.to)).toEqual(["b", "a"]);
    expect(skipped).toEqual([]);
  });

  it("draws what it can and says what it left out", () => {
    // Refusing the whole picture over the frame around it would be worse for
    // the reader than drawing it and saying the frame is missing.
    const { input, skipped } = asSequence(
      [
        "sequenceDiagram",
        "  autonumber",
        "  Note over a,b: the interesting part",
        "  loop every minute",
        "    a->>b: poll",
        "  end",
        "  alt found",
        "    b-->>a: the row",
        "  else missing",
        "    b-->>a: nothing",
        "  end",
      ].join("\n"),
    );
    expect(input.messages).toHaveLength(3);
    // One clause, because four sentences each saying "not drawn" is the same
    // sentence four times.
    expect(skipped).toEqual([
      "1 note, 1 loop block, 1 alt block and 1 else block not drawn",
    ]);
  });

  it("refuses a line it cannot read, and says which line", () => {
    const got = read("sequenceDiagram\n a->>b: fine\n Order Store->>API: spaces");
    expect(got.kind).toBe("refused");
    expect(got.kind === "refused" && got.why).toContain("Line 3");
  });
});

describe("the round trip through the app's own Mermaid", () => {
  // The load-bearing test of the whole file. Both diagram cards have carried a
  // copy-as-Mermaid button since they shipped, so the app has been emitting
  // Mermaid all along — a note that can read it back has to read *that* first.
  const flowView: FlowView = {
    type: "flow",
    title: "How a request reaches the database",
    caption: null,
    stages: [
      { name: "Edge", nodes: [{ label: "Client", note: null }] },
      {
        name: "Service",
        nodes: [
          { label: "API", note: "axum" },
          { label: "Worker", note: null },
        ],
      },
      { name: null, nodes: [{ label: "Order Store", note: "postgres" }] },
    ],
    edges: [
      { from: "Client", to: "API", label: "POST /orders" },
      { from: "API", to: "Order Store", label: "insert" },
      { from: "Worker", to: "API", label: "retry" },
      { from: "Order Store", to: "Client", label: "cached" },
      { from: "Worker", to: "Worker", label: "poll" },
    ],
  };

  it("gives back the same flow diagram it wrote", () => {
    const written = flow.mermaid(flow.fromView(flowView));
    const { input, skipped } = asFlow(written);

    expect(skipped).toEqual([]);
    // The stages survive, which is the one thing the payload knows and the one
    // thing Mermaid would otherwise re-infer.
    expect(input.stages.map((s) => s.name)).toEqual(["Edge", "Service", null]);
    expect(columns(input)).toEqual([
      ["Client"],
      ["API", "Worker"],
      ["Order Store"],
    ]);
    // Notes survive their `<br/>`, a label with a space survives its alias, and
    // every arrow keeps its own label and direction.
    expect(input.stages[1].nodes[0].note).toBe("axum");
    expect(arrows(input)).toEqual([
      "Client -> API (POST /orders)",
      "API -> Order Store (insert)",
      "Worker -> API (retry)",
      "Order Store -> Client (cached)",
      "Worker -> Worker (poll)",
    ]);
  });

  it("lays the re-read flow out exactly as the card did", () => {
    // Same boxes in the same places, which is the only way to know the two
    // halves of the round trip agree about the diagram and not just about the text.
    const original = flow.plan(flow.fromView(flowView));
    const reread = flow.plan(asFlow(flow.mermaid(flow.fromView(flowView))).input);
    expect(reread.boxes.map((b) => [b.label, b.x, b.y])).toEqual(
      original.boxes.map((b) => [b.label, b.x, b.y]),
    );
    expect(reread.arrows.map((a) => a.path)).toEqual(
      original.arrows.map((a) => a.path),
    );
  });

  it("survives a second trip out and back", () => {
    const once = asFlow(flow.mermaid(flow.fromView(flowView))).input;
    const twice = asFlow(flow.mermaid(once)).input;
    expect(columns(twice)).toEqual(columns(once));
    expect(arrows(twice)).toEqual(arrows(once));
  });

  const sequenceView: SequenceView = {
    type: "sequence",
    title: "Placing an order",
    caption: null,
    participants: ["Client", "API", "Order Store"],
    messages: [
      { from: "Client", to: "API", text: "POST /orders", kind: "call" },
      { from: "API", to: "API", text: "validate the body", kind: "call" },
      { from: "API", to: "Order Store", text: "insert row", kind: "call" },
      { from: "Order Store", to: "API", text: "ok", kind: "return" },
      { from: "API", to: "Client", text: "201 Created", kind: "return" },
    ],
  };

  it("gives back the same sequence diagram it wrote", () => {
    const written = sequence.mermaid(sequence.fromView(sequenceView));
    const { input, skipped } = asSequence(written);

    expect(skipped).toEqual([]);
    // The alias is what makes `Order Store` writable at all, and reading it
    // back is what makes the lane's name survive.
    expect(input.participants.map((p) => p.label)).toEqual([
      "Client",
      "API",
      "Order Store",
    ]);
    expect(
      input.messages.map((m) => `${label(input, m.from)}->${label(input, m.to)}:${m.kind}`),
    ).toEqual([
      "Client->API:call",
      "API->API:call",
      "API->Order Store:call",
      "Order Store->API:return",
      "API->Client:return",
    ]);
  });

  it("lays the re-read sequence out exactly as the card did", () => {
    const original = sequence.plan(sequence.fromView(sequenceView));
    const reread = sequence.plan(
      asSequence(sequence.mermaid(sequence.fromView(sequenceView))).input,
    );
    expect(reread).toEqual(original);
  });
});

function label(input: sequence.SequenceInput, key: string): string {
  return input.participants.find((p) => p.key === key)?.label ?? key;
}
