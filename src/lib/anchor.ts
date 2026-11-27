import type { CSSProperties } from "react";

/**
 * Where a popup opened from `trigger` goes: under it, unless there is more room
 * above, and pulled in from the right edge so it never opens off-screen.
 *
 * Shared by `Select` and `Menu`, which both portal their popup to the body (so
 * a drawer's `overflow: hidden` cannot clip it) and so both have to place it
 * against the trigger themselves. `align: "end"` lines the popup's right edge
 * up with the trigger's, for a ⋯ button at the end of a row.
 */
export function anchorTo(
  trigger: HTMLElement,
  popupHeight: number,
  { minWidth = 180, align = "start" }: { minWidth?: number; align?: "start" | "end" } = {},
): { style: CSSProperties; up: boolean } {
  const box = trigger.getBoundingClientRect();
  const below = window.innerHeight - box.bottom;
  const up = below < popupHeight + 8 && box.top > below;
  const width = Math.max(box.width, minWidth);
  const left =
    align === "end"
      ? Math.max(8, box.right - width)
      : Math.min(box.left, window.innerWidth - width - 8);
  return {
    up,
    style: {
      left,
      minWidth: align === "end" ? minWidth : box.width,
      ...(up ? { bottom: window.innerHeight - box.top + 4 } : { top: box.bottom + 4 }),
    },
  };
}
