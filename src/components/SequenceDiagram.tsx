import { HEAD_H, PAD, SELF_DROP, SELF_W, head, type Layout } from "../lib/sequence";

/**
 * A sequence diagram, drawn.
 *
 * Nothing but the picture — no head, no caption, no copy button. Both things
 * that draw one hand it a plan from [`plan`] and the sentence a screen reader
 * gets: a `show_sequence` card in the transcript, and a ```mermaid fence in a
 * note.
 *
 * Hand-drawn SVG for the reason the chart is hand-drawn divs. A diagram library
 * would be the largest dependency in the app by two orders of magnitude, would
 * arrive with its own palette to fight, and could not be checked before it was
 * rendered — where this payload is refused by the tool if an arrow names a lane
 * that was never declared.
 */
export function SequenceDiagram({
  plan,
  label,
}: {
  plan: Layout;
  /** What a screen reader is told, since the picture says nothing to one. */
  label: string;
}) {
  return (
    // Scrolls rather than scales. A diagram squeezed to fit a narrow window
    // shrinks its labels out of legibility, and the labels are the half of it
    // that carries the meaning.
    <div className="sequence-box">
      <svg
        className="sequence"
        width={plan.width}
        height={plan.height}
        viewBox={`${plan.left} 0 ${plan.width} ${plan.height}`}
        role="img"
        aria-label={label}
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
  );
}
