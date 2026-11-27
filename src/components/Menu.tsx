import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";

import { anchorTo } from "../lib/anchor";

export type MenuItem = {
  label: string;
  onSelect: () => void;
  disabled?: boolean;
  /** Said in the failure colour: a delete, a remove, an uninstall. */
  danger?: boolean;
  tip?: string;
};

/**
 * A card's ⋯: the actions it has that are not the one it is for.
 *
 * Every card in the drawers ended in a row of three or four quiet text buttons
 * — Edit, Disable, Delete; Open, Copy to mine, Delete — which made a list of
 * five servers a list of twenty controls, and gave the one action a card is
 * usually opened for (Sign in, Open) no more weight than the rest. A card now
 * keeps that one in view and puts the others here.
 *
 * A menu does not make a destructive action safer or less safe: a card's
 * Delete still arms the card's own two-press confirmation, so choosing it
 * here is the first press, not the last.
 *
 * The WAI-ARIA menu button: Enter, Space or ↓ opens it with the first item
 * focused, ↑ opens it on the last, arrows move, Home and End jump, Escape
 * closes and returns focus to the ⋯. The list is portalled into a `data-modal`
 * layer, as `Select`'s is and for the same reasons — a drawer cannot clip it,
 * and Escape or a press outside closes the menu rather than the drawer.
 */
export function Menu({
  items,
  label,
  disabled,
}: {
  items: readonly (MenuItem | false | null | undefined)[];
  /** Names the ⋯ for a screen reader: "Actions for github". */
  label: string;
  disabled?: boolean;
}) {
  const shown = items.filter((i): i is MenuItem => !!i);
  const [open, setOpen] = useState(false);
  const [start, setStart] = useState<"first" | "last">("first");
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);

  const rows = () =>
    [...(list.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? [])];

  const close = (refocus = true) => {
    setOpen(false);
    if (refocus) trigger.current?.focus();
  };

  const [place, setPlace] = useState<ReturnType<typeof anchorTo>>();
  useLayoutEffect(() => {
    if (!open || !trigger.current) return;
    setPlace(anchorTo(trigger.current, list.current?.offsetHeight ?? 0, { minWidth: 160, align: "end" }));
    const r = rows();
    (start === "first" ? r[0] : r[r.length - 1])?.focus({ preventScroll: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const away = () => close(false);
    window.addEventListener("resize", away);
    window.addEventListener("blur", away);
    return () => {
      window.removeEventListener("resize", away);
      window.removeEventListener("blur", away);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  if (shown.length === 0) return null;

  const onTriggerKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setStart(e.key === "ArrowUp" ? "last" : "first");
      setOpen(true);
    }
  };

  const onListKey = (e: KeyboardEvent) => {
    const r = rows();
    const at = r.indexOf(document.activeElement as HTMLButtonElement);
    const go = (i: number) => {
      e.preventDefault();
      r[(i + r.length) % r.length]?.focus();
    };
    switch (e.key) {
      case "ArrowDown":
        return go(at + 1);
      case "ArrowUp":
        return go(at - 1);
      case "Home":
        return go(0);
      case "End":
        return go(r.length - 1);
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        return close();
      case "Tab":
        e.preventDefault();
        return close();
    }
  };

  return (
    <>
      <button
        ref={trigger}
        type="button"
        className="menu-button quiet"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={label}
        data-tip="More"
        data-open={open ? "" : undefined}
        disabled={disabled}
        onClick={() => {
          setStart("first");
          setOpen((o) => !o);
        }}
        onKeyDown={onTriggerKey}
      >
        <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <circle cx="3.5" cy="8" r="1.25" fill="currentColor" />
          <circle cx="8" cy="8" r="1.25" fill="currentColor" />
          <circle cx="12.5" cy="8" r="1.25" fill="currentColor" />
        </svg>
      </button>
      {open &&
        createPortal(
          <div
            className="select-layer"
            data-modal=""
            onPointerDown={(e) => {
              if (e.target === e.currentTarget) {
                e.preventDefault();
                close();
              }
            }}
          >
            <div
              ref={list}
              role="menu"
              aria-label={label}
              className="menu-list"
              data-up={place?.up ? "" : undefined}
              style={place ? place.style : { visibility: "hidden" }}
              onKeyDown={onListKey}
            >
              {shown.map((item) => (
                <button
                  key={item.label}
                  type="button"
                  role="menuitem"
                  className={`menu-item${item.danger ? " danger" : ""}`}
                  disabled={item.disabled}
                  data-tip={item.tip}
                  data-tip-side="left"
                  onPointerMove={(e) => e.currentTarget.focus({ preventScroll: true })}
                  onClick={() => {
                    close();
                    item.onSelect();
                  }}
                >
                  {item.label}
                </button>
              ))}
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}
