import { useMemo, useState } from "react";

import { describe as describeFlow, plan as planFlow } from "../lib/flow";
import { describe as describeSequence, plan as planSequence } from "../lib/sequence";
import { read } from "../lib/mermaid";
import { CopyButton } from "./CopyButton";
import { FlowDiagram } from "./FlowDiagram";
import { SequenceDiagram } from "./SequenceDiagram";

/**
 * A ```mermaid fence, drawn where it can be.
 *
 * The reading is `lib/mermaid` and the drawing is the two engines that already
 * draw these diagrams for `show_flow` and `show_sequence`. What is left here is
 * the one decision a fence has that a tool call does not: what to show when the
 * source is not something the reader can draw.
 *
 * It shows the source, and says why, in that order of prominence. A fence is
 * text somebody wrote, and the text is never wrong — so the failure case is not
 * an error state, it is the same block with a sentence over it. The sentence
 * names the diagram type or the line, because "could not render" tells a person
 * nothing they can act on.
 *
 * The **source** toggle is on the drawable case too. A diagram is easier to read
 * than its source and harder to check, and the fence is the thing being edited
 * in a note.
 */
export function MermaidBlock({
  source,
  /**
   * Whether the text is still arriving.
   *
   * Half a diagram is not a diagram, and a fence that is three lines into being
   * written parses as a refusal — so while a turn streams, this is the painted
   * source it was before, and it becomes a picture when the text stops moving.
   * Nothing flickers, and nobody reads "line 4 is not something this reads yet"
   * about a line that is not finished.
   */
  streaming,
}: {
  source: string;
  streaming: boolean;
}) {
  const [showing, setShowing] = useState<"diagram" | "source">("diagram");
  const text = source.replace(/\n$/, "");
  const got = useMemo(() => (streaming ? null : read(text)), [text, streaming]);

  const drawable =
    got !== null && got.kind !== "refused" ? got : null;

  // Laid out once per source rather than once per render. The source toggle
  // re-renders this without moving a line of the diagram.
  const layout = useMemo(() => {
    if (!drawable) return null;
    return drawable.kind === "flow"
      ? {
          kind: "flow" as const,
          plan: planFlow(drawable.input),
          label: describeFlow(drawable.input),
        }
      : {
          kind: "sequence" as const,
          plan: planSequence(drawable.input),
          label: describeSequence(drawable.input),
        };
  }, [drawable]);

  return (
    <div className="md-code">
      <div className="md-code-head">
        <span className="md-code-lang">mermaid</span>
        {drawable && (
          <button
            className="md-copy"
            onClick={() => setShowing(showing === "source" ? "diagram" : "source")}
          >
            {showing === "source" ? "diagram" : "source"}
          </button>
        )}
        <CopyButton className="md-copy" text={text} />
      </div>

      {got?.kind === "refused" && <p className="md-mermaid-note">{got.why}</p>}

      {drawable === null || layout === null || showing === "source" ? (
        <pre>
          <code>{text}</code>
        </pre>
      ) : (
        <div className="md-mermaid">
          {layout.kind === "flow" ? (
            <FlowDiagram plan={layout.plan} label={layout.label} />
          ) : (
            <SequenceDiagram plan={layout.plan} label={layout.label} />
          )}
          {drawable.skipped.length > 0 && (
            // Under the picture rather than over it: what is here is a footnote
            // about a diagram that drew, not a warning about one that did not.
            <p className="md-mermaid-note">{drawable.skipped.join(". ")}.</p>
          )}
        </div>
      )}
    </div>
  );
}
