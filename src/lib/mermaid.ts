/**
 * Mermaid, read.
 *
 * The app has emitted Mermaid since `show_flow` shipped — both diagram cards
 * carry a **copy as Mermaid** button, because a diagram gets pasted into a
 * README or an issue. Nothing read it back. A note that can hold a ```mermaid
 * fence is the other half of that round trip, and this is it.
 *
 * # Why a scanner rather than the library
 *
 * `SequenceCard` wrote the argument down before there was anything to decide:
 * a diagram library "would be the largest dependency in the app by two orders
 * of magnitude, would arrive with its own palette to fight, and could not be
 * checked before it was rendered". All three still hold. `mermaid` is 80 MB
 * unpacked across d3, three cytoscape packages, katex and marked; it themes
 * itself; it loads its own fonts, which the app's CSP would have to be widened
 * for; and it generates element ids per render, which the screenshot suite
 * would then have to tolerate. What this file feeds instead is `lib/flow` and
 * `lib/sequence` — the engines that already draw these two diagrams, in the
 * app's own palette, checked by tests that run without a browser.
 *
 * The price is that this reads a subset, and the honest thing to do with a
 * subset is say so on screen rather than draw something else. Every return
 * that is not a diagram carries a sentence naming what stopped it, and a
 * diagram that dropped something it could not draw says what.
 *
 * # What it reads
 *
 * `flowchart` and `graph` in any direction, though the picture is always laid
 * out left to right — direction in Mermaid is presentational, so the boxes,
 * arrows and labels are all still right, and [`read`] reports the re-orientation
 * rather than hiding it. Subgraphs become the stages when every node is in one,
 * which is exactly what this app emits; otherwise the layering is inferred by
 * longest path. And `sequenceDiagram`, where the lanes and the order are
 * declared and there is nothing to infer.
 *
 * Everything else — `classDiagram`, `stateDiagram`, `erDiagram`, `gantt`,
 * `pie`, and the rest — is named and refused.
 */

import type { FlowEdgeIn, FlowInput, FlowNodeIn, FlowStageIn } from "./flow";
import type { MessageIn, ParticipantIn, SequenceInput } from "./sequence";

/**
 * What a fence turned out to hold.
 *
 * `skipped` is on the two drawable arms rather than being a third arm, because
 * a sequence diagram with a `loop` block in it is a diagram we can draw with one
 * thing missing — and refusing the whole picture over the frame around it would
 * be worse for the reader than drawing it and saying what is not there.
 */
export type Read =
  | { kind: "flow"; input: FlowInput; skipped: string[] }
  | { kind: "sequence"; input: SequenceInput; skipped: string[] }
  | { kind: "refused"; why: string };

/** Reads a ```mermaid fence's contents. */
export function read(source: string): Read {
  const { title, lines } = preamble(source);
  const first = lines.find((line) => line.text !== "");
  if (!first) return { kind: "refused", why: "The diagram is empty." };

  const header = /^(\w[\w-]*)\b(.*)$/.exec(first.text);
  const type = header?.[1] ?? first.text;
  const rest = lines.filter((line) => line !== first);

  if (type === "flowchart" || type === "graph" || type === "flowchart-elk") {
    return flow(rest, title, (header?.[2] ?? "").trim());
  }
  if (type === "sequenceDiagram") {
    return sequence(rest, title);
  }
  return {
    kind: "refused",
    why: `Taurus draws ${named(type)} as text, not as a picture — it reads flowcharts and sequence diagrams so far.`,
  };
}

/**
 * The diagram types this does not draw, by the name a person would use.
 *
 * Spelled out rather than echoed, because `quadrantChart` in the middle of a
 * sentence reads as a typo and "a quadrant chart" reads as an answer. An
 * unlisted keyword is echoed as itself, which is right for one Mermaid added
 * after this was written.
 */
const NAMES: Record<string, string> = {
  classDiagram: "class diagrams",
  "classDiagram-v2": "class diagrams",
  stateDiagram: "state diagrams",
  "stateDiagram-v2": "state diagrams",
  erDiagram: "entity-relationship diagrams",
  journey: "user journeys",
  gantt: "Gantt charts",
  pie: "pie charts",
  quadrantChart: "quadrant charts",
  requirementDiagram: "requirement diagrams",
  gitGraph: "git graphs",
  mindmap: "mind maps",
  timeline: "timelines",
  sankey: "Sankey diagrams",
  "sankey-beta": "Sankey diagrams",
  xychart: "XY charts",
  "xychart-beta": "XY charts",
  block: "block diagrams",
  "block-beta": "block diagrams",
  packet: "packet diagrams",
  "packet-beta": "packet diagrams",
  kanban: "Kanban boards",
  architecture: "architecture diagrams",
  "architecture-beta": "architecture diagrams",
  radar: "radar charts",
  treemap: "treemaps",
  zenuml: "ZenUML diagrams",
  C4Context: "C4 diagrams",
  C4Container: "C4 diagrams",
  C4Component: "C4 diagrams",
  mermaid: "that",
};

function named(type: string): string {
  return NAMES[type] ?? `\`${type}\``;
}

type Line = { text: string; at: number };

/**
 * Strips what sits in front of the diagram, and finds its title.
 *
 * Three things can. A `---` frontmatter block, which is where Mermaid puts a
 * title and which is YAML — only `title:` is read out of it, because the rest
 * configures a renderer this file is not. An `%%{init: …}%%` directive, which is
 * the same. And `%%` comments anywhere.
 *
 * Statements are also split on `;`, which Mermaid allows as a terminator, so
 * everything downstream can treat a line as one statement.
 */
function preamble(source: string): { title: string; lines: Line[] } {
  const raw = source.replace(/\r\n?/g, "\n").split("\n");
  let title = "";
  let from = 0;

  if (raw[0]?.trim() === "---") {
    const close = raw.findIndex((line, i) => i > 0 && line.trim() === "---");
    if (close > 0) {
      for (const line of raw.slice(1, close)) {
        const found = /^\s*title\s*:\s*(.*)$/.exec(line);
        if (found) title = found[1].trim().replace(/^["']|["']$/g, "");
      }
      from = close + 1;
    }
  }

  const lines: Line[] = [];
  raw.slice(from).forEach((line, i) => {
    const at = from + i + 1;
    const bare = line
      .replace(/%%\{[\s\S]*?\}%%/g, "")
      .replace(/%%.*$/, "")
      .trim();
    // A `;` inside a label is a `;` in a label, not a terminator. Splitting
    // only outside brackets and quotes costs one scan, and saves a node
    // labelled `Wait; then retry` from becoming two statements.
    for (const text of splitOutside(bare, ";")) {
      lines.push({ text: text.trim(), at });
    }
  });

  return { title, lines };
}

/** Splits on `sep`, ignoring any that sits inside quotes or brackets. */
function splitOutside(text: string, sep: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let quoted = false;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === '"') quoted = !quoted;
    else if (!quoted && "([{".includes(c)) depth++;
    else if (!quoted && ")]}".includes(c)) depth = Math.max(0, depth - 1);
    else if (!quoted && depth === 0 && c === sep) {
      parts.push(text.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(text.slice(start));
  return parts.filter((part) => part.trim() !== "");
}

// --------------------------------------------------------------- flowcharts

/** A node as the source declared it, before it is put in a column. */
export type Node = { key: string; label: string; note: string | null; order: number };

/**
 * The directions that are not the one this draws, and how to say so.
 *
 * Direction in Mermaid is presentational: `graph TD` and `graph LR` carry the
 * same nodes, the same arrows and the same labels. Laying a `TD` out left to
 * right re-orients the picture and loses nothing, which is why it is reported
 * rather than refused. Stages as rows is a second geometry for the engine and a
 * feature in its own right.
 */
const REORIENTED: Record<string, string> = {
  TB: "top to bottom",
  TD: "top to bottom",
  BT: "bottom to top",
  RL: "right to left",
};

/**
 * Statements that style rather than say anything.
 *
 * Dropped in silence, unlike everything in `skipped`: these carry no content a
 * reader of the picture would miss, and listing them under every diagram would
 * train people to ignore the line that does matter.
 */
const DECORATION = /^(style|classDef|class|linkStyle|click|accTitle|accDescr)\b/;

function flow(lines: Line[], title: string, direction: string): Read {
  const skipped: string[] = [];
  const asked = REORIENTED[direction.trim().toUpperCase()];
  if (asked) skipped.push(`Laid out left to right rather than ${asked}`);

  const nodes = new Map<string, Node>();
  const edges: FlowEdgeIn[] = [];

  /**
   * The columns as the source laid them out, in the order they were opened.
   *
   * A subgraph opens one. So does the first node declared after a subgraph
   * closed, which is what makes this round-trip this app's own output:
   * `mermaid()` writes a named stage as a subgraph and an unnamed one as bare
   * nodes between them, so a diagram with three stages and two names has two
   * subgraphs and a loose run after them.
   */
  const slots: { name: string | null }[] = [];
  const slotOf = new Map<string, number>();
  /** Subgraphs currently open, innermost last. */
  const open: number[] = [];
  /** Where a node declared right now would go. */
  let current: number | null = null;
  let subgraphs = 0;
  let nested = false;

  const declare = (ref: Ref): Node => {
    const existing = nodes.get(ref.key);
    if (existing) {
      // A later mention with a label wins over an earlier bare one: `A --> B`
      // then `B[Store]` is how people write these.
      if (ref.label !== null) {
        existing.label = ref.label;
        existing.note = ref.note;
      }
      return existing;
    }
    const node: Node = {
      key: ref.key,
      label: ref.label ?? ref.key,
      note: ref.note,
      order: nodes.size,
    };
    nodes.set(ref.key, node);
    if (current === null) {
      slots.push({ name: null });
      current = slots.length - 1;
    }
    slotOf.set(ref.key, current);
    return node;
  };

  for (const line of lines) {
    const text = line.text;
    if (text === "") continue;

    if (/^end\b/.test(text)) {
      open.pop();
      // Back inside whatever contained it, or nowhere — where the next node
      // opens a column of its own.
      current = open.length > 0 ? open[open.length - 1] : null;
      continue;
    }
    const subgraph = /^subgraph\s+(.*)$/.exec(text);
    if (subgraph) {
      subgraphs += 1;
      slots.push({ name: subgraphName(subgraph[1]) || null });
      current = slots.length - 1;
      open.push(current);
      if (open.length > 1) nested = true;
      continue;
    }
    if (/^direction\b/.test(text)) continue;
    if (DECORATION.test(text)) continue;

    const chain = statement(text);
    if (chain === null) {
      return {
        kind: "refused",
        why: `Line ${line.at} is not something this reads yet: \`${text}\``,
      };
    }

    // `A & B --> C` is a fan, and every left node connects to every right one.
    let previous: Ref[] | null = null;
    for (const step of chain) {
      for (const ref of step.refs) declare(ref);
      if (previous && step.link) {
        for (const from of previous) {
          for (const to of step.refs) {
            edges.push({ from: from.key, to: to.key, label: step.link.label });
            if (step.link.both) {
              edges.push({ from: to.key, to: from.key, label: step.link.label });
            }
          }
        }
      }
      previous = step.refs;
    }
  }

  if (nodes.size === 0) {
    return { kind: "refused", why: "The diagram declares no nodes." };
  }

  const order = [...nodes.values()].sort((a, b) => a.order - b.order);

  // Subgraphs are the columns when there are any, because that is the reading
  // this app's own output needs: `show_flow` names its stages and `mermaid()`
  // writes each named one as a subgraph.
  //
  // Elsewhere a subgraph means "a box drawn around some boxes", which is not
  // something this draws, and taking it as a column instead is presentational
  // rather than wrong — every node, arrow and label is still there, and a chain
  // that happened to be inside one is drawn as arrows within a column. The same
  // trade the direction makes.
  //
  // One inside another is the case with no such reading: the inner group is
  // both a column and part of one. That falls back to inferring the layering,
  // and says so.
  let stages: FlowStageIn[];
  if (subgraphs > 0 && !nested) {
    stages = slots.map((slot, i) => ({
      name: slot.name,
      nodes: order.filter((node) => slotOf.get(node.key) === i).map(asNode),
    }));
  } else {
    if (nested) {
      skipped.push(
        `${plural(subgraphs, "subgraph")} flattened, because one is inside another`,
      );
    }
    stages = layer(order, edges).map((column) => ({
      name: null,
      nodes: column.map(asNode),
    }));
  }

  return {
    kind: "flow",
    input: { title: title || "Diagram", stages, edges },
    skipped,
  };
}

function asNode(node: Node): FlowNodeIn {
  return { key: node.key, label: node.label, note: node.note };
}

/**
 * `subgraph one[Title]`, `subgraph one["Title"]`, or `subgraph Title`.
 *
 * The bracketed form's id is discarded: a stage is a column with a heading, and
 * nothing needs to name one.
 */
function subgraphName(rest: string): string {
  const bracketed = /^[^\s[]*\s*\[(.*)\]\s*$/.exec(rest);
  const text = bracketed ? bracketed[1] : rest;
  return unquote(text.trim());
}

/**
 * Which column each node belongs in, when the source did not say.
 *
 * Longest path from the sources: a node sits one to the right of the furthest
 * thing that reaches it. This is the half of drawing a graph that `show_flow`
 * asks the model to do, on the grounds that it fails visibly when it goes wrong
 * — a fence has nobody to ask, so it is done here and tested.
 *
 * Cycles are the only wrinkle, and a retry loop is the most ordinary shape a
 * flowchart has. Edges that close a cycle are found by depth-first search and
 * left out of the layering, which leaves a DAG to measure; the engine then draws
 * them as the backward loops they are. Nodes are walked in declaration order, so
 * which edge of a cycle is treated as the closing one is stable rather than
 * dependent on map iteration.
 */
export function layer(nodes: Node[], edges: FlowEdgeIn[]): Node[][] {
  const forward = new Map<string, string[]>();
  const known = new Set(nodes.map((node) => node.key));
  for (const node of nodes) forward.set(node.key, []);
  for (const edge of edges) {
    if (edge.from === edge.to) continue;
    if (!known.has(edge.from) || !known.has(edge.to)) continue;
    forward.get(edge.from)!.push(edge.to);
  }

  // Depth-first, marking every edge that points at something already on the
  // stack. Those are the ones that close a cycle.
  const back = new Set<string>();
  const state = new Map<string, "open" | "shut">();
  const walk = (key: string) => {
    state.set(key, "open");
    for (const next of forward.get(key)!) {
      const seen = state.get(next);
      if (seen === "open") back.add(`${key} ${next}`);
      else if (seen === undefined) walk(next);
    }
    state.set(key, "shut");
  };
  for (const node of nodes) if (!state.has(node.key)) walk(node.key);

  const into = new Map<string, string[]>();
  const out = new Map<string, number>();
  for (const node of nodes) {
    into.set(node.key, []);
    out.set(node.key, 0);
  }
  for (const [from, tos] of forward) {
    for (const to of tos) {
      if (back.has(`${from} ${to}`)) continue;
      into.get(to)!.push(from);
      out.set(from, out.get(from)! + 1);
    }
  }

  // Longest path, relaxed in topological order. Kahn from the sinks backwards
  // would give the same answer; forwards from the sources keeps a node's depth
  // one past whatever reaches it, which is the reading direction.
  const depth = new Map<string, number>(nodes.map((node) => [node.key, 0]));
  const ready = nodes.filter((node) => into.get(node.key)!.length === 0);
  const queue = ready.map((node) => node.key);
  const left = new Map(
    nodes.map((node) => [node.key, into.get(node.key)!.length]),
  );
  while (queue.length > 0) {
    const key = queue.shift()!;
    for (const next of forward.get(key)!) {
      if (back.has(`${key} ${next}`)) continue;
      depth.set(next, Math.max(depth.get(next)!, depth.get(key)! + 1));
      const remaining = left.get(next)! - 1;
      left.set(next, remaining);
      if (remaining === 0) queue.push(next);
    }
  }

  const deepest = Math.max(...[...depth.values()]);
  const columns: Node[][] = Array.from({ length: deepest + 1 }, () => []);
  // Declaration order within a column, which is the only order the source
  // actually stated.
  for (const node of nodes) columns[depth.get(node.key)!].push(node);
  return columns.filter((column) => column.length > 0);
}

// ------------------------------------------------------- flowchart statements

/** A node as one statement referred to it. */
type Ref = { key: string; label: string | null; note: string | null };

/** What joins two node references. */
type Link = { label: string | null; both: boolean };

type Step = { refs: Ref[]; link: Link | null };

/**
 * One flowchart statement: node references joined by links.
 *
 * `A`, `A[Start]`, `A --> B`, `A -->|yes| B --> C`, `A & B --> C`. Returns
 * `null` for something this does not read, which is a refusal naming the line
 * rather than a diagram missing an arrow nobody was told about.
 */
export function statement(text: string): Step[] | null {
  const steps: Step[] = [];
  let rest = text;
  let link: Link | null = null;

  for (;;) {
    const refs: Ref[] = [];
    for (;;) {
      const ref = reference(rest);
      if (!ref) return null;
      refs.push(ref.ref);
      rest = ref.rest.trimStart();
      if (rest.startsWith("&")) {
        rest = rest.slice(1).trimStart();
        continue;
      }
      break;
    }
    steps.push({ refs, link });

    if (rest === "") return steps;
    const next = connector(rest);
    if (!next) return null;
    link = next.link;
    rest = next.rest.trimStart();
    if (rest === "") return null;
  }
}

/**
 * The shapes a node's text can be wrapped in, and what closes each.
 *
 * All of them draw as a rectangle. A diamond for a decision and a cylinder for
 * a store are a real loss and they are written down as one — but reading the
 * text out of them is not optional, because `A{Is it cached?}` with the shape
 * unparsed is a box labelled `A`.
 */
const SHAPES: [open: string, close: string][] = [
  ["(((", ")))"],
  ["((", "))"],
  ["([", "])"],
  ["[[", "]]"],
  ["[(", ")]"],
  ["[/", "/]"],
  ["[\\", "\\]"],
  ["{{", "}}"],
  ["[/", "\\]"],
  ["[\\", "/]"],
  ["[", "]"],
  ["(", ")"],
  ["{", "}"],
  [">", "]"],
];

/**
 * An identifier, then optionally its text in one of the shapes.
 *
 * An id is runs of word characters joined by single dots or hyphens, so
 * `user-service` and `a.b` are one id each while `A-->B` is an id, a link and
 * an id. Allowing a hyphen freely is the version of this that looks right and
 * reads `A-->B` as a node called `A--`.
 */
function reference(text: string): { ref: Ref; rest: string } | null {
  const id = /^([A-Za-z0-9_]+(?:[-.][A-Za-z0-9_]+)*)/.exec(text);
  if (!id) return null;
  let rest = text.slice(id[0].length);

  for (const [open, close] of SHAPES) {
    if (!rest.startsWith(open)) continue;
    const end = closing(rest, open.length, close);
    if (end === -1) continue;
    const inside = rest.slice(open.length, end);
    rest = rest.slice(end + close.length);
    const { label, note } = split(inside);
    return { ref: { key: id[1], label, note }, rest };
  }

  return { ref: { key: id[1], label: null, note: null }, rest };
}

/** Where `close` sits, skipping any inside quotes. */
function closing(text: string, from: number, close: string): number {
  let quoted = false;
  for (let i = from; i <= text.length - close.length; i++) {
    if (text[i] === '"') {
      quoted = !quoted;
      continue;
    }
    if (!quoted && text.startsWith(close, i)) return i;
  }
  return -1;
}

/**
 * A node's text, split at its first line break.
 *
 * `show_flow` gives a node a label and an optional quieter second line, and
 * `mermaid()` writes that out as `Label<br/>note`. Reading it back the same way
 * is what closes the round trip — without it, a diagram copied out of the app
 * and pasted into a note comes back with `<br/>` printed in the box.
 */
function split(inside: string): { label: string; note: string | null } {
  const text = unquote(inside.trim());
  const at = /<br\s*\/?>/i.exec(text);
  if (!at) return { label: text, note: null };
  return {
    label: text.slice(0, at.index).trim(),
    note: text.slice(at.index + at[0].length).trim() || null,
  };
}

function unquote(text: string): string {
  const quoted = /^"([\s\S]*)"$/.exec(text.trim());
  return (quoted ? quoted[1] : text).trim();
}

/**
 * A link between two nodes, in any of the forms people write.
 *
 * `-->` and its longer relatives, `---`, `==>`, `-.->`, an arrowhead of `>`,
 * `o` or `x`, a `<` on the tail for a two-way arrow, and a label either after
 * the link in `|bars|` or in the middle of it as `-- yes -->`.
 *
 * Tried middle-label first: `A -- yes --> B` also matches the plain form with
 * `yes --> B` left over, which would then fail to parse as a node reference and
 * refuse a diagram that is perfectly ordinary.
 */
export function connector(text: string): { link: Link; rest: string } | null {
  const middle =
    /^\s*(<)?(?:-{2,}|={2,}|-\.+)\s*([^-=<>|]+?)\s*(?:-{2,}|={2,}|\.-+)([>ox])?\s*/.exec(
      text,
    );
  if (middle && middle[2].trim() !== "") {
    return {
      link: { label: middle[2].trim(), both: middle[1] === "<" },
      rest: text.slice(middle[0].length),
    };
  }

  const plain = /^\s*(<)?(?:-\.+-*|-{2,}|={2,})([>ox])?\s*(?:\|([^|]*)\|)?\s*/.exec(text);
  if (!plain || plain[0].trim() === "") return null;
  return {
    link: {
      label: plain[3] === undefined ? null : unquote(plain[3]) || null,
      both: plain[1] === "<",
    },
    rest: text.slice(plain[0].length),
  };
}

// ---------------------------------------------------------- sequence diagrams

/**
 * Arrows, longest first.
 *
 * A dashed arrow is a return and a solid one is a call, which is the whole of
 * what [`MessageKind`] carries. Mermaid's crosses and open circles say something
 * finer about whether a call failed or was async; the diagram is drawn without
 * that distinction, and `show_sequence` argues at its own `MessageKind` why two
 * kinds is the right number for a picture somebody reads once.
 */
const ARROWS: [token: string, kind: "call" | "return"][] = [
  ["<<-->>", "return"],
  ["<<->>", "call"],
  ["-->>", "return"],
  ["--x", "return"],
  ["--)", "return"],
  ["-->", "return"],
  ["->>", "call"],
  ["-x", "call"],
  ["-)", "call"],
  ["->", "call"],
];

/** Blocks whose frame is not drawn, by the word that opens them. */
const BLOCKS = new Set([
  "loop",
  "alt",
  "else",
  "opt",
  "par",
  "and",
  "critical",
  "option",
  "break",
  "rect",
  "box",
]);

function sequence(lines: Line[], title: string): Read {
  const participants: ParticipantIn[] = [];
  const seen = new Map<string, ParticipantIn>();
  const messages: MessageIn[] = [];
  const dropped = new Map<string, number>();

  const declare = (key: string, label?: string): ParticipantIn => {
    const existing = seen.get(key);
    if (existing) {
      if (label) existing.label = label;
      return existing;
    }
    const lane = { key, label: label ?? key };
    seen.set(key, lane);
    participants.push(lane);
    return lane;
  };

  const drop = (what: string) => dropped.set(what, (dropped.get(what) ?? 0) + 1);

  for (const line of lines) {
    const text = line.text;
    if (text === "") continue;

    const word = /^(\w+)/.exec(text)?.[1] ?? "";

    if (word === "participant" || word === "actor") {
      const rest = text.slice(word.length).trim();
      const alias = /^(.*?)\s+as\s+(.*)$/.exec(rest);
      if (alias) declare(alias[1].trim(), unquote(alias[2]));
      else declare(rest.trim(), unquote(rest.trim()));
      continue;
    }
    if (word === "autonumber" || word === "end" || word === "deactivate") continue;
    if (word === "activate") continue;
    if (word === "Note" || word === "note") {
      drop("note");
      continue;
    }
    if (BLOCKS.has(word)) {
      drop(word === "box" ? "participant group" : `${word} block`);
      continue;
    }
    if (word === "create" || word === "destroy") {
      // `create participant C` is the declaration plus an annotation on when it
      // appears. The lane is real; the timing is not drawn.
      const rest = text.replace(/^\w+\s+(participant|actor)?\s*/, "").trim();
      if (word === "create" && rest) declare(rest.split(/\s+as\s+/)[0].trim());
      drop("lifetime marker");
      continue;
    }
    if (DECORATION.test(text) || word === "accTitle" || word === "accDescr") continue;

    const message = arrow(text);
    if (!message) {
      return {
        kind: "refused",
        why: `Line ${line.at} is not something this reads yet: \`${text}\``,
      };
    }
    declare(message.from);
    declare(message.to);
    messages.push(message);
  }

  if (participants.length === 0) {
    return { kind: "refused", why: "The diagram declares no participants." };
  }

  // One clause rather than one per kind: "1 note not drawn. 1 loop block not
  // drawn." is the same sentence twice, and the reader wants the list.
  const counted = [...dropped].map(([what, n]) => plural(n, what));
  const skipped =
    counted.length === 0
      ? []
      : [`${list(counted)} not drawn`];
  return {
    kind: "sequence",
    input: { title: title || "Diagram", participants, messages },
    skipped,
  };
}

/**
 * `A->>B: text`, with `+`/`-` activation suffixes ignored.
 *
 * The arrow is looked for in front of the colon only. `A->>B: use --> for edges`
 * is a call whose text mentions an arrow, and searching the whole line would
 * read the one in the prose.
 */
export function arrow(text: string): MessageIn | null {
  const colon = text.indexOf(":");
  if (colon === -1) return null;
  const head = text.slice(0, colon);

  for (const [token, kind] of ARROWS) {
    const at = head.indexOf(token);
    if (at <= 0) continue;
    const from = head.slice(0, at).trim();
    // `A->>+B` activates B and `A->>-B` deactivates it. The mark is on the
    // target, not after it.
    const to = head
      .slice(at + token.length)
      .trim()
      .replace(/^[+-]/, "")
      .trim();
    if (from === "" || to === "") continue;
    // Mermaid splits a message line on its arrow, so a lane whose name has a
    // space in it is one Mermaid cannot parse either — which is why this app
    // aliases them on the way out. Refusing beats inventing a lane.
    if (/\s/.test(from) || /\s/.test(to)) continue;
    return { from, to, text: text.slice(colon + 1).trim(), kind };
  }
  return null;
}

function plural(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}

/** `a`, `a and b`, `a, b and c`. */
function list(parts: string[]): string {
  if (parts.length === 1) return parts[0];
  return `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}
