import type { ReactNode, Ref } from "react";

import type { Ink } from "../lib/ink";

/**
 * The painted layer under a textarea: the same characters, coloured.
 *
 * Shared by the three editors that paint — SQL, a file on the canvas, a note.
 * Each wrote this out, and the one bug the layer has was found three separate
 * times: a `<pre>` swallows one trailing newline, so text ending in one paints
 * a line short and the caret sits below its own text. The newline after the
 * runs is that fix, here once.
 *
 * Hidden from a screen reader, because the textarea over it holds the same
 * characters and a reader that saw both would read everything twice.
 * `before` and `after` are for the canvas, which paints only a window of a
 * long file and stands the rest in as height.
 */
export function InkLayer({
  runs,
  className,
  ghost,
  before,
  after,
}: {
  runs: Ink[];
  className: string;
  /** For an editor that keeps the layer's scroll in step with the box. */
  ghost?: Ref<HTMLPreElement>;
  before?: ReactNode;
  after?: ReactNode;
}) {
  return (
    <pre className={className} aria-hidden="true" ref={ghost}>
      {before}
      {runs.map((run, i) => (
        <span key={i} className={`ink-${run.kind}`}>
          {run.text}
        </span>
      ))}
      {"\n"}
      {after}
    </pre>
  );
}

/**
 * Makes a textarea exactly as tall as what is in it, at the width it has now.
 *
 * `auto` first, and that is the whole trick: `scrollHeight` on a box already
 * tall enough reports the height it has, so measuring without collapsing it
 * makes the box grow and never shrink. The floor and ceiling, where there are
 * any, are the stylesheet's.
 */
export function growToContent(el: HTMLTextAreaElement): void {
  el.style.height = "auto";
  el.style.height = `${el.scrollHeight}px`;
}
