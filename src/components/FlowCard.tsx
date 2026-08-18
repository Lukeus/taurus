import { CopyButton } from "./CopyButton";
import type { TranscriptView } from "../lib/api";

type FlowView = Extract<TranscriptView, { type: "flow" }>;

/**
 * A `show_flow` result.
 *
 * Boxes in stages, arrows between them — the shape of a system rather than the
 * order of a conversation, which is what `show_sequence` is for.
 *
 * The layering is the model's. Working out which node sits at which depth, and
 * what order to put them in so the lines cross as little as possible, is the
 * hard half of drawing a graph and the half that fails visibly when it goes
 * wrong. It is also a question the model can already answer, because anything
 * worth diagramming was understood in stages before it was written down. Asked
 * for the stages, everything here is arithmetic.
 */
export function FlowCard({ view }: { view: FlowView }) {
  const { title, caption, stages, edges } = view;
  const plan = layout(view);

  return (
    <div className="view-card">
      <div className="view-head">
        <div>
          <h3>{title}</h3>
          {caption && <p className="view-caption">{caption}</p>}
        </div>
        <div className="spacer" />
        <span className="micro">{plural(plan.boxes.length, "node")}</span>
      </div>

      <div className="flow-box">
        <svg
          className="flow"
          width={plan.width}
          height={plan.height}
          viewBox={`0 0 ${plan.width} ${plan.height}`}
          role="img"
          aria-label={described(view)}
        >
          {/* Shifted whole rather than by letting the viewBox start negative,
              so the padding inside the box stays even on both sides when a
              loop reaches out to the left of the first column. */}
          <g transform={`translate(${plan.overhang}, 0)`}>
            {plan.headings.map((heading) => (
              <text
                key={heading.name}
                className="flow-stage"
                x={heading.x}
                y={STAGE_LABEL_Y}
                textAnchor="middle"
              >
                {heading.name}
              </text>
            ))}

            {/* Lines first, so one that has to pass a box goes behind it
                rather than across its label. */}
            {plan.arrows.map((arrow, i) => (
              <g key={i} className={`flow-arrow${arrow.back ? " back" : ""}`}>
                <path className="flow-line" d={arrow.path} fill="none" />
                <path className="flow-head" d={arrow.head} />
              </g>
            ))}

            {plan.boxes.map((box) => (
              <g key={box.label} className="flow-node">
                <rect
                  className="flow-rect"
                  x={box.x}
                  y={box.y}
                  width={box.width}
                  height={box.height}
                  rx={7}
                />
                <text
                  className="flow-name"
                  x={box.x + box.width / 2}
                  y={box.note ? box.y + 19 : box.y + box.height / 2 + 1}
                  textAnchor="middle"
                >
                  {box.label}
                </text>
                {box.note && (
                  <text
                    className="flow-note"
                    x={box.x + box.width / 2}
                    y={box.y + 34}
                    textAnchor="middle"
                  >
                    {box.note}
                  </text>
                )}
              </g>
            ))}

            {/* Last, and over everything. An edge label is centred on its own
                arrow, and a label longer than the gap it sits in would
                otherwise disappear under the box the arrow points at — which is
                exactly the label you most wanted to read. Drawn on top, with a
                halo in the ground colour so it stays legible over a line. */}
            {plan.arrows.map(
              (arrow, i) =>
                arrow.label && (
                  <text
                    key={i}
                    className={`flow-label${arrow.back ? " back" : ""}`}
                    x={arrow.labelAt.x}
                    y={arrow.labelAt.y}
                    textAnchor={arrow.labelAt.anchor ?? "middle"}
                  >
                    {arrow.label}
                  </text>
                ),
            )}
          </g>
        </svg>
      </div>

      <div className="view-foot">
        <span className="micro">
          {plural(stages.length, "stage")} · {plural(edges.length, "edge")}
        </span>
        <div className="spacer" />
        <CopyButton className="pill" label="copy as Mermaid" text={() => mermaid(view)} />
      </div>
    </div>
  );
}

const PAD = 16;
/** Room above the first box for the stage headings. */
const STAGE_H = 26;
const STAGE_LABEL_Y = 12;
/** Between one stage's column and the next. Wide enough for an edge label. */
const STAGE_GAP = 82;
/** Between two boxes in the same stage. */
const NODE_GAP = 22;
const NODE_H = 42;
const NODE_H_NOTE = 52;
const NODE_MIN_W = 90;
const NAME_SIZE = 12.5;
const NOTE_SIZE = 10.5;
/** See `SequenceCard` — text is estimated, not measured, so layout stays pure. */
const ADVANCE = 0.6;

function textWidth(text: string, size: number): number {
  return text.length * size * ADVANCE;
}

export type Box = {
  label: string;
  note: string | null;
  stage: number;
  x: number;
  y: number;
  width: number;
  height: number;
};

export type Arrow = {
  path: string;
  head: string;
  label: string | null;
  labelAt: { x: number; y: number; anchor?: "middle" | "end" };
  /** Points at its own stage or an earlier one — a retry, a callback. */
  back: boolean;
  /**
   * How far this arrow's routing reaches past the boxes.
   *
   * A loop is drawn outside the block of stages, so the drawing is bigger than
   * the boxes are — and an SVG sized to the boxes alone would clip exactly the
   * edges that most need to be seen. Reported per arrow and folded into the
   * viewBox once every route is known.
   */
  extent: { bottom: number; left: number };
};

export type FlowLayout = {
  width: number;
  height: number;
  boxes: Box[];
  headings: { name: string; x: number }[];
  arrows: Arrow[];
  /** How far everything is shifted right to bring a left-reaching loop into
   *  view. Zero for a diagram whose loops all run along the bottom. */
  overhang: number;
};

/**
 * Where every box and arrow goes.
 *
 * Pure, so it can be tested as the arithmetic it is. Two rules are worth
 * stating. A stage's column is as wide as its widest box, so a long label
 * pushes its own column out rather than being clipped. And a stage's boxes are
 * centred vertically against the tallest stage, which is what stops a diagram
 * with one node at the end from looking like it hangs off the top.
 */
export function layout(view: FlowView): FlowLayout {
  const { stages, edges } = view;

  // Each column is as wide as it needs to be, so one long label costs width
  // once rather than across every stage.
  const widths = stages.map((stage) =>
    stage.nodes.reduce(
      (max, node) =>
        Math.max(
          max,
          textWidth(node.label, NAME_SIZE) + 26,
          node.note ? textWidth(node.note, NOTE_SIZE) + 22 : 0,
        ),
      NODE_MIN_W,
    ),
  );
  const heights = stages.map((stage) =>
    stage.nodes.reduce(
      (sum, node) => sum + (node.note ? NODE_H_NOTE : NODE_H) + NODE_GAP,
      -NODE_GAP,
    ),
  );
  const tallest = Math.max(...heights, 0);

  const left = (stage: number) =>
    PAD + widths.slice(0, stage).reduce((sum, w) => sum + w + STAGE_GAP, 0);

  const boxes: Box[] = [];
  stages.forEach((stage, s) => {
    let y = PAD + STAGE_H + (tallest - heights[s]) / 2;
    for (const node of stage.nodes) {
      const height = node.note ? NODE_H_NOTE : NODE_H;
      boxes.push({
        label: node.label,
        note: node.note ?? null,
        stage: s,
        x: left(s),
        y,
        width: widths[s],
        height,
      });
      y += height + NODE_GAP;
    }
  });

  const at = new Map(boxes.map((box) => [box.label, box]));
  const arrows: Arrow[] = [];
  for (const edge of edges) {
    const from = at.get(edge.from);
    const to = at.get(edge.to);
    // Refused by the tool, so only a transcript from some other build gets
    // here. One arrow short beats a card that throws.
    if (!from || !to) continue;
    arrows.push(arrow(from, to, edge.label ?? null));
  }

  // Everything is laid out against a left edge of PAD; a loop that reaches
  // further left than that is shifted back into view by moving the drawing
  // rather than by letting the viewBox start negative, which keeps the box's
  // own padding even on both sides.
  const overhang = Math.max(0, PAD - Math.min(PAD, ...arrows.map((a) => a.extent.left)));
  const bottom = Math.max(
    PAD + STAGE_H + tallest,
    ...arrows.map((a) => a.extent.bottom),
  );

  const headings = stages
    .map((stage, s) => ({ name: stage.name ?? "", x: left(s) + widths[s] / 2 }))
    .filter((heading) => heading.name !== "");

  return {
    width: left(stages.length) - STAGE_GAP + PAD + overhang,
    height: bottom + PAD,
    boxes,
    headings,
    arrows,
    overhang,
  };
}

/**
 * One arrow, routed between two boxes.
 *
 * Three cases, because a graph drawn in stages has three kinds of edge and
 * running them all the same way makes the picture unreadable.
 *
 * A **forward** edge leaves the right face and arrives at the left, curved so
 * that two edges into one box arrive at different angles and stay tellable
 * apart.
 *
 * An edge **back to an earlier stage** — a retry, a failure path — is dropped
 * below both boxes and run along the bottom. Drawn straight it would lie along
 * the forward edges it is the return path for, and a loop that looks like the
 * line it loops over is the one thing a workflow diagram must not do.
 *
 * An edge **within one stage** cannot use the bottom route at all: both boxes
 * are in the same column, so "down, across, up" has no across, and the line
 * would go down and come straight back over itself. It goes around the left of
 * the column instead.
 */
function arrow(from: Box, to: Box, label: string | null): Arrow {
  if (to.stage > from.stage) {
    const x1 = from.x + from.width;
    const y1 = from.y + from.height / 2;
    const x2 = to.x;
    const y2 = to.y + to.height / 2;
    const bend = Math.max((x2 - x1) / 2, 18);
    return {
      path: `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`,
      head: head(x2, y2, "right"),
      label,
      // Above the midpoint of the curve, which for a cubic with symmetric
      // handles is the average of the two ends.
      labelAt: { x: (x1 + x2) / 2, y: (y1 + y2) / 2 - 8 },
      back: false,
      extent: { bottom: 0, left: 0 },
    };
  }

  if (to.stage === from.stage) {
    const y1 = from.y + from.height / 2;
    const y2 = to.y + to.height / 2;
    const out = from.x - SIDE_LOOP;
    return {
      path: `M ${from.x} ${y1} H ${out} V ${y2} H ${to.x}`,
      head: head(to.x, y2, "right"),
      label,
      labelAt: { x: out - 4, y: (y1 + y2) / 2, anchor: "end" },
      back: true,
      extent: { bottom: 0, left: out - labelWidth(label) - 8 },
    };
  }

  const x1 = from.x + from.width / 2;
  const y1 = from.y + from.height;
  const x2 = to.x + to.width / 2;
  const y2 = to.y + to.height;
  const drop = Math.max(y1, y2) + BOTTOM_LOOP;
  return {
    path: `M ${x1} ${y1} V ${drop} H ${x2} V ${y2}`,
    head: head(x2, y2, "up"),
    label,
    labelAt: { x: (x1 + x2) / 2, y: drop + 13 },
    back: true,
    // The label sits under the run, so the box has to reach past both.
    extent: { bottom: drop + (label ? 20 : 6), left: 0 },
  };
}

/** How far a same-stage loop reaches left of its column. */
const SIDE_LOOP = 22;
/** How far a backward loop drops below the lower of its two boxes. */
const BOTTOM_LOOP = 26;

function labelWidth(label: string | null): number {
  return label ? textWidth(label, NOTE_SIZE) : 0;
}

/**
 * The arrowhead, as a filled triangle. A path per arrow rather than a shared
 * `<marker>`, which would be addressed by a document id two cards could
 * collide on.
 */
function head(x: number, y: number, pointing: "right" | "up"): string {
  return pointing === "up"
    ? `M ${x} ${y} L ${x - 4} ${y + 7} L ${x + 4} ${y + 7} Z`
    : `M ${x} ${y} L ${x - 7} ${y - 4} L ${x - 7} ${y + 4} Z`;
}

/** What a screen reader is told, since the picture says nothing to one. */
export function described(view: FlowView): string {
  const stages = view.stages
    .map((stage, i) => {
      const names = stage.nodes.map((n) => n.label).join(", ");
      return `${stage.name ?? `Stage ${i + 1}`}: ${names}`;
    })
    .join(". ");
  const edges = view.edges
    .map((e) => `${e.from} to ${e.to}${e.label ? `, ${e.label}` : ""}`)
    .join(". ");
  return `Flow diagram, ${view.title}. ${stages}. Connections: ${edges}.`;
}

/**
 * The same diagram as Mermaid source.
 *
 * Nodes are aliased to `n0`, `n1` … for the reason the sequence diagram
 * aliases participants: a label with a space, a bracket or an arrow in it is
 * either a parse error or a node nobody declared. Stages become subgraphs when
 * the model named them, which is what carries the layering across — without it
 * Mermaid would re-infer a layout and lose the one thing this payload knows.
 */
export function mermaid(view: FlowView): string {
  const alias = new Map<string, string>();
  view.stages.forEach((stage) =>
    stage.nodes.forEach((node) => alias.set(node.label, `n${alias.size}`)),
  );

  const lines = ["flowchart LR"];
  view.stages.forEach((stage, s) => {
    const named = stage.name?.trim();
    if (named) lines.push(`    subgraph s${s}[${named}]`);
    for (const node of stage.nodes) {
      const text = node.note ? `${node.label}<br/>${node.note}` : node.label;
      lines.push(`    ${named ? "    " : ""}${alias.get(node.label)}["${text}"]`);
    }
    if (named) lines.push("    end");
  });
  for (const edge of view.edges) {
    const from = alias.get(edge.from);
    const to = alias.get(edge.to);
    if (!from || !to) continue;
    lines.push(
      edge.label ? `    ${from} -->|${edge.label}| ${to}` : `    ${from} --> ${to}`,
    );
  }
  return lines.join("\n");
}

function plural(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}
