import { useEffect, useRef, type KeyboardEvent } from "react";

import type { FindOptions } from "../lib/find";
import { APPLE, isComposing } from "../lib/keys";
import type { FindBarProps } from "../lib/useFind";

/** The three toggles, their marks, and the letter that flips each. */
const TOGGLES: { option: keyof FindOptions; mark: string; name: string; code: string }[] = [
  { option: "matchCase", mark: "Aa", name: "Match case", code: "KeyC" },
  { option: "wholeWord", mark: "W", name: "Whole word", code: "KeyW" },
  { option: "regex", mark: ".*", name: "Regular expression", code: "KeyR" },
];

/** ⌥⌘ on a Mac, Alt elsewhere, as other editors bind the toggles. */
const toggleChord = (e: KeyboardEvent) =>
  e.altKey && !e.ctrlKey && (APPLE ? e.metaKey : !e.metaKey) && !e.shiftKey;
const toggleHint = APPLE ? "⌥⌘" : "Alt+";

/**
 * The box over a file or a note: a word to find, or `:120` to go to a line, and
 * below it, where the text can be changed, what to put in its place.
 */
export function FindBar({
  query,
  said,
  asked,
  options,
  replace,
  onQuery,
  onOption,
  onMove,
  onSubmit,
  onClose,
}: FindBarProps) {
  const box = useRef<HTMLInputElement>(null);
  const swap = useRef<HTMLInputElement>(null);
  const replacing = !!replace?.open;
  useEffect(() => {
    box.current?.focus();
    box.current?.setSelectionRange(query === ":" ? 1 : 0, query.length);
    // Only when asked. On `query` it would select what's being typed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [asked]);

  /* Keys both boxes share: Escape to leave, and the toggles. */
  const common = (e: KeyboardEvent<HTMLInputElement>): boolean => {
    if (e.key === "Escape") {
      // Marked, so the canvas doesn't read it as closing the file too.
      e.preventDefault();
      onClose();
      return true;
    }
    const toggle = toggleChord(e) && TOGGLES.find((t) => t.code === e.code);
    if (toggle) {
      e.preventDefault();
      onOption(toggle.option);
      return true;
    }
    return false;
  };

  /* A replace takes the focus into the text, which is where it writes; it comes
     back here so the next Enter replaces the next one. */
  const andBack = (act: () => void) => () => {
    act();
    swap.current?.focus();
  };

  return (
    <div className="doc-find" role="search">
      <div className="doc-find-row">
        {replace && (
          <button
            className="doc-find-button doc-find-fold"
            aria-label={replacing ? "Hide replace" : "Show replace"}
            aria-expanded={replacing}
            onClick={() => replace.onOpen(!replacing)}
          >
            {replacing ? "▾" : "▸"}
          </button>
        )}
        <input
          ref={box}
          className="doc-find-input"
          value={query}
          placeholder="Find, or :line"
          aria-label="Find, or type a colon and a line number"
          spellCheck={false}
          onChange={(e) => onQuery(e.target.value)}
          onKeyDown={(e) => {
            if (isComposing(e) || common(e)) return;
            if (e.key === "Enter") {
              e.preventDefault();
              if (!onSubmit()) onMove(e.shiftKey ? -1 : 1);
            }
          }}
        />
        {said && (
          <span className="doc-find-count" role="status">
            {said}
          </span>
        )}
        {TOGGLES.map((t) => (
          <button
            key={t.option}
            className="doc-find-button doc-find-toggle"
            aria-label={t.name}
            aria-pressed={options[t.option]}
            data-tip={`${t.name} (${toggleHint}${t.code.slice(3)})`}
            onClick={() => onOption(t.option)}
          >
            {t.mark}
          </button>
        ))}
        <button className="doc-find-button" aria-label="Previous match" onClick={() => onMove(-1)}>
          ↑
        </button>
        <button className="doc-find-button" aria-label="Next match" onClick={() => onMove(1)}>
          ↓
        </button>
        <button className="doc-find-button" aria-label="Close find" onClick={onClose}>
          ✕
        </button>
      </div>
      {replace && replacing && (
        <div className="doc-find-row">
          <input
            ref={swap}
            className="doc-find-input doc-replace-input"
            value={replace.text}
            placeholder={options.regex ? "Replace, $1 for a group" : "Replace"}
            aria-label="Replace with"
            spellCheck={false}
            onChange={(e) => replace.onText(e.target.value)}
            onKeyDown={(e) => {
              if (isComposing(e) || common(e)) return;
              if (e.key === "Enter") {
                e.preventDefault();
                // ⌘↵ for every match, as other editors have it.
                if (APPLE ? e.metaKey : e.ctrlKey) andBack(replace.onAll)();
                else andBack(replace.onOne)();
              }
            }}
          />
          <button
            className="doc-find-button doc-find-action"
            data-tip="Replace this match (Enter)"
            onClick={andBack(replace.onOne)}
          >
            Replace
          </button>
          <button
            className="doc-find-button doc-find-action"
            data-tip={`Replace every match (${APPLE ? "⌘" : "Ctrl+"}Enter)`}
            onClick={andBack(replace.onAll)}
          >
            All
          </button>
        </div>
      )}
    </div>
  );
}
