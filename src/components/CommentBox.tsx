import { useState } from "react";

/**
 * Where a comment is written: on a line of a turn's diff, or on a step of a
 * plan.
 *
 * Enter adds it and Shift+Enter starts a new line, the same keys as the
 * composer it is going to. Escape and Cancel both drop what was typed: it has
 * not gone anywhere yet, and there is nothing to lose but the sentence.
 *
 * Escape stops here rather than also closing whatever holds the box — the plan
 * panel's editor, a drawer — because the person pressing it is talking to the
 * box.
 */
export function CommentBox({
  onSave,
  onCancel,
}: {
  onSave: (text: string) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState("");
  const save = () => {
    if (text.trim()) onSave(text.trim());
  };
  return (
    <div className="comment-box">
      <textarea
        autoFocus
        rows={2}
        value={text}
        placeholder="Your comment goes with your next message"
        aria-label="Comment"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onCancel();
          } else if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            save();
          }
        }}
      />
      <div className="comment-actions">
        <button className="quiet" onClick={onCancel}>
          Cancel
        </button>
        <button onClick={save} disabled={!text.trim()}>
          Add comment
        </button>
      </div>
    </div>
  );
}
