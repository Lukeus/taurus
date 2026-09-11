import { CopyButton } from "./CopyButton";
import { FlowDiagram } from "./FlowDiagram";
import { describe, fromView, mermaid, plan } from "../lib/flow";
import type { TranscriptView } from "../lib/api";
import { plural } from "../lib/format";

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
 * for the stages, everything in [`plan`] is arithmetic.
 *
 * The card is the head, the foot and the copy button. The picture is
 * [`FlowDiagram`] and the geometry is `lib/flow`, because a ```mermaid fence in
 * a note draws the same diagram with none of this around it.
 */
export function FlowCard({ view }: { view: FlowView }) {
  const { title, caption, stages, edges } = view;
  const input = fromView(view);
  const layout = plan(input);

  return (
    <div className="view-card">
      <div className="view-head">
        <div>
          <h3>{title}</h3>
          {caption && <p className="view-caption">{caption}</p>}
        </div>
        <div className="spacer" />
        <span className="micro">{plural(layout.boxes.length, "node")}</span>
      </div>

      <FlowDiagram plan={layout} label={describe(input)} />

      <div className="view-foot">
        <span className="micro">
          {plural(stages.length, "stage")} · {plural(edges.length, "edge")}
        </span>
        <div className="spacer" />
        <CopyButton
          className="pill"
          label="copy as Mermaid"
          text={() => mermaid(input)}
        />
      </div>
    </div>
  );
}
