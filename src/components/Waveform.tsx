import { memo, useEffect, useRef } from "react";

import { useReducedMotion } from "../lib/motion";

/**
 * The shape of the work, as a row of bars.
 *
 * From the motion spec's first state: *"a live waveform that changes character
 * with each phase — traveling wave while reasoning, center ripple while
 * composing, scattered ticks while tracing, a sweeping peak while scanning. the
 * motion tells you what kind of work is happening."*
 *
 * The mockup cycles those four on a timer, because a mockup has no agent. This
 * does not: the app knows what is running, so the shape is chosen by the *tool
 * category the harness already assigns* — see `waveFor`. A reader who learns
 * the four shapes can tell reading from writing without reading a word, which
 * is the whole claim the spec makes for this and the only reason it beats a
 * spinner.
 *
 * # Why this is a rAF loop and not CSS
 *
 * Twelve bars each on their own keyframes would be twelve animations to keep in
 * phase, and the shapes are functions of position *and* time — a travelling
 * wave is a phase offset per bar, which in CSS is twelve hand-written delays
 * that stop being right the moment the bar count changes. One loop writing
 * `transform` is cheaper than that and exact.
 *
 * It costs one `requestAnimationFrame` while a turn is running and nothing at
 * all otherwise: the component is unmounted between turns, and the browser does
 * not run frames for a hidden window.
 *
 * Memoized, and that matters more than it looks. This sits inside the turn that
 * is streaming, so its parent re-renders on every token — and each of those
 * would otherwise reconcile eight spans for a component whose output has not
 * changed. All three props are primitives, so the default comparison is exactly
 * right: it re-renders when the agent moves to a different kind of work, and
 * not once in between.
 */
export const Waveform = memo(function Waveform({
  mode,
  bars = 10,
  className,
}: {
  mode: Wave;
  bars?: number;
  className?: string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const reduced = useReducedMotion();

  /*
   * The current shape, read through a ref rather than closed over.
   *
   * The effect must not restart when the mode changes — restarting resets `t`,
   * and the wave would jump back to its first frame every time the agent moved
   * from one tool to the next. Reading it out of a ref each frame lets the
   * shape change under a clock that keeps running.
   */
  const shape = useRef(mode);
  shape.current = mode;

  useEffect(() => {
    // Not started at all, rather than started and hidden. This is the one
    // animation in the app that is script rather than CSS, so the `@media`
    // block that switches everything else off cannot reach it.
    if (reduced) return;
    const start = performance.now();
    let frame = requestAnimationFrame(function draw(now) {
      const t = (now - start) / 1000;
      const row = box.current;
      if (row) {
        const of = SHAPES[shape.current];
        const n = row.children.length;
        for (let x = 0; x < n; x++) {
          const v = Math.max(0.08, Math.min(1, of(t, x, n)));
          const bar = row.children[x] as HTMLElement;
          bar.style.transform = `scaleY(${v.toFixed(3)})`;
          // Height and weight together. A bar that only got shorter read as a
          // bar chart; fading the short ones makes it read as a signal.
          bar.style.opacity = (0.45 + 0.55 * v).toFixed(2);
        }
      }
      frame = requestAnimationFrame(draw);
    });
    return () => cancelAnimationFrame(frame);
  }, [reduced]);

  return (
    <div className={`waveform${className ? ` ${className}` : ""}`} aria-hidden ref={box}>
      {Array.from({ length: bars }, (_, i) => (
        // Still, and deliberately not flat, when motion is off: a row of even
        // ticks says "something is here" without claiming to be measuring it.
        <span key={i} style={reduced ? { transform: "scaleY(0.5)" } : undefined} />
      ))}
    </div>
  );
});

/** Which of the four shapes a row of bars is drawing. */
export type Wave = "wave" | "ripple" | "ticks" | "peak";

/*
 * The four, as the motion spec wrote them — copied rather than re-derived,
 * because the constants are the design and there is nothing to improve about
 * `12.9898` except to get it wrong.
 *
 * `t` is seconds since the loop started, `x` is the bar's index, `n` the count.
 * All four return roughly 0…1 and are clamped by the caller.
 */
const SHAPES: Record<Wave, (t: number, x: number, n: number) => number> = {
  /** A phase offset per bar: the wave walks along the row. */
  wave: (t, x) => 0.5 + 0.42 * Math.sin(t * 3.2 - x * 0.9),
  /** Peaks in the middle and falls away to both ends. */
  ripple: (t, x, n) =>
    0.3 + 0.65 * Math.max(0, Math.sin(t * 2.4 - Math.abs(x - n / 2) * 0.7)),
  /** Deterministic noise — the same row every time, and not a pattern. */
  ticks: (t, x) =>
    0.25 +
    0.6 * Math.abs(Math.sin(t * 1.7 + x * 12.9898) * Math.sin(t * 0.9 + x * 78.233)),
  /** A single peak sweeping across, like a scan line laid on its side. */
  peak: (t, x, n) =>
    0.2 + 0.75 * Math.exp(-Math.pow(x - (((t * 2.2) % (n + 4)) - 2), 2) / 2.2),
};

/**
 * Which shape belongs to a category of work.
 *
 * The mapping is the adaptation, and it is what makes the waveform worth more
 * than a spinner: the spec's four verbs — reasoning, composing, tracing,
 * scanning — line up with categories the harness already assigns to every tool
 * call, so the shape is a fact about the turn rather than a timer.
 *
 * `null` is a turn with no tool running, which is the model thinking. That gets
 * the travelling wave, the spec's own reasoning shape.
 */
export function waveFor(category: string | null): Wave {
  switch (category) {
    // Reading a file *is* a scan, and the spec draws scanning as a sweeping
    // peak — the same motion as the band that sweeps a buffer in state 02.
    case "read":
      return "peak";
    // Composing. The ripple starts in the middle and pushes out, which is the
    // one shape that reads as something being produced rather than traversed.
    case "wrote":
    case "kept":
      return "ripple";
    // A command's output arrives in bursts nobody can predict, and scattered
    // ticks are the only shape here that is not periodic.
    case "ran":
    case "skill":
      return "ticks";
    default:
      return "wave";
  }
}
