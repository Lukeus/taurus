import { describe as suite, expect, it } from "vitest";

import { describe, fromView, mermaid, plan, type FlowInput } from "./flow";
import type { TranscriptView } from "./api";

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

/** The layout of a `show_flow` payload, which is what most of these are about. */
const layout = (patch: Partial<FlowView> = {}) => plan(fromView(view(patch)));

suite("laying out a flow diagram", () => {
  it("puts each stage in its own column, left to right", () => {
    const { boxes } = layout();
    const client = boxes.find((b) => b.label === "Client")!;
    const api = boxes.find((b) => b.label === "API")!;
    const postgres = boxes.find((b) => b.label === "Postgres")!;
    expect(client.x).toBeLessThan(api.x);
    expect(api.x).toBeLessThan(postgres.x);
  });

  it("stacks a stage's nodes and centres it against the tallest", () => {
    // Otherwise a stage with one node hangs off the top while its neighbour
    // fills the height, and the picture reads as though it starts halfway up.
    const { boxes, height } = layout();
    const api = boxes.find((b) => b.label === "API")!;
    const worker = boxes.find((b) => b.label === "Worker")!;
    const client = boxes.find((b) => b.label === "Client")!;
    expect(worker.y).toBeGreaterThan(api.y);

    const columnMiddle = (api.y + worker.y + worker.height) / 2;
    expect(Math.abs(client.y + client.height / 2 - columnMiddle)).toBeLessThan(2);
    expect(height).toBeGreaterThan(worker.y + worker.height);
  });

  it("widens a column to fit its longest label rather than clipping it", () => {
    const narrow = layout();
    const wide = layout({
      stages: [
        { name: "Edge", nodes: [{ label: "AVeryLongComponentName", note: null }] },
        ...view().stages.slice(1),
      ],
    });
    expect(wide.boxes[0].width).toBeGreaterThan(narrow.boxes[0].width);
  });

  it("curves a forward edge from one box's right face to the next box's left", () => {
    const { boxes, arrows } = layout();
    const client = boxes.find((b) => b.label === "Client")!;
    const [first] = arrows;
    expect(first.back).toBe(false);
    expect(first.path.startsWith(`M ${client.x + client.width}`)).toBe(true);
    expect(first.path).toContain("C");
  });

  it("drops an edge to an earlier stage below every box", () => {
    // Run straight it would lie along the forward edge it is the return path
    // for, which is the one thing a workflow diagram must not do.
    const { boxes, arrows, height } = layout({
      edges: [
        { from: "Client", to: "API", label: null },
        { from: "Postgres", to: "Client", label: "cached" },
      ],
    });
    const loop = arrows[1];
    const lowest = Math.max(...boxes.map((b) => b.y + b.height));

    expect(loop.back).toBe(true);
    expect(loop.path).toContain("V");
    // And the drawing grew to hold it, rather than clipping the loop away.
    expect(loop.extent.bottom).toBeGreaterThan(lowest);
    expect(height).toBeGreaterThan(loop.extent.bottom);
  });

  it("measures a bottom run from the deepest box, not from its own two", () => {
    // Its own two boxes is what a three-stage diagram gets away with. Here the
    // loop joins two single-node columns either side of a tall one, and
    // measured from its own ends it would run straight through the tall one.
    const { boxes, arrows } = layout({
      stages: [
        { name: null, nodes: [{ label: "Client", note: null }] },
        {
          name: null,
          nodes: [
            { label: "A", note: null },
            { label: "B", note: null },
            { label: "C", note: null },
          ],
        },
        { name: null, nodes: [{ label: "Postgres", note: null }] },
      ],
      edges: [{ from: "Postgres", to: "Client", label: null }],
    });
    const [loop] = arrows;
    const deepest = Math.max(...boxes.map((b) => b.y + b.height));
    const run = Number(/V (\d+(?:\.\d+)?) H/.exec(loop.path)![1]);
    expect(run).toBeGreaterThan(deepest);
  });

  it("gives two bottom runs that overlap their own lanes", () => {
    // The defect this is about is invisible on a small diagram and constant on
    // a large one: both runs used to drop the same distance, so they shared a y
    // and their labels landed on top of each other.
    const { arrows } = layout({
      edges: [
        { from: "Postgres", to: "Client", label: "cached" },
        { from: "Worker", to: "Client", label: "retry" },
      ],
    });
    const runs = arrows.map((a) => Number(/V (\d+(?:\.\d+)?) H/.exec(a.path)![1]));
    expect(runs[0]).not.toBe(runs[1]);
    // Far enough apart that a label hanging under the upper one clears the
    // lower one.
    expect(Math.abs(runs[0] - runs[1])).toBeGreaterThanOrEqual(20);
  });

  it("shares one lane between two bottom runs that do not overlap", () => {
    // Four stages, and two return paths that cover different halves. Pushing
    // the second one deeper would make the drawing taller for nothing.
    const { arrows } = plan({
      title: "t",
      stages: [
        { nodes: [{ key: "a", label: "A" }] },
        { nodes: [{ key: "b", label: "B" }] },
        { nodes: [{ key: "c", label: "C" }] },
        { nodes: [{ key: "d", label: "D" }] },
      ],
      edges: [
        { from: "b", to: "a" },
        { from: "d", to: "c" },
      ],
    });
    const runs = arrows.map((a) => Number(/V (\d+(?:\.\d+)?) H/.exec(a.path)![1]));
    expect(runs[0]).toBe(runs[1]);
  });

  it("routes an edge inside one stage around the side, not underneath", () => {
    // Both boxes share a column, so "down, across, up" has no across — the
    // line would go down and come straight back over itself.
    const { boxes, arrows, overhang } = layout({
      edges: [{ from: "Worker", to: "API", label: "retry" }],
    });
    const worker = boxes.find((b) => b.label === "Worker")!;
    const [loop] = arrows;

    expect(loop.back).toBe(true);
    expect(loop.path.startsWith(`M ${worker.x} `)).toBe(true);
    expect(loop.extent.bottom).toBe(0);
    // Nothing to shift: this loop is in the second column and reaches into the
    // gap before it, which is already there.
    expect(overhang).toBe(0);
  });

  it("steps a second side loop in the same column further out", () => {
    const { arrows } = plan({
      title: "t",
      stages: [
        {
          nodes: [
            { key: "a", label: "A" },
            { key: "b", label: "B" },
            { key: "c", label: "C" },
          ],
        },
        { nodes: [{ key: "d", label: "D" }] },
      ],
      edges: [
        { from: "c", to: "a" },
        { from: "b", to: "a" },
      ],
    });
    const outs = arrows.map((a) => Number(/H (-?\d+(?:\.\d+)?) V/.exec(a.path)![1]));
    expect(outs[0]).not.toBe(outs[1]);
  });

  it("leaves a side loop in another column alone", () => {
    // Two columns each with a loop of their own. They cannot collide, and
    // pushing the second one out past a column it never touches would say they
    // could.
    const { arrows, boxes } = plan({
      title: "t",
      stages: [
        {
          nodes: [
            { key: "a", label: "A" },
            { key: "b", label: "B" },
          ],
        },
        {
          nodes: [
            { key: "c", label: "C" },
            { key: "d", label: "D" },
          ],
        },
      ],
      edges: [
        { from: "b", to: "a" },
        { from: "d", to: "c" },
      ],
    });
    const [first, second] = arrows;
    const out = (a: typeof first) => Number(/H (-?\d+(?:\.\d+)?) V/.exec(a.path)![1]);
    // Each sits the same distance to the left of its own column.
    expect(boxes[0].x - out(first)).toBe(boxes[2].x - out(second));
  });

  it("rings a self-edge over the top of its box", () => {
    // The route that used to be wrong. A box and itself share a stage, so this
    // took the side route, where both ends are at the same height — 22px out
    // and straight back over itself, drawn as a flat spike.
    const { boxes, arrows } = layout({
      edges: [{ from: "Worker", to: "Worker", label: "poll" }],
    });
    const worker = boxes.find((b) => b.label === "Worker")!;
    const [ring] = arrows;

    expect(ring.back).toBe(true);
    // Leaves and re-enters the top face, at two different x.
    const [, outX, topY, backX] =
      /M (-?[\d.]+) [\d.]+ V (-?[\d.]+) H (-?[\d.]+) V/.exec(ring.path)!;
    expect(Number(outX)).toBeGreaterThan(Number(backX));
    expect(Number(topY)).toBeLessThan(worker.y);
    // And clears the stage headings, which is the whole reason the ring goes
    // above rather than below.
    expect(Number(topY)).toBeGreaterThan(14);
    // The head points back down into the box.
    expect(ring.head).toContain(`M ${backX} ${worker.y}`);
  });

  it("keeps a self-edge's label beside the ring and makes room for it", () => {
    // Centred above, a label on the top box of a named stage sits in the stage
    // heading's own line.
    const { arrows, width } = layout({
      stages: [
        { name: "Edge", nodes: [{ label: "Client", note: null }] },
        { name: "Service", nodes: [{ label: "API", note: null }] },
      ],
      edges: [{ from: "API", to: "API", label: "a good long label" }],
    });
    const [ring] = arrows;
    expect(ring.labelAt.anchor).toBe("start");
    expect(ring.extent.right).toBeGreaterThan(0);
    expect(width).toBeGreaterThan(ring.extent.right);
  });

  it("shifts the drawing when a loop reaches left of the first column", () => {
    // The one case where the boxes are not the leftmost thing in the picture.
    // Sized to them alone, the loop would be cut off at the edge.
    const { overhang, boxes } = layout({
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
    });
    expect(overhang).toBeGreaterThan(0);
    // And the boxes are still where the layout put them; the shift is applied
    // when the drawing is placed, not baked into every coordinate.
    expect(boxes[0].x).toBeGreaterThan(0);
  });

  it("drops an edge naming a node that does not exist", () => {
    const { arrows } = layout({
      edges: [
        { from: "Client", to: "Ghost", label: null },
        { from: "Client", to: "API", label: "POST /orders" },
      ],
    });
    expect(arrows).toHaveLength(1);
    expect(arrows[0].label).toBe("POST /orders");
  });

  it("only heads the stages that were named", () => {
    const { headings } = layout();
    expect(headings.map((h) => h.name)).toEqual(["Edge", "Service"]);
  });

  it("draws nothing for a diagram with no nodes", () => {
    // Refused by the tool and by the reader, so this is only reachable from a
    // transcript written by some other build. Zero beats a negative width.
    const empty = plan({ title: "t", stages: [], edges: [] });
    expect(empty).toEqual({
      width: 0,
      height: 0,
      boxes: [],
      headings: [],
      arrows: [],
      overhang: 0,
    });
    expect(plan({ title: "t", stages: [{ nodes: [] }], edges: [] }).width).toBe(0);
  });
});

suite("a node's key and its label", () => {
  const twins: FlowInput = {
    title: "Two of them",
    stages: [
      { nodes: [{ key: "a", label: "User" }] },
      { nodes: [{ key: "b", label: "User" }] },
    ],
    edges: [{ from: "a", to: "b", label: "syncs to" }],
  };

  it("keeps two nodes that read the same apart", () => {
    // `show_flow` refuses a duplicate label, because there the label *is* the
    // identity. Mermaid has ids, and `a["User"]` and `b["User"]` are two boxes.
    const { boxes, arrows } = plan(twins);
    expect(boxes.map((b) => b.label)).toEqual(["User", "User"]);
    expect(boxes[0].x).not.toBe(boxes[1].x);
    expect(arrows).toHaveLength(1);
    expect(arrows[0].back).toBe(false);
  });

  it("names both by their label when describing them", () => {
    expect(describe(twins)).toContain("User to User, syncs to");
  });

  it("aliases by key when emitting Mermaid, so the two survive the round trip", () => {
    const text = mermaid(twins);
    expect(text).toContain('n0["User"]');
    expect(text).toContain('n1["User"]');
    expect(text).toContain("n0 -->|syncs to| n1");
  });

  it("is the label for a show_flow payload", () => {
    const input = fromView(view());
    expect(input.stages[0].nodes[0]).toEqual({
      key: "Client",
      label: "Client",
      note: null,
    });
  });
});

suite("copying a flow as Mermaid", () => {
  it("carries the stages across as subgraphs", () => {
    // Without them Mermaid re-infers a layout and loses the one thing this
    // payload actually knows.
    const text = mermaid(fromView(view()));
    expect(text.startsWith("flowchart LR")).toBe(true);
    expect(text).toContain("subgraph s0[Edge]");
    expect(text).toContain("subgraph s1[Service]");
  });

  it("aliases nodes so a label with a space cannot break the parse", () => {
    const text = mermaid(
      fromView(
        view({
          stages: [
            { name: null, nodes: [{ label: "Order Store", note: null }] },
            { name: null, nodes: [{ label: "API", note: null }] },
          ],
          edges: [{ from: "Order Store", to: "API", label: "reads" }],
        }),
      ),
    );
    expect(text).toContain('n0["Order Store"]');
    expect(text).toContain("n0 -->|reads| n1");
  });

  it("keeps a node's note on a second line", () => {
    expect(mermaid(fromView(view()))).toContain('["API<br/>axum"]');
  });
});

suite("describing a flow for a screen reader", () => {
  it("names the stages and then the connections", () => {
    const text = describe(fromView(view()));
    expect(text).toContain("Edge: Client");
    expect(text).toContain("Service: API, Worker");
    // An unnamed stage still says which depth it is.
    expect(text).toContain("Stage 3: Postgres");
    expect(text).toContain("Client to API, POST /orders");
  });
});
