import { useMemo } from "react";

import type { DiffLine, FileDiff } from "../lib/api";
import { grammarFor, mark, paint } from "../lib/ink";
import { pairs, refine } from "../lib/intraline";

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
 *
 * Two things narrow the read further, and both address the same complaint: a
 * line diff tells you a line was replaced and leaves finding the difference to
 * you. The text is coloured by the file's own language, so a change to a
 * string does not have to be told apart from a change to a name by reading
 * both. And where a removal and an addition are the same line before and
 * after, the part that actually differs is marked inside them — see
 * `intraline.ts`, which includes the cases where it declines to guess.
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
            <Hunk key={h} lines={hunk.lines} grammar={grammarFor(diff.path)} />
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

/**
 * One run of lines, painted and marked together.
 *
 * Together because neither answer is a property of a line on its own: which
 * characters changed is a fact about a *pair* of lines, and the pairing is
 * read off the run's shape. Doing it a hunk at a time is also what keeps the
 * work proportional to what is on screen — the Changes drawer draws a diff per
 * file, and the permission dialog draws one while a turn waits on it.
 */
function Hunk({ lines, grammar }: { lines: DiffLine[]; grammar: string | null }) {
  const painted = useMemo(() => {
    const paired = pairs(lines.map((line) => line.kind));
    const spans = new Map<number, { from: number; to: number }>();
    for (const [i, j] of paired) {
      // Once per pair rather than once per line. The map records the pairing
      // from both directions, and refining the same two lines a second time
      // would give the same answer at twice the price.
      if (lines[i].kind !== "removed") continue;
      const found = refine(lines[i].text, lines[j].text);
      if (!found) continue;
      spans.set(i, { from: found.oldFrom, to: found.oldTo });
      spans.set(j, { from: found.newFrom, to: found.newTo });
    }
    return lines.map((line, i) => mark(paint(line.text, grammar), spans.get(i) ?? null));
  }, [lines, grammar]);

  return (
    <div className="diff-hunk">
      {lines.map((line, i) => (
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
          {/* A blank line still has to fill its row, or the grid closes it up
              and the numbers stop lining up with the file. */}
          <span className="diff-text">
            {line.text
              ? painted[i].map((run, r) => (
                  <span
                    key={r}
                    className={`ink-${run.kind}${run.changed ? " ink-changed" : ""}`}
                  >
                    {run.text}
                  </span>
                ))
              : " "}
          </span>
        </div>
      ))}
    </div>
  );
}
