import { describe, expect, it } from "vitest";

import { described, layout, mermaid } from "./FlowCard";
import type { TranscriptView } from "../lib/api";

type FlowView = Extract<TranscriptView, { type: "flow" }>;

const view = (patch: Partial<FlowView> = {}): FlowView => ({
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
    { name: null, nodes: [{ label: "Postgres", note: null }] },
  ],
  edges: [
    { from: "Client", to: "API", label: "POST /orders" },
    { from: "API", to: "Postgres", label: "insert" },
  ],
  ...patch,
});

describe("laying out a flow diagram", () => {
  it("puts each stage in its own column, left to right", () => {
    const { boxes } = layout(view());
    const client = boxes.find((b) => b.label === "Client")!;
    const api = boxes.find((b) => b.label === "API")!;
    const postgres = boxes.find((b) => b.label === "Postgres")!;
    expect(client.x).toBeLessThan(api.x);
    expect(api.x).toBeLessThan(postgres.x);
  });

  it("stacks a stage's nodes and centres it against the tallest", () => {
    // Otherwise a stage with one node hangs off the top while its neighbour
    // fills the height, and the picture reads as though it starts halfway up.
    const { boxes, height } = layout(view());
    const api = boxes.find((b) => b.label === "API")!;
    const worker = boxes.find((b) => b.label === "Worker")!;
    const client = boxes.find((b) => b.label === "Client")!;
    expect(worker.y).toBeGreaterThan(api.y);

    const columnMiddle = (api.y + worker.y + worker.height) / 2;
    expect(Math.abs(client.y + client.height / 2 - columnMiddle)).toBeLessThan(2);
    expect(height).toBeGreaterThan(worker.y + worker.height);
  });

  it("widens a column to fit its longest label rather than clipping it", () => {
    const narrow = layout(view());
    const wide = layout(
      view({
        stages: [
          { name: "Edge", nodes: [{ label: "AVeryLongComponentName", note: null }] },
          ...view().stages.slice(1),
        ],
      }),
    );
    expect(wide.boxes[0].width).toBeGreaterThan(narrow.boxes[0].width);
  });

  it("curves a forward edge from one box's right face to the next box's left", () => {
    const { boxes, arrows } = layout(view());
    const client = boxes.find((b) => b.label === "Client")!;
    const [first] = arrows;
    expect(first.back).toBe(false);
    expect(first.path.startsWith(`M ${client.x + client.width}`)).toBe(true);
    expect(first.path).toContain("C");
  });

  it("drops an edge to an earlier stage below both boxes", () => {
    // Run straight it would lie along the forward edge it is the return path
    // for, which is the one thing a workflow diagram must not do.
    const { boxes, arrows, height } = layout(
      view({
        edges: [
          { from: "Client", to: "API", label: null },
          { from: "Postgres", to: "Client", label: "cached" },
        ],
      }),
    );
    const loop = arrows[1];
    const lowest = Math.max(...boxes.map((b) => b.y + b.height));

    expect(loop.back).toBe(true);
    expect(loop.path).toContain("V");
    // And the drawing grew to hold it, rather than clipping the loop away.
    expect(loop.extent.bottom).toBeGreaterThan(lowest);
    expect(height).toBeGreaterThan(loop.extent.bottom);
  });

  it("routes an edge inside one stage around the side, not underneath", () => {
    // Both boxes share a column, so "down, across, up" has no across — the
    // line would go down and come straight back over itself.
    const { boxes, arrows, overhang } = layout(
      view({ edges: [{ from: "Worker", to: "API", label: "retry" }] }),
    );
    const worker = boxes.find((b) => b.label === "Worker")!;
    const [loop] = arrows;

    expect(loop.back).toBe(true);
    expect(loop.path.startsWith(`M ${worker.x} `)).toBe(true);
    expect(loop.extent.bottom).toBe(0);
    // Nothing to shift: this loop is in the second column and reaches into the
    // gap before it, which is already there.
    expect(overhang).toBe(0);
  });

  it("shifts the drawing when a loop reaches left of the first column", () => {
    // The one case where the boxes are not the leftmost thing in the picture.
    // Sized to them alone, the loop would be cut off at the edge.
    const { overhang, boxes } = layout(
      view({
        stages: [
          {
            name: "Edge",
            nodes: [
              { label: "Client", note: null },
              { label: "Retrier", note: null },
            ],
          },
          { name: "Service", nodes: [{ label: "API", note: null }] },
        ],
        edges: [
          { from: "Client", to: "API", label: null },
          { from: "Retrier", to: "Client", label: "back off" },
        ],
      }),
    );
    expect(overhang).toBeGreaterThan(0);
    // And the boxes are still where the layout put them; the shift is applied
    // when the drawing is placed, not baked into every coordinate.
    expect(boxes[0].x).toBeGreaterThan(0);
  });

  it("drops an edge naming a node that does not exist", () => {
    const { arrows } = layout(
      view({
        edges: [
          { from: "Client", to: "Ghost", label: null },
          { from: "Client", to: "API", label: "POST /orders" },
        ],
      }),
    );
    expect(arrows).toHaveLength(1);
    expect(arrows[0].label).toBe("POST /orders");
  });

  it("only heads the stages the model named", () => {
    const { headings } = layout(view());
    expect(headings.map((h) => h.name)).toEqual(["Edge", "Service"]);
  });
});

describe("copying a flow as Mermaid", () => {
  it("carries the stages across as subgraphs", () => {
    // Without them Mermaid re-infers a layout and loses the one thing this
    // payload actually knows.
    const text = mermaid(view());
    expect(text.startsWith("flowchart LR")).toBe(true);
    expect(text).toContain("subgraph s0[Edge]");
    expect(text).toContain("subgraph s1[Service]");
  });

  it("aliases nodes so a label with a space cannot break the parse", () => {
    const text = mermaid(
      view({
        stages: [
          { name: null, nodes: [{ label: "Order Store", note: null }] },
          { name: null, nodes: [{ label: "API", note: null }] },
        ],
        edges: [{ from: "Order Store", to: "API", label: "reads" }],
      }),
    );
    expect(text).toContain('n0["Order Store"]');
    expect(text).toContain("n0 -->|reads| n1");
  });

  it("keeps a node's note on a second line", () => {
    expect(mermaid(view())).toContain('["API<br/>axum"]');
  });
});

describe("describing a flow for a screen reader", () => {
  it("names the stages and then the connections", () => {
    const text = described(view());
    expect(text).toContain("Edge: Client");
    expect(text).toContain("Service: API, Worker");
    // An unnamed stage still says which depth it is.
    expect(text).toContain("Stage 3: Postgres");
    expect(text).toContain("Client to API, POST /orders");
  });
});
