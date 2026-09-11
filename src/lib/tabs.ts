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
 */
export function onTabKeys(e: KeyboardEvent<HTMLElement>): void {
  const tabs = [
    ...e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]'),
  ].filter((tab) => !tab.disabled);
  const at = tabs.indexOf(document.activeElement as HTMLButtonElement);
  if (at < 0 || tabs.length === 0) return;
  const to =
    e.key === "ArrowRight"
      ? (at + 1) % tabs.length
      : e.key === "ArrowLeft"
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
