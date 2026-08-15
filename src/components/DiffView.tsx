import type { FileDiff } from "../lib/api";

/**
 * The change a write would make, drawn as a diff.
 *
 * Shown at the moment of approval, which is the moment it matters: a byte count
 * says a file is about to be replaced and nothing about what with, and for an
 * overwrite that is the whole decision.
 *
 * The gutter is `+` and `-` rather than colour alone. Colour is the fast read,
 * but it is the one that fails on a projector, in a screenshot, and for a
 * reader who cannot distinguish red from green — and this is not a view where
 * the fallback can be "look more carefully".
 */
export function DiffView({ diff }: { diff: FileDiff }) {
  const empty = diff.added === 0 && diff.removed === 0;

  return (
    <div className="diff">
      <div className="diff-head">
        <span className="diff-verb">{diff.created ? "create" : "replace"}</span>
        <span className="diff-path">{diff.path}</span>
        {!empty && (
          <span className="diff-stat">
            <span className="added">+{diff.added}</span>
            <span className="removed">−{diff.removed}</span>
          </span>
        )}
      </div>

      {/* A write that changes nothing is usually a model looping. Saying so is
          more use than an empty frame, and it is a decision worth not making. */}
      {empty ? (
        <p className="diff-note">This would leave the file exactly as it is.</p>
      ) : (
        <div className="diff-body" role="table" aria-label={`Changes to ${diff.path}`}>
          {diff.hunks.map((hunk, h) => (
            <div className="diff-hunk" key={h}>
              {hunk.lines.map((line, i) => (
                <div className={`diff-line ${line.kind}`} key={i} role="row">
                  {/* Numbers are the file's own, so one read off this dialog
                      means the same thing as one read off `read_file`. */}
                  <span className="diff-num" aria-hidden="true">
                    {line.old_line ?? ""}
                  </span>
                  <span className="diff-num" aria-hidden="true">
                    {line.new_line ?? ""}
                  </span>
                  <span className="diff-gutter" aria-hidden="true">
                    {GUTTER[line.kind]}
                  </span>
                  <span className="diff-text">{line.text || " "}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      )}

      {/* A cut that does not announce itself reads as the whole change, which
          is the one thing a permission prompt must never do. */}
      {diff.elided > 0 && (
        <p className="diff-note">
          {diff.elided} more {diff.elided === 1 ? "line" : "lines"} not shown.
        </p>
      )}
    </div>
  );
}

const GUTTER: Record<string, string> = {
  added: "+",
  removed: "-",
  context: " ",
};
