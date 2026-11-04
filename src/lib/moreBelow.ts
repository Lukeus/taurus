import { useEffect, type RefObject } from "react";

/**
 * Marks a scrolling box with `data-more` while there is anything below what it
 * shows, so its CSS can fade the bottom edge.
 *
 * A Mac hides a scrollbar until somebody scrolls, so a box that has run out of
 * room looks the same as one that has shown everything: in the rail at 840px,
 * the Activity fold's last row sat under the footer and read as not being
 * there at all. An attribute rather than state, as the code editor's
 * sideways fade is, so scrolling re-renders nothing.
 *
 * Checked on scroll, on a resize of the box, and on a change to what is in it,
 * since a fold opening adds height with no scroll to say so — and again when
 * an animation inside it ends, because an unfolding fold is still short when
 * its rows are added.
 */
export function useMoreBelow(ref: RefObject<HTMLElement | null>) {
  useEffect(() => {
    const box = ref.current;
    if (!box) return;
    const check = () => {
      const more = box.scrollHeight - box.scrollTop - box.clientHeight > 1;
      box.toggleAttribute("data-more", more);
    };
    check();
    box.addEventListener("scroll", check, { passive: true });
    box.addEventListener("animationend", check);
    box.addEventListener("transitionend", check);
    const size = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(check);
    size?.observe(box);
    const content = typeof MutationObserver === "undefined" ? null : new MutationObserver(check);
    content?.observe(box, { childList: true, subtree: true });
    return () => {
      box.removeEventListener("scroll", check);
      box.removeEventListener("animationend", check);
      box.removeEventListener("transitionend", check);
      size?.disconnect();
      content?.disconnect();
    };
  }, [ref]);
}
