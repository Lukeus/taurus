import type { FileDiff } from "../lib/api";

/**
 * A change to one file, drawn as a diff.
 *
 * Shown at the moment of approval, which is the moment it matters: a byte count
 * says a file is about to be replaced and nothing about what with, and for an
 * overwrite that is the whole decision. The Changes drawer reuses it after the
 * fact, for a turn already made — the same rendering, so a diff read before
 * approving and a diff read a week later cannot disagree about what happened.
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
        <span className="diff-verb">{VERB(diff)}</span>
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
        <p className="diff-note">This leaves the file exactly as it is.</p>
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

/**
 * What happened to the file, in one word.
 *
 * `delete` only ever comes from a recorded change — a write cannot remove a
 * file — and it has to be said, because an all-removed diff otherwise reads
 * identically to a file truncated to nothing.
 */
function VERB(diff: FileDiff): string {
  if (diff.deleted) return "delete";
  return diff.created ? "create" : "replace";
}

const GUTTER: Record<string, string> = {
  added: "+",
  removed: "-",
  context: " ",
};
