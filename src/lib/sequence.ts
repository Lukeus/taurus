/**
 * Where every lane and arrow in a sequence diagram goes.
 *
 * Pure arithmetic, so it can be tested as arithmetic and so the first paint is
 * correct. It lives in `lib` rather than inside `SequenceCard` for the reason
 * `lib/flow` does: a `show_sequence` card in the transcript and a ```mermaid
 * fence in a note draw the same picture with different chrome around it.
 *
 * The layout needs no algorithm. The lanes are declared in order and the
 * messages are in order, which is the whole of it — see `lib/flow` for the
 * contrast, where the layering is the hard part and the caller has to supply it.
 *
 * The one thing worth stating: a label is centred over its own arrow and each
 * message owns a row, so two labels can never collide with one another — only
 * with the edges of the box. That is what the extent pass at the end is for, and
 * why the viewBox can start left of zero.
 *
 * # Why a participant has a key as well as a label
 *
 * `show_sequence` names its lanes by the string drawn at the top of them.
 * Mermaid separates the two — `participant p0 as Order Store` — and this file
 * is read by both, so a lane carries the name messages use to find it and the
 * text drawn above it. For a `show_sequence` payload they are the same string.
 */

import type { MessageKind } from "../bindings/MessageKind";
import type { TranscriptView } from "./api";

type SequenceView = Extract<TranscriptView, { type: "sequence" }>;

/** One lane, as the caller describes it. */
export type ParticipantIn = {
  /** What messages name it. */
  key: string;
  /** What is drawn at the top of the lane. */
  label: string;
};

export type MessageIn = {
  from: string;
  to: string;
  text: string;
  kind: MessageKind;
};

export type SequenceInput = {
  title: string;
  participants: ParticipantIn[];
  messages: MessageIn[];
};

/** A `show_sequence` payload as this file wants it: the label is also the key. */
export function fromView(view: SequenceView): SequenceInput {
  return {
    title: view.title,
    participants: view.participants.map((name) => ({ key: name, label: name })),
    messages: view.messages,
  };
}

export type Row = {
  y: number;
  /** Lane centres, in pixels. Equal for a call a participant makes to itself. */
  from: number;
  to: number;
  self: boolean;
  kind: MessageKind;
  text: string;
};

export type Layout = {
  /** The viewBox's left edge, which a long first label can push negative. */
  left: number;
  width: number;
  height: number;
  lanes: { x: number; label: string }[];
  rows: Row[];
};

/** Where every lane and arrow goes. */
export function plan(input: SequenceInput): Layout {
  const { participants, messages } = input;

  const widest = participants.reduce(
    (max, lane) => Math.max(max, textWidth(lane.label, NAME_SIZE)),
    0,
  );
  const laneWidth = Math.max(LANE_MIN, widest + 34);
  const centre = (i: number) => PAD + laneWidth / 2 + i * laneWidth;

  const lanes = participants.map((lane, i) => ({ x: centre(i), label: lane.label }));
  const index = new Map(participants.map((lane, i) => [lane.key, i]));

  let y = HEAD_H + HEAD_GAP;
  const rows: Row[] = [];
  for (const message of messages) {
    const from = index.get(message.from);
    const to = index.get(message.to);
    // Unreachable through the tool, which refuses an arrow naming a lane it
    // never declared, and through the reader, which declares a lane the first
    // time it is mentioned. A transcript hand-edited past that check drops the
    // row rather than drawing an arrow from nowhere.
    if (from === undefined || to === undefined) continue;

    const self = from === to;
    rows.push({
      y,
      from: centre(from),
      to: centre(to),
      self,
      kind: message.kind,
      text: message.text,
    });
    y += self ? SELF_H : ROW_H;
  }

  const height = y - ROW_H + HEAD_GAP + PAD;

  // How far the drawing actually reaches, labels included. A label centred over
  // a short arrow in the leftmost lane runs off the left of the box, and a
  // viewBox that started at zero would cut it in half.
  let left = 0;
  let right = participants.length * laneWidth + PAD * 2;
  for (const row of rows) {
    const half = textWidth(row.text, LABEL_SIZE) / 2;
    const at = row.self ? row.from + SELF_W + 8 + half : (row.from + row.to) / 2;
    left = Math.min(left, at - half - PAD);
    right = Math.max(right, at + half + PAD);
  }
  for (const lane of lanes) {
    const half = textWidth(lane.label, NAME_SIZE) / 2;
    left = Math.min(left, lane.x - half - PAD);
    right = Math.max(right, lane.x + half + PAD);
  }

  return { left, width: right - left, height, lanes, rows };
}

/** Space around the drawing. */
export const PAD = 14;
/** The participant row at the top, and where the lifelines start. */
export const HEAD_H = 30;
/** Between the participant row and the first arrow. */
const HEAD_GAP = 20;
/** One message. */
const ROW_H = 38;
/** How far a self-call reaches to the right of its own lane, and drops. */
export const SELF_W = 26;
export const SELF_DROP = 20;
/** A self-call needs the drop plus the usual breathing room. */
const SELF_H = ROW_H + SELF_DROP;
/** Narrowest a lane may be, whatever its name. */
const LANE_MIN = 128;

/**
 * Roughly how wide a string renders, in pixels.
 *
 * Estimated rather than measured: the layout has to be a pure function so it
 * can be tested and so the first paint is correct, and measuring text needs a
 * laid-out document that neither a test nor a first render has. Everything
 * here is set in the mono face at a known size, where the advance is close
 * enough to constant that the estimate is only ever a pixel or two out over a
 * label — and the only thing riding on it is how much margin the box leaves.
 */
const ADVANCE = 0.6;

function textWidth(text: string, size: number): number {
  return text.length * size * ADVANCE;
}

const NAME_SIZE = 12;
const LABEL_SIZE = 11.5;

/**
 * The arrowhead, as a filled triangle.
 *
 * A path per arrow rather than one `<marker>` referenced by all of them:
 * markers are addressed by document id, and two diagrams in one transcript
 * would either collide on the name or need one generated per card.
 */
export function head(x: number, y: number, direction: 1 | -1): string {
  const back = x + direction * 7;
  return `M ${x} ${y} L ${back} ${y - 4} L ${back} ${y + 4} Z`;
}

/**
 * What a screen reader is told, since the picture itself says nothing to one.
 *
 * The order of events in a sentence per arrow, which is the same information
 * the drawing carries and the form it was in before it was drawn.
 */
export function describe(input: SequenceInput): string {
  const label = new Map(input.participants.map((p) => [p.key, p.label] as const));
  const name = (key: string) => label.get(key) ?? key;
  const steps = input.messages.map((m) =>
    m.from === m.to
      ? `${name(m.from)} ${m.text}`
      : `${name(m.from)} ${m.kind === "return" ? "returns to" : "to"} ${name(m.to)}: ${m.text}`,
  );
  return `Sequence diagram, ${input.title}. ${steps.join(". ")}.`;
}

/**
 * The same diagram as Mermaid source.
 *
 * Participants are aliased to `p0`, `p1` … rather than named directly: Mermaid
 * splits a message line on its arrow, so a participant with a space in its name
 * — `Order Store` — produces a diagram that either fails to parse or quietly
 * invents a lane. The alias makes every name safe without asking the caller to
 * avoid spaces.
 */
export function mermaid(input: SequenceInput): string {
  const alias = new Map(input.participants.map((p, i) => [p.key, `p${i}`]));
  const lines = ["sequenceDiagram"];
  for (const participant of input.participants) {
    lines.push(`    participant ${alias.get(participant.key)} as ${participant.label}`);
  }
  for (const message of input.messages) {
    const from = alias.get(message.from);
    const to = alias.get(message.to);
    if (!from || !to) continue;
    const arrow = message.kind === "return" ? "-->>" : "->>";
    lines.push(`    ${from}${arrow}${to}: ${message.text}`);
  }
  return lines.join("\n");
}
