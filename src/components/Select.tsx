import {
  Children,
  isValidElement,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { anchorTo } from "../lib/anchor";

export type SelectOption = {
  value: string;
  label: string;
  /** Said faintly at the end of the row: a context size, a source, a scope. */
  hint?: string;
  disabled?: boolean;
  /** Options sharing a group are listed under it, in the order they arrive. */
  group?: string;
};

/**
 * The app's dropdown, in place of the platform's.
 *
 * A native `<select>` draws its closed state in our styles and its open list in
 * the operating system's: a light, system-font menu over a dark window on
 * macOS, a different one again on Windows and Linux. Every settings row that
 * offered a choice was the one place the window stopped looking like itself.
 * It also could not say more than a label per option, so a model's context
 * length or a server's scope had to live somewhere else.
 *
 * The behaviour is the WAI-ARIA select-only combobox: focus stays on the
 * trigger and the active row is pointed at with `aria-activedescendant`, so a
 * screen reader reads it as a list box and the keyboard works the way it does
 * on a native one — arrows, Home/End, Page keys, typing a name, Enter or Space
 * to choose, Escape to leave unchanged.
 *
 * The open list is portalled to the body inside a transparent full-window
 * layer marked `data-modal`. Two things follow from that, and both are the
 * point. A drawer's `overflow: hidden` cannot clip it. And `Modal` sees the
 * layer as the frontmost modal, so Escape closes the list rather than the
 * drawer it was opened in, and a press outside lands on the layer — closing
 * the list, as a native one does — rather than on the drawer's scrim, which
 * would close the drawer too.
 *
 * The trigger is a `<button>`, which is a labelable element, so a
 * `<label htmlFor>` pointed at `id` names it exactly as it named the select.
 *
 * Options come as `options`, or as `<option>` and `<optgroup>` children read
 * the way the browser reads them — which is how a `<select>` already in the
 * app becomes one of these by changing its tag and nothing in its list.
 */
export function Select({
  value,
  onChange,
  options: given,
  children,
  id,
  className,
  disabled,
  placeholder = "Choose…",
  "aria-label": ariaLabel,
  "data-tip": tip,
}: {
  value: string;
  onChange: (value: string) => void;
  options?: readonly SelectOption[];
  children?: ReactNode;
  id?: string;
  className?: string;
  disabled?: boolean;
  /** Shown when `value` matches nothing listed. */
  placeholder?: string;
  "aria-label"?: string;
  "data-tip"?: string;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const options = given ?? optionsFrom(children);
  const listId = useId();
  const optionId = (i: number) => `${listId}-${i}`;

  const selected = options.findIndex((o) => o.value === value);
  const current = selected >= 0 ? options[selected] : undefined;

  const enabled = (i: number) => i >= 0 && i < options.length && !options[i].disabled;
  /** The next choosable row from `from`, stepping by `step`, or `from` if none. */
  const step = (from: number, by: number) => {
    let i = from;
    for (let n = 0; n < options.length; n++) {
      i = Math.min(options.length - 1, Math.max(0, i + by));
      if (enabled(i)) return i;
      if (i === 0 || i === options.length - 1) break;
    }
    return enabled(from) ? from : -1;
  };
  const first = () => step(-1, 1);
  const last = () => step(options.length, -1);

  const show = (at = selected >= 0 ? selected : first()) => {
    if (disabled || options.length === 0) return;
    setActive(at);
    setOpen(true);
  };
  const close = () => setOpen(false);
  const choose = (i: number) => {
    if (!enabled(i)) return;
    close();
    if (options[i].value !== value) onChange(options[i].value);
  };

  // Typing a name moves to it, the way a native list does. The buffer
  // collects a word typed quickly and starts over after a pause.
  const typed = useRef({ text: "", at: 0 });
  const seek = (key: string) => {
    const now = Date.now();
    const t = typed.current;
    t.text = now - t.at > 600 ? key : t.text + key;
    t.at = now;
    const want = t.text.toLowerCase();
    // From the row after the active one, so pressing the same letter again
    // walks through every name that starts with it.
    const from = t.text.length === 1 ? active + 1 : Math.max(active, 0);
    for (let n = 0; n < options.length; n++) {
      const i = (from + n) % options.length;
      if (enabled(i) && options[i].label.toLowerCase().startsWith(want)) return i;
    }
    return -1;
  };

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (disabled) return;
    const printable = e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey;
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        show();
      } else if (printable) {
        const hit = seek(e.key);
        if (hit >= 0) {
          e.preventDefault();
          show(hit);
        }
      }
      return;
    }
    const move = (i: number) => {
      e.preventDefault();
      if (i >= 0) setActive(i);
    };
    switch (e.key) {
      case "ArrowDown":
        return move(step(active, 1));
      case "ArrowUp":
        return move(step(active, -1));
      case "Home":
        return move(first());
      case "End":
        return move(last());
      case "PageDown":
        return move(step(Math.min(active + 9, options.length - 1), 1));
      case "PageUp":
        return move(step(Math.max(active - 9, 0), -1));
      case "Enter":
      case " ":
        e.preventDefault();
        return choose(active);
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        return close();
      case "Tab":
        return close();
    }
    if (printable) {
      const hit = seek(e.key);
      if (hit >= 0) move(hit);
    }
  };

  // Placed against the trigger, below it unless there is more room above.
  // Measured before paint, so the list never draws once in the wrong place.
  const [place, setPlace] = useState<ReturnType<typeof anchorTo>>();
  useLayoutEffect(() => {
    if (!open || !trigger.current) return;
    setPlace(anchorTo(trigger.current, list.current?.offsetHeight ?? 0));
  }, [open]);

  // The active row stays in view as the keyboard moves it.
  useEffect(() => {
    if (!open || active < 0) return;
    document.getElementById(optionId(active))?.scrollIntoView?.({ block: "nearest" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, active]);

  // A window that changes size leaves the list pointing at where the trigger
  // was. Closing is what a native list does, and it is never wrong.
  useEffect(() => {
    if (!open) return;
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
    };
  }, [open]);

  let lastGroup: string | undefined;
  return (
    <>
      <button
        type="button"
        ref={trigger}
        id={id}
        className={`select-box${className ? ` ${className}` : ""}`}
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open && active >= 0 ? optionId(active) : undefined}
        aria-label={ariaLabel}
        data-tip={tip}
        data-value={value}
        data-open={open ? "" : undefined}
        disabled={disabled}
        onClick={() => (open ? close() : show())}
        onKeyDown={onKeyDown}
        onBlur={(e) => {
          // A press inside the list keeps focus here (see the list's
          // `onPointerDown`), so a blur is focus genuinely leaving.
          if (!list.current?.contains(e.relatedTarget as Node | null)) close();
        }}
      >
        <span className={`select-value${current ? "" : " empty"}`}>
          <span className="select-current">{current?.label ?? (value || placeholder)}</span>
          {options.map((o) => (
            <span key={o.value} className="select-sizer" aria-hidden="true">
              {o.label}
            </span>
          ))}
        </span>
        <svg className="select-chevron" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M2 3.5 5 6.5 8 3.5" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
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
            <ul
              ref={list}
              id={listId}
              role="listbox"
              aria-label={ariaLabel}
              className="select-list"
              data-up={place?.up ? "" : undefined}
              style={place ? place.style : { visibility: "hidden" }}
              // Keeps focus on the trigger, which is where the keyboard and
              // `aria-activedescendant` live.
              onPointerDown={(e) => e.preventDefault()}
            >
              {options.map((o, i) => {
                const heading = o.group !== undefined && o.group !== lastGroup ? o.group : null;
                lastGroup = o.group;
                return [
                  heading !== null && (
                    <li key={`g-${heading}`} role="presentation" className="select-group">
                      {heading}
                    </li>
                  ),
                  <li
                    key={o.value}
                    id={optionId(i)}
                    role="option"
                    aria-selected={i === selected}
                    aria-disabled={o.disabled || undefined}
                    data-value={o.value}
                    data-active={i === active ? "" : undefined}
                    className="select-option"
                    onPointerMove={() => enabled(i) && setActive(i)}
                    onClick={() => choose(i)}
                  >
                    <span className="select-check" aria-hidden="true">
                      {i === selected ? "✓" : ""}
                    </span>
                    <span className="min-w-0 flex-1 truncate">{o.label}</span>
                    {o.hint && <span className="select-hint">{o.hint}</span>}
                  </li>,
                ];
              })}
              {options.length === 0 && (
                <li role="presentation" className="select-group">
                  Nothing to choose from
                </li>
              )}
            </ul>
          </div>,
          document.body,
        )}
    </>
  );
}

/**
 * `<option>` and `<optgroup>` children as options. Fragments, arrays from a
 * `.map` and the `false` of a conditional are all flattened away by
 * `Children.toArray`, so whatever shape a list was written in for a native
 * select reads the same here.
 */
function optionsFrom(children: ReactNode, group?: string): SelectOption[] {
  const out: SelectOption[] = [];
  for (const child of Children.toArray(children)) {
    if (!isValidElement<{ value?: unknown; label?: string; disabled?: boolean; children?: ReactNode }>(child)) {
      continue;
    }
    const { props } = child;
    if (child.type === "optgroup") {
      out.push(...optionsFrom(props.children, props.label));
    } else if (child.type === "option") {
      const label = textOf(props.children);
      out.push({
        value: props.value === undefined ? label : String(props.value),
        label,
        disabled: props.disabled,
        group,
      });
    }
  }
  return out;
}

function textOf(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  return "";
}
