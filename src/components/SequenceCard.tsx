import { CopyButton } from "./CopyButton";
import { SequenceDiagram } from "./SequenceDiagram";
import { describe, fromView, mermaid, plan } from "../lib/sequence";
import type { TranscriptView } from "../lib/api";

type SequenceView = Extract<TranscriptView, { type: "sequence" }>;

/**
 * A `show_sequence` result.
 *
 * Drawn rather than described, because the thing a sequence diagram is for —
 * seeing where a chain of calls turns around, and which lane the work is
 * actually happening in — is not something a numbered list can show.
 *
 * The card is the head, the foot and the copy button; the picture is
 * [`SequenceDiagram`] and the arithmetic is `lib/sequence`, because a ```mermaid
 * fence in a note draws the same diagram with none of this around it.
 */
export function SequenceCard({ view }: { view: SequenceView }) {
  const { title, caption, participants } = view;
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
        <span className="micro">{plural(view.messages.length, "message")}</span>
      </div>

      <SequenceDiagram plan={layout} label={describe(input)} />

      <div className="view-foot">
        <span className="micro">{plural(participants.length, "participant")}</span>
        <div className="spacer" />
        {/* Mermaid because it is what a diagram gets pasted into — a README, a
            design doc, an issue. The app does not depend on it to draw this;
            it reads and writes it either way. */}
        <CopyButton
          className="pill"
          label="copy as Mermaid"
          text={() => mermaid(input)}
        />
      </div>
    </div>
  );
}

function plural(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}
