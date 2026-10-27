import type { KeyboardEvent } from "react";

/**
 * The arrow keys between the tabs of a tablist.
 *
 * Every tablist a screen reader has met moves with Left and Right and jumps
 * with Home and End, and announces a row of tabs as exactly that. A `role`
 * without the keys is a promise the control does not keep.
 *
 * Goes on the element with `role="tablist"`. Moving selects, the way the app's
 * switchers already behave on a click, and a disabled tab is stepped over
 * rather than landed on. The tabs themselves carry a roving `tabIndex` — 0 on
 * the selected one, -1 on the rest — so Tab reaches the row once and the
 * arrows do the rest.
 *
 * A `radiogroup` of pill buttons makes the same promise and keeps it the same
 * way, so this goes on those too: it moves among `role="radio"` as readily as
 * among `role="tab"`.
 *
 * A list with `aria-orientation="vertical"` moves with Up and Down instead,
 * which is what a screen reader tells someone to press for one.
 */
export function onTabKeys(e: KeyboardEvent<HTMLElement>): void {
  const tabs = [
    ...e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"], [role="radio"]'),
  ].filter((tab) => !tab.disabled);
  const at = tabs.indexOf(document.activeElement as HTMLButtonElement);
  if (at < 0 || tabs.length === 0) return;
  const vertical = e.currentTarget.getAttribute("aria-orientation") === "vertical";
  const next = vertical ? "ArrowDown" : "ArrowRight";
  const previous = vertical ? "ArrowUp" : "ArrowLeft";
  const to =
    e.key === next
      ? (at + 1) % tabs.length
      : e.key === previous
        ? (at - 1 + tabs.length) % tabs.length
        : e.key === "Home"
          ? 0
          : e.key === "End"
            ? tabs.length - 1
            : null;
  if (to === null) return;
  e.preventDefault();
  tabs[to].focus();
  tabs[to].click();
}
