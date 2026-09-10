import { STAGE_LABEL_Y, type FlowLayout } from "../lib/flow";

/**
 * A flow diagram, drawn.
 *
 * Nothing but the picture — no head, no caption, no copy button. Two things
 * draw one: a `show_flow` card in the transcript, which surrounds it with the
 * title it was given and the Mermaid it can be copied as, and a ```mermaid fence
 * in a note, which has a fence's own chrome instead. Both hand it a plan from
 * [`plan`] and the sentence a screen reader gets, because neither the geometry
 * nor the description depends on which of them is asking.
 */
export function FlowDiagram({
  plan,
  label,
}: {
  plan: FlowLayout;
  /** What a screen reader is told, since the picture says nothing to one. */
  label: string;
}) {
  return (
    // Scrolls rather than scales. A diagram squeezed to fit a narrow window
    // shrinks its labels out of legibility, and the labels are the half of it
    // that carries the meaning.
    <div className="flow-box">
      <svg
        className="flow"
        width={plan.width}
        height={plan.height}
        viewBox={`0 0 ${plan.width} ${plan.height}`}
        role="img"
        aria-label={label}
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
            <g key={box.key} className="flow-node">
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
  );
}
