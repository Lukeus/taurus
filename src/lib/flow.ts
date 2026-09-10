/**
 * Where every box and arrow in a flow diagram goes.
 *
 * Pure arithmetic over a diagram that already knows its own layering, so it can
 * be tested as the arithmetic it is. It lives in `lib` rather than inside
 * `FlowCard` because there are now two things that draw a flow: a `show_flow`
 * card in the transcript, and a ```mermaid fence in a note. The card owns its
 * head, its foot and its copy button; the geometry belongs to neither.
 *
 * # Why the caller declares the layering
 *
 * Working out which node sits at which depth is the hard half of drawing a
 * graph and the half that fails visibly when it goes wrong. `show_flow` asks
 * the model for the stages, because anything worth diagramming was understood
 * in stages before it was written down. A Mermaid fence gets them from its
 * subgraphs when it has them and from [`layer`] when it does not. Either way,
 * by the time a diagram reaches this file the columns are decided and
 * everything here is arithmetic.
 *
 * # Why a node has a key as well as a label
 *
 * `show_flow` refuses two nodes with the same label, because there an edge
 * names its ends by label and two boxes sharing one would send every arrow to
 * whichever came first. Mermaid does not work that way: it has ids, and
 * `a["User"]` and `b["User"]` are two different boxes that read the same. So a
 * node carries the name edges use to find it *and* the text drawn inside it,
 * and they are allowed to differ. For a `show_flow` payload they are the same
 * string — see [`fromView`].
 */

import type { TranscriptView } from "./api";

type FlowView = Extract<TranscriptView, { type: "flow" }>;

/** One box, as the caller describes it. */
export type FlowNodeIn = {
  /**
   * What edges name it. Unique across the diagram; a later node reusing a key
   * is the earlier one's box, which is what `A --> B` then `A --> C` means.
   */
  key: string;
  /** What is drawn inside it. */
  label: string;
  /** A second, quieter line inside the box. */
  note?: string | null;
};

/** One column: everything at the same depth. */
export type FlowStageIn = { name?: string | null; nodes: FlowNodeIn[] };

/** One arrow. `from` and `to` are node keys. */
export type FlowEdgeIn = { from: string; to: string; label?: string | null };

export type FlowInput = {
  title: string;
  stages: FlowStageIn[];
  edges: FlowEdgeIn[];
};

/** A `show_flow` payload as this file wants it: the label is also the key. */
export function fromView(view: FlowView): FlowInput {
  return {
    title: view.title,
    stages: view.stages.map((stage) => ({
      name: stage.name,
      nodes: stage.nodes.map((node) => ({
        key: node.label,
        label: node.label,
        note: node.note,
      })),
    })),
    edges: view.edges,
  };
}

export type Box = {
  /** What edges name it. Not drawn. */
  key: string;
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
  labelAt: { x: number; y: number; anchor?: "start" | "middle" | "end" };
  /**
   * Certainly a loop: an arrow to an earlier stage, or a box to itself.
   *
   * Not "routed the long way round" — a same-stage edge is too, and is not
   * drawn as a loop. This is the flag that dashes the line, so it says only what
   * can be known: an arrow that goes back is a retry, a failure path or a
   * callback, and an arrow between two things at the same depth might be a step.
   */
  back: boolean;
  /**
   * How far this arrow's routing reaches past the boxes.
   *
   * A loop is drawn outside the block of stages, so the drawing is bigger than
   * the boxes are — and an SVG sized to the boxes alone would clip exactly the
   * edges that most need to be seen. Reported per arrow and folded into the
   * viewBox once every route is known.
   */
  extent: { bottom: number; left: number; right: number };
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
 * Two rules are worth stating. A stage's column is as wide as its widest box,
 * so a long label pushes its own column out rather than being clipped. And a
 * stage's boxes are centred vertically against the tallest stage, which is what
 * stops a diagram with one node at the end from looking like it hangs off the
 * top.
 */
export function plan(input: FlowInput): FlowLayout {
  const stages = input.stages.filter((stage) => stage.nodes.length > 0);
  if (stages.length === 0) {
    return { width: 0, height: 0, boxes: [], headings: [], arrows: [], overhang: 0 };
  }

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
        key: node.key,
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

  const at = new Map(boxes.map((box) => [box.key, box]));

  // Resolved before anything is routed, because the lanes below are packed
  // against the arrows that will actually be drawn. An edge naming a node that
  // does not exist is refused by the tool and by the Mermaid reader, so only a
  // transcript from some other build gets here — one arrow short beats a card
  // that throws.
  const routes: Route[] = [];
  for (const edge of input.edges) {
    const from = at.get(edge.from);
    const to = at.get(edge.to);
    if (!from || !to) continue;
    routes.push({ from, to, label: edge.label ?? null, kind: kindOf(from, to) });
  }

  // Every backward run is measured from the same floor: the deepest box in the
  // whole diagram. Measuring from its own two boxes is what a three-stage
  // diagram gets away with — at nine stages a loop between two short columns
  // runs straight through a tall one.
  const floor = boxes.reduce((low, box) => Math.max(low, box.y + box.height), 0);

  const under = lanes(
    routes.filter((r) => r.kind === "under"),
    (r) => xSpan(r),
  );
  // Packed per column: two loops inside one stage collide with each other, and
  // never with a loop in a different stage.
  const side = lanes(
    routes.filter((r) => r.kind === "side"),
    (r) => ySpan(r),
    (r) => r.from.stage,
  );
  // A box with two self-edges is unusual and legal. The second ring sits
  // outside the first rather than on top of it.
  const self = lanes(
    routes.filter((r) => r.kind === "self"),
    () => [0, 0],
    (r) => r.from.key,
  );

  const arrows = routes.map((route) =>
    arrow(route, {
      floor,
      lane: (under.get(route) ?? side.get(route) ?? self.get(route) ?? 0),
    }),
  );

  // Everything is laid out against a left edge of PAD; a loop that reaches
  // further left than that is shifted back into view by moving the drawing
  // rather than by letting the viewBox start negative, which keeps the box's
  // own padding even on both sides.
  const overhang = Math.max(0, PAD - Math.min(PAD, ...arrows.map((a) => a.extent.left)));
  const bottom = Math.max(
    PAD + STAGE_H + tallest,
    ...arrows.map((a) => a.extent.bottom),
  );
  const right = Math.max(
    left(stages.length) - STAGE_GAP,
    ...arrows.map((a) => a.extent.right),
  );

  const headings = stages
    .map((stage, s) => ({ name: stage.name ?? "", x: left(s) + widths[s] / 2 }))
    .filter((heading) => heading.name !== "");

  return {
    width: right + PAD + overhang,
    height: bottom + PAD,
    boxes,
    headings,
    arrows,
    overhang,
  };
}

/**
 * Which of the four routes an edge takes.
 *
 * A graph drawn in stages has four kinds of edge and running them all the same
 * way makes the picture unreadable. `self` is checked before `side` because a
 * box and itself share a stage, and the side route between them collapses to a
 * line that leaves and re-enters at the same height — 22px out and straight
 * back over itself.
 */
type Kind = "forward" | "under" | "side" | "self";

function kindOf(from: Box, to: Box): Kind {
  if (from === to) return "self";
  if (to.stage > from.stage) return "forward";
  if (to.stage === from.stage) return "side";
  return "under";
}

type Route = { from: Box; to: Box; label: string | null; kind: Kind };

/**
 * One arrow, routed.
 *
 * A **forward** edge leaves the right face and arrives at the left, curved so
 * that two edges into one box arrive at different angles and stay tellable
 * apart.
 *
 * An edge **back to an earlier stage** — a retry, a failure path — is dropped
 * below every box and run along the bottom. Drawn straight it would lie along
 * the forward edges it is the return path for, and a loop that looks like the
 * line it loops over is the one thing a workflow diagram must not do.
 *
 * An edge **within one stage** cannot use the bottom route at all: both boxes
 * are in the same column, so "down, across, up" has no across, and the line
 * would go down and come straight back over itself. It goes around the left of
 * the column instead — routed like a loop, but not drawn as one, since being at
 * the same depth says nothing about whether the arrow is a step or a retry.
 *
 * An edge **from a box to itself** has neither an across nor a height to travel
 * over, so it is a ring over the top face. It goes above rather than below or
 * beside because those are the two directions already carrying every other kind
 * of loop, and because the space above a box is the one place in this layout
 * that is always empty: the stage headings sit at [`STAGE_LABEL_Y`], the boxes
 * start [`STAGE_H`] below them, and a ring [`SELF_LOOP`] tall clears both.
 */
function arrow(route: Route, at: { floor: number; lane: number }): Arrow {
  const { from, to, label } = route;

  if (route.kind === "forward") {
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
      extent: { bottom: 0, left: 0, right: 0 },
    };
  }

  if (route.kind === "self") {
    const mid = from.x + from.width / 2;
    const out = mid + SELF_W;
    const back = mid - SELF_W;
    const top = from.y - SELF_LOOP - at.lane * SELF_LANE;
    return {
      path: `M ${out} ${from.y} V ${top} H ${back} V ${from.y}`,
      head: head(back, from.y, "down"),
      label,
      // Beside the ring rather than above it. Centred above, a label on the
      // top box of a named stage would sit in the stage heading's line.
      labelAt: { x: out + 5, y: top + 4, anchor: "start" },
      back: true,
      extent: {
        bottom: 0,
        left: 0,
        right: out + (label ? labelWidth(label) + 9 : 0),
      },
    };
  }

  if (route.kind === "side") {
    const y1 = from.y + from.height / 2;
    const y2 = to.y + to.height / 2;
    const out = from.x - SIDE_LOOP - at.lane * SIDE_LANE;
    return {
      path: `M ${from.x} ${y1} H ${out} V ${y2} H ${to.x}`,
      head: head(to.x, y2, "right"),
      label,
      labelAt: { x: out - 4, y: (y1 + y2) / 2, anchor: "end" },
      // Routed like a loop and **not drawn as one**. The route is geometry —
      // two boxes in one column have no "across" to run along, so the line has
      // to go around. The dash is a claim, and here it would be the wrong one:
      // an edge between two things at the same depth is as often the next step
      // as it is a retry. `show_flow` only ever promised the loop treatment for
      // an arrow "pointing back to an earlier stage", and a Mermaid subgraph
      // routinely holds a chain — which arrived dashed, reading as a failure
      // path, in the first photograph of one.
      back: false,
      extent: { bottom: 0, left: out - labelWidth(label) - 8, right: 0 },
    };
  }

  const x1 = from.x + from.width / 2;
  const y1 = from.y + from.height;
  const x2 = to.x + to.width / 2;
  const y2 = to.y + to.height;
  const drop = at.floor + BOTTOM_LOOP + at.lane * BOTTOM_LANE;
  return {
    path: `M ${x1} ${y1} V ${drop} H ${x2} V ${y2}`,
    head: head(x2, y2, "up"),
    label,
    labelAt: { x: (x1 + x2) / 2, y: drop + 13 },
    back: true,
    // The label sits under the run, so the box has to reach past both.
    extent: { bottom: drop + (label ? 20 : 6), left: 0, right: 0 },
  };
}

/**
 * Which lane each route runs in, so two loops never share a line.
 *
 * The bug this fixes is invisible on a small diagram and constant on a large
 * one: every backward edge used to drop the same distance below the boxes, so
 * two of them ran along the same y and their labels landed on top of each
 * other. Greedy interval packing is enough — routes that do not overlap share a
 * lane, and the widest is placed first so a loop spanning the diagram sits
 * outside the ones it contains rather than cutting through them.
 *
 * `group` keeps separate families apart: two side loops collide only if they
 * are in the same column, and pretending otherwise pushes the second one out
 * past a column it never touches.
 */
function lanes<T>(
  routes: T[],
  span: (route: T) => [number, number],
  group: (route: T) => string | number = () => "",
): Map<T, number> {
  const taken = new Map<string | number, [number, number][][]>();
  const at = new Map<T, number>();

  const widest = [...routes].sort((a, b) => {
    const [al, ah] = span(a);
    const [bl, bh] = span(b);
    return bh - bl - (ah - al);
  });

  for (const route of widest) {
    const key = group(route);
    const family = taken.get(key) ?? [];
    if (!taken.has(key)) taken.set(key, family);

    const [lo, hi] = span(route);
    // Inflated, so two runs that merely touch at an endpoint do not read as one
    // line that turns a corner.
    const from = lo - LANE_MARGIN;
    const to = hi + LANE_MARGIN;

    let lane = family.findIndex((used) =>
      used.every(([l, h]) => to <= l || from >= h),
    );
    if (lane === -1) {
      lane = family.length;
      family.push([]);
    }
    family[lane].push([from, to]);
    at.set(route, lane);
  }

  return at;
}

/** What a bottom run covers horizontally: the two box centres it joins. */
function xSpan(route: Route): [number, number] {
  const a = route.from.x + route.from.width / 2;
  const b = route.to.x + route.to.width / 2;
  return [Math.min(a, b), Math.max(a, b)];
}

/** What a side loop covers vertically. */
function ySpan(route: Route): [number, number] {
  const a = route.from.y + route.from.height / 2;
  const b = route.to.y + route.to.height / 2;
  return [Math.min(a, b), Math.max(a, b)];
}

const PAD = 16;
/** Room above the first box for the stage headings. */
const STAGE_H = 26;
export const STAGE_LABEL_Y = 12;
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

/** How far a same-stage loop reaches left of its column. */
const SIDE_LOOP = 22;
/** Between one side loop and the next one out, in the same column. */
const SIDE_LANE = 16;
/** How far the first backward loop drops below the deepest box. */
const BOTTOM_LOOP = 26;
/** Between one bottom run and the next one down. Clears a label, which hangs
 *  13px under its own run. */
const BOTTOM_LANE = 24;
/** How far a self-loop's ring rises above its box's top face. */
const SELF_LOOP = 18;
/** Between one self-loop ring and the next, on the same box. */
const SELF_LANE = 8;
/** Half the width of a self-loop's ring. */
const SELF_W = 12;
/** Slack between two runs in one lane, so touching ends do not read as joined. */
const LANE_MARGIN = 6;

function textWidth(text: string, size: number): number {
  return text.length * size * ADVANCE;
}

function labelWidth(label: string | null): number {
  return label ? textWidth(label, NOTE_SIZE) : 0;
}

/**
 * The arrowhead, as a filled triangle. A path per arrow rather than a shared
 * `<marker>`, which would be addressed by a document id two cards could
 * collide on.
 */
function head(x: number, y: number, pointing: "right" | "up" | "down"): string {
  if (pointing === "up") return `M ${x} ${y} L ${x - 4} ${y + 7} L ${x + 4} ${y + 7} Z`;
  if (pointing === "down") return `M ${x} ${y} L ${x - 4} ${y - 7} L ${x + 4} ${y - 7} Z`;
  return `M ${x} ${y} L ${x - 7} ${y - 4} L ${x - 7} ${y + 4} Z`;
}

/** What a screen reader is told, since the picture says nothing to one. */
export function describe(input: FlowInput): string {
  const stages = input.stages
    .map((stage, i) => {
      const names = stage.nodes.map((n) => n.label).join(", ");
      return `${stage.name ?? `Stage ${i + 1}`}: ${names}`;
    })
    .join(". ");
  const label = new Map(
    input.stages.flatMap((stage) => stage.nodes.map((n) => [n.key, n.label] as const)),
  );
  const edges = input.edges
    .map((e) => {
      const from = label.get(e.from) ?? e.from;
      const to = label.get(e.to) ?? e.to;
      return `${from} to ${to}${e.label ? `, ${e.label}` : ""}`;
    })
    .join(". ");
  return `Flow diagram, ${input.title}. ${stages}. Connections: ${edges}.`;
}

/**
 * The same diagram as Mermaid source.
 *
 * Nodes are aliased to `n0`, `n1` … for the reason the sequence diagram
 * aliases participants: a label with a space, a bracket or an arrow in it is
 * either a parse error or a node nobody declared. Stages become subgraphs when
 * they are named, which is what carries the layering across — without it
 * Mermaid would re-infer a layout and lose the one thing this payload knows.
 */
export function mermaid(input: FlowInput): string {
  const alias = new Map<string, string>();
  input.stages.forEach((stage) =>
    stage.nodes.forEach((node) => alias.set(node.key, `n${alias.size}`)),
  );

  const lines = ["flowchart LR"];
  input.stages.forEach((stage, s) => {
    const named = stage.name?.trim();
    if (named) lines.push(`    subgraph s${s}[${named}]`);
    for (const node of stage.nodes) {
      const text = node.note ? `${node.label}<br/>${node.note}` : node.label;
      lines.push(`    ${named ? "    " : ""}${alias.get(node.key)}["${text}"]`);
    }
    if (named) lines.push("    end");
  });
  for (const edge of input.edges) {
    const from = alias.get(edge.from);
    const to = alias.get(edge.to);
    if (!from || !to) continue;
    lines.push(
      edge.label ? `    ${from} -->|${edge.label}| ${to}` : `    ${from} --> ${to}`,
    );
  }
  return lines.join("\n");
}
