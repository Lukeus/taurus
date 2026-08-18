import { CopyButton } from "./CopyButton";
import type { SequenceMessage, TranscriptView } from "../lib/api";

type SequenceView = Extract<TranscriptView, { type: "sequence" }>;

/**
 * A `show_sequence` result.
 *
 * Drawn rather than described, because the thing a sequence diagram is for —
 * seeing where a chain of calls turns around, and which lane the work is
 * actually happening in — is not something a numbered list can show. The
 * layout needs no algorithm: the model declared the lanes in order and the
 * messages in order, so this is arithmetic over a payload that already knows
 * its own shape.
 *
 * Hand-drawn SVG for the same reason the chart is hand-drawn divs. A diagram
 * library would be the largest dependency in the app by two orders of
 * magnitude, would arrive with its own palette to fight, and could not be
 * checked before it was rendered — where this payload is refused by the tool
 * if an arrow names a lane that was never declared.
 */
export function SequenceCard({ view }: { view: SequenceView }) {
  const { title, caption, participants, messages } = view;
  const plan = layout(view);

  return (
    <div className="view-card">
      <div className="view-head">
        <div>
          <h3>{title}</h3>
          {caption && <p className="view-caption">{caption}</p>}
        </div>
        <div className="spacer" />
        <span className="micro">{plural(messages.length, "message")}</span>
      </div>

      {/* Scrolls rather than scales. A diagram squeezed to fit a narrow window
          shrinks its labels out of legibility, and the labels are the half of
          it that carries the meaning. */}
      <div className="sequence-box">
        <svg
          className="sequence"
          width={plan.width}
          height={plan.height}
          viewBox={`${plan.left} 0 ${plan.width} ${plan.height}`}
          role="img"
          aria-label={described(view)}
        >
          {plan.lanes.map((lane, i) => (
            <g key={`${lane.label}-${i}`}>
              <line
                className="sequence-life"
                x1={lane.x}
                y1={HEAD_H}
                x2={lane.x}
                y2={plan.height - PAD}
              />
              <text className="sequence-name" x={lane.x} y={HEAD_H / 2}>
                {lane.label}
              </text>
            </g>
          ))}

          {plan.rows.map((row, i) =>
            row.self ? (
              <g key={i} className={`sequence-arrow ${row.kind}`}>
                <path
                  className="sequence-line"
                  d={`M ${row.from} ${row.y} h ${SELF_W} v ${SELF_DROP} h ${-SELF_W}`}
                  fill="none"
                />
                <path
                  className="sequence-head"
                  d={head(row.from, row.y + SELF_DROP, 1)}
                />
                <text
                  className="sequence-label"
                  x={row.from + SELF_W + 8}
                  y={row.y + SELF_DROP / 2}
                  textAnchor="start"
                >
                  {row.text}
                </text>
              </g>
            ) : (
              <g key={i} className={`sequence-arrow ${row.kind}`}>
                <line
                  className="sequence-line"
                  x1={row.from}
                  y1={row.y}
                  x2={row.to}
                  y2={row.y}
                />
                <path
                  className="sequence-head"
                  d={head(row.to, row.y, row.to > row.from ? -1 : 1)}
                />
                <text
                  className="sequence-label"
                  x={(row.from + row.to) / 2}
                  y={row.y - 8}
                  textAnchor="middle"
                >
                  {row.text}
                </text>
              </g>
            ),
          )}
        </svg>
      </div>

      <div className="view-foot">
        <span className="micro">
          {plural(participants.length, "participant")}
        </span>
        <div className="spacer" />
        {/* Mermaid because it is what a diagram gets pasted into — a README, a
            design doc, an issue. The app does not depend on it to draw this;
            it just speaks it on the way out. */}
        <CopyButton className="pill" label="copy as Mermaid" text={() => mermaid(view)} />
      </div>
    </div>
  );
}

/** Space around the drawing. */
const PAD = 14;
/** The participant row at the top, and where the lifelines start. */
const HEAD_H = 30;
/** Between the participant row and the first arrow. */
const HEAD_GAP = 20;
/** One message. */
const ROW_H = 38;
/** How far a self-call reaches to the right of its own lane, and drops. */
const SELF_W = 26;
const SELF_DROP = 20;
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

export type Row = {
  y: number;
  /** Lane centres, in pixels. Equal for a call a participant makes to itself. */
  from: number;
  to: number;
  self: boolean;
  kind: SequenceMessage["kind"];
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

/**
 * Where every lane and arrow goes.
 *
 * Separate from the drawing so it can be tested as arithmetic, which is all it
 * is. The one thing worth stating: a label is centred over its own arrow and
 * each message owns a row, so two labels can never collide with one another —
 * only with the edges of the box. That is what the extent pass at the end is
 * for, and why the viewBox can start left of zero.
 */
export function layout(view: SequenceView): Layout {
  const { participants, messages } = view;

  const widest = participants.reduce(
    (max, name) => Math.max(max, textWidth(name, NAME_SIZE)),
    0,
  );
  const laneWidth = Math.max(LANE_MIN, widest + 34);
  const centre = (i: number) => PAD + laneWidth / 2 + i * laneWidth;

  const lanes = participants.map((label, i) => ({ x: centre(i), label }));
  const index = new Map(participants.map((name, i) => [name, i]));

  let y = HEAD_H + HEAD_GAP;
  const rows: Row[] = [];
  for (const message of messages) {
    const from = index.get(message.from);
    const to = index.get(message.to);
    // Unreachable through the tool, which refuses an arrow naming a lane it
    // never declared. A transcript hand-edited past that check drops the row
    // rather than drawing an arrow from nowhere.
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

/**
 * The arrowhead, as a filled triangle.
 *
 * A path per arrow rather than one `<marker>` referenced by all of them:
 * markers are addressed by document id, and two diagrams in one transcript
 * would either collide on the name or need one generated per card.
 */
function head(x: number, y: number, direction: 1 | -1): string {
  const back = x + direction * 7;
  return `M ${x} ${y} L ${back} ${y - 4} L ${back} ${y + 4} Z`;
}

/**
 * What a screen reader is told, since the picture itself says nothing to one.
 *
 * The order of events in a sentence per arrow, which is the same information
 * the drawing carries and the form it was in before it was drawn.
 */
export function described(view: SequenceView): string {
  const steps = view.messages.map((m) =>
    m.from === m.to
      ? `${m.from} ${m.text}`
      : `${m.from} ${m.kind === "return" ? "returns to" : "to"} ${m.to}: ${m.text}`,
  );
  return `Sequence diagram, ${view.title}. ${steps.join(". ")}.`;
}

/**
 * The same diagram as Mermaid source.
 *
 * Participants are aliased to `p0`, `p1` … rather than named directly: Mermaid
 * splits a message line on its arrow, so a participant with a space in its name
 * — `Order Store` — produces a diagram that either fails to parse or quietly
 * invents a lane. The alias makes every name safe without asking the model to
 * avoid spaces.
 */
export function mermaid(view: SequenceView): string {
  const alias = new Map(view.participants.map((name, i) => [name, `p${i}`]));
  const lines = ["sequenceDiagram"];
  for (const [name, id] of alias) {
    lines.push(`    participant ${id} as ${name}`);
  }
  for (const message of view.messages) {
    const from = alias.get(message.from);
    const to = alias.get(message.to);
    if (!from || !to) continue;
    const arrow = message.kind === "return" ? "-->>" : "->>";
    lines.push(`    ${from}${arrow}${to}: ${message.text}`);
  }
  return lines.join("\n");
}

function plural(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}
