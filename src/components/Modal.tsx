import { useEffect, useRef, type ReactNode } from "react";

/**
 * The scrim every drawer, editor and dialog sits on, and the keyboard
 * behaviour they all owed and none of them had.
 *
 * Three things, and they are only worth having together. Escape closes, because
 * a panel over the whole app that can only be dismissed by hitting a ✕ or a
 * patch of dimmed background is one a keyboard cannot get out of. Focus moves
 * into the panel on open and back to whatever opened it on close, because
 * otherwise it stays on the row behind the scrim — so the first Tab walks the
 * *background*, and a screen reader is still reading the page the user has
 * just covered up. And Tab wraps inside the panel, which is what makes the two
 * above hold rather than being true only until the first Tab.
 *
 * The panel itself stays with the caller: a drawer is an `<aside>`, a dialog is
 * a `<div role="alertdialog">`, and the difference matters to what reads them.
 * This wraps whatever it is given and treats the first element inside as the
 * panel.
 */
export function Modal({
  /**
   * Dismisses it. Omitted for a panel that must be answered rather than
   * closed — the permission prompt, where Escape has no honest meaning: it is
   * neither "allow" nor a decision the user has made.
   */
  onClose,
  /** For the two that layer over an already-open drawer. */
  className = "scrim",
  children,
}: {
  onClose?: () => void;
  className?: string;
  children: ReactNode;
}) {
  const scrim = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const panel = scrim.current?.firstElementChild as HTMLElement | null;
    if (!panel) return;

    // Where focus was, to give it back. Read before anything is moved.
    const opener = document.activeElement as HTMLElement | null;

    // The panel is not naturally focusable, so it is made so only for this —
    // `-1` keeps it out of the Tab order it is about to be holding.
    if (!panel.hasAttribute("tabindex")) panel.tabIndex = -1;
    // The container rather than the first control: landing on a button means
    // a stray Enter has already pressed it, which on the permission prompt is
    // a grant nobody chose.
    panel.focus({ preventScroll: true });

    return () => opener?.focus?.({ preventScroll: true });
  }, []);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      /*
       * Not this one, if something is open on top of it.
       *
       * The editors render inside the drawer that lists what they edit, so
       * "on top" is a containment question the DOM can answer — and answering
       * it that way is order-independent, which a stack of registrations is
       * not: React runs effects child-first, so two modals mounting in the
       * same commit register innermost *first*.
       *
       * The permission prompt is the one modal that is not nested in anything,
       * so a drawer open behind it still takes Escape. That closes the drawer
       * and leaves the prompt exactly where it was, waiting to be answered,
       * which is the right outcome even if it is not the reasoned one.
       */
      if (scrim.current?.querySelector(".scrim")) return;
      if (e.key === "Escape" && onClose) {
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== "Tab") return;

      const panel = scrim.current?.firstElementChild as HTMLElement | null;
      if (!panel) return;
      // No visibility filter: these panels render their sections conditionally
      // rather than hiding them, so anything matching is on screen. A
      // `display`-based test would also be the one part of this that behaves
      // differently under a test renderer than in a window.
      const stops = [...panel.querySelectorAll<HTMLElement>(FOCUSABLE)];
      if (stops.length === 0) {
        // Nothing to move between, so Tab must not escape to the background.
        e.preventDefault();
        return;
      }

      const first = stops[0];
      const last = stops[stops.length - 1];
      const active = document.activeElement;
      // Off an end, off the panel itself, or from somewhere behind the scrim
      // entirely — all three have to come back inside. Anything else is an
      // ordinary step between two controls, which the browser does better.
      const edge = !panel.contains(active) || active === panel;
      if (e.shiftKey ? edge || active === first : edge || active === last) {
        e.preventDefault();
        (e.shiftKey ? last : first).focus();
      }
    };

    // Captured so that a control inside the panel cannot swallow Escape before
    // this sees it — which one further up the tree is entitled to do, and the
    // composer's own Escape handling does.
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [onClose]);

  return (
    <div className={className} onClick={onClose} ref={scrim}>
      {children}
    </div>
  );
}

/**
 * What Tab visits. Deliberately not `[tabindex="-1"]`, which is the panel
 * itself and anything else made reachable by script but not by the ring.
 */
const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");
