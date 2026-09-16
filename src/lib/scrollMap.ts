/**
 * Where somebody is in a note, carried from one way of showing it to the other.
 *
 * Write and Read show the same note at different heights: a Mermaid fence is a
 * dozen short lines in one and a picture in the other, a sketch embed is one
 * line and a drawing. So the same scroll offset is a different place in each,
 * and even the same *fraction* of the way down drifts by a screen or more
 * across a note with one big diagram in it.
 *
 * What both views do share is the headings — the same ones, in the same order.
 * So a position is kept as "this far between heading 3 and heading 4" and put
 * back between the same two headings on the other side. Before the first
 * heading and after the last, the top and the end of the content stand in.
 * The DOM work of finding where each heading sits is the pane's; this is the
 * arithmetic, and it is pure so it can be tested.
 *
 * When the two sides do not have the same number of landmarks — an underlined
 * heading, a heading inside a quote, which one side counts and the other does
 * not — lining them up would put somebody under the wrong heading. The
 * position falls back to the fraction of the way down, which is vaguer and
 * never wrong about which section it is in by more than the drift.
 */

/** A position, as the landmarks around it see it. */
export type Spot = {
  /** Which gap between landmarks the position is in. */
  index: number;
  /** How far across that gap, from 0 to 1. */
  t: number;
  /** How many landmarks it was measured against — a spot only means the same
   *  thing on a side with the same number. */
  of: number;
  /** The fraction of the way down, for when it does not. */
  ratio: number;
  /** Scrolled to the very end, which stays the end whatever the heights. */
  end: boolean;
};

/**
 * Where offset `y` is among `landmarks`.
 *
 * `landmarks` run top to bottom, starting at 0 and ending at the content's
 * height; `max` is how far the box can scroll.
 */
export function locate(y: number, landmarks: readonly number[], max: number): Spot {
  const ratio = max > 0 ? Math.min(1, Math.max(0, y / max)) : 0;
  const end = max > 0 && y >= max - 1;
  if (!ordered(landmarks)) return { index: 0, t: 0, of: 0, ratio, end };
  let index = 0;
  while (index < landmarks.length - 2 && y >= landmarks[index + 1]) index++;
  const from = landmarks[index];
  const span = landmarks[index + 1] - from;
  const t = span > 0 ? Math.min(1, Math.max(0, (y - from) / span)) : 0;
  return { index, t, of: landmarks.length, ratio, end };
}

/** The offset on another side that is the same place as `spot`. */
export function place(spot: Spot, landmarks: readonly number[], max: number): number {
  if (max <= 0) return 0;
  if (spot.end) return max;
  if (spot.of !== landmarks.length || !ordered(landmarks)) return spot.ratio * max;
  const from = landmarks[spot.index];
  const y = from + spot.t * (landmarks[spot.index + 1] - from);
  return Math.min(max, Math.max(0, y));
}

/** At least a top and an end, never going back up. */
function ordered(landmarks: readonly number[]): boolean {
  if (landmarks.length < 2) return false;
  for (let i = 1; i < landmarks.length; i++) {
    if (!(landmarks[i] >= landmarks[i - 1])) return false;
  }
  return true;
}
