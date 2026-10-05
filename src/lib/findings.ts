/**
 * A review, cut into the things it found.
 *
 * The reviewer is asked to give each finding as its own top-level numbered
 * item, complete on its own (see `BRIEF` in `taurus_host::review`), so each
 * one can be handed on by itself: forked from, as the brief for another
 * attempt at the turn it's about. This finds those items again in the
 * Markdown. A model that answers in some other shape gets no items, and the
 * drawer offers the whole review instead, so nothing here has to guess.
 */

/** A stretch of a review: one finding, or the prose around them. */
export type ReviewPart = { text: string; finding: boolean };

/** A bullet or an ordered marker at the margin, which starts a finding. */
const ITEM = /^([-+*]|\d{1,9}[.)])(\s|$)/;
/** A code fence's opening or closing run. */
const FENCE = /^\s*(`{3,}|~{3,})/;

/**
 * Cuts `text` at each top-level list item.
 *
 * An item runs on through its indented lines, its blank lines, and any line
 * that carries its paragraph on with no blank before it. It ends at the next
 * item, and at a line back at the margin after a blank one, which is prose
 * again. Nothing inside a code fence starts or ends anything.
 */
export function reviewParts(text: string): ReviewPart[] {
  const parts: ReviewPart[] = [];
  let fence: string | null = null;
  let afterBlank = false;

  for (const line of text.split("\n")) {
    const blank = line.trim() === "";
    const marker = FENCE.exec(line)?.[1];
    const current = parts.at(-1);
    if (fence) {
      if (marker && marker[0] === fence[0] && marker.length >= fence.length) fence = null;
    } else {
      if (ITEM.test(line)) {
        parts.push({ text: "", finding: true });
      } else if (!current || (current.finding && !blank && afterBlank && !/^\s/.test(line))) {
        // Prose: the start, or back at the margin after a blank line. A
        // finding's own continuation is indented, or follows on directly.
        parts.push({ text: "", finding: false });
      }
      if (marker) fence = marker;
    }
    parts[parts.length - 1].text += `${line}\n`;
    afterBlank = blank;
  }

  return parts
    .map((part) => ({ ...part, text: part.text.trim() }))
    .filter((part) => part.text !== "");
}

/** Whether a review has findings to fork from one at a time. */
export function hasFindings(parts: ReviewPart[]): boolean {
  return parts.some((part) => part.finding);
}

/**
 * The question a fork from a finding asks: the turn's own, with what the
 * review found added after it.
 *
 * Said as a review of an earlier attempt, because in the fork that's what it
 * is. The attempt it found fault with isn't in the fork's transcript, so a
 * finding that read as if it were would send the model looking for it. Put
 * in the composer rather than sent, like every fork's question, to be edited
 * first.
 */
export function withFinding(prompt: string, finding: string): string {
  const said = finding.replace(/^([-+*]|\d{1,9}[.)])\s+/, "").trim();
  return `${prompt.trim()}\n\nA review of an earlier attempt at this found the following. Take it into account:\n\n${said}`;
}
