import { useEffect, useRef } from "react";

import { isComposing } from "../lib/keys";

/** The box over a file: a word to find, or `:120` to go to a line. */
export function FindBar({
  query,
  said,
  asked,
  onQuery,
  onMove,
  onSubmit,
  onClose,
}: {
  query: string;
  /** Where the search has got to, like `3 of 12`. */
  said: string | null;
  /** Bumped each time a key asks for the box, so it takes focus again. */
  asked: number;
  onQuery: (query: string) => void;
  onMove: (by: 1 | -1) => void;
  /** Enter on a line number. */
  onSubmit: () => boolean;
  /** Back to the file. */
  onClose: () => void;
}) {
  const box = useRef<HTMLInputElement>(null);
  useEffect(() => {
    box.current?.focus();
    box.current?.setSelectionRange(query === ":" ? 1 : 0, query.length);
    // Only when asked. On `query` it would select what's being typed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [asked]);

  return (
    <div className="doc-find" role="search">
      <input
        ref={box}
        className="doc-find-input"
        value={query}
        placeholder="Find, or :line"
        aria-label="Find in this file, or type a colon and a line number"
        spellCheck={false}
        onChange={(e) => onQuery(e.target.value)}
        onKeyDown={(e) => {
          if (isComposing(e)) return;
          if (e.key === "Escape") {
            // Marked, so the canvas doesn't read it as closing the file too.
            e.preventDefault();
            onClose();
          } else if (e.key === "Enter") {
            e.preventDefault();
            if (!onSubmit()) onMove(e.shiftKey ? -1 : 1);
          }
        }}
      />
      {said && (
        <span className="doc-find-count" role="status">
          {said}
        </span>
      )}
      <button className="doc-find-button" aria-label="Previous match" onClick={() => onMove(-1)}>
        ↑
      </button>
      <button className="doc-find-button" aria-label="Next match" onClick={() => onMove(1)}>
        ↓
      </button>
      <button className="doc-find-button" aria-label="Close find" onClick={onClose}>
        ✕
      </button>
    </div>
  );
}
