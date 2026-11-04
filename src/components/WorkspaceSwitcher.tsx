import { useEffect, useMemo, useRef, useState } from "react";

import { basename, parentDir } from "../lib/format";
import { isComposing } from "../lib/keys";
import { marked, rankPaths, type Scored } from "../lib/palette";
import { Modal } from "./Modal";

/** The row that is always there, for a folder that isn't on the list yet. */
const BROWSE = "\u0000browse";

/**
 * The folders opened before, to go back to one without the folder dialog.
 *
 * Go to file's box, and its keys, over a different list. An empty box starts on
 * the folder opened before this one, so ⌃R then Enter goes back and forth
 * between two projects. The open folder is listed, marked, for where you are.
 * "Open folder…" is the last row whatever is typed, since a folder that isn't
 * on the list is the other reason to be here.
 */
export function WorkspaceSwitcher({
  current,
  recent,
  blocked,
  onSwitch,
  onBrowse,
  onForget,
  onClose,
}: {
  current: string | null;
  recent: string[];
  /** Why switching can't happen now, if it can't. */
  blocked: string | null;
  onSwitch: (path: string) => void;
  onBrowse: () => void;
  onForget: (path: string) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const box = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);

  const folders = useMemo(() => {
    const others = recent.filter((path) => path !== current);
    return current ? [current, ...others] : others;
  }, [recent, current]);
  const rows = useMemo<Scored<string>[]>(() => {
    const ranked = rankPaths(folders, query, folders.length);
    return [...ranked, { item: BROWSE, spans: [], score: 0 } as Scored<string>];
  }, [folders, query]);

  // The folder before this one, when nothing is typed: the likeliest place to go.
  const first = !query.trim() && rows[0]?.item === current && rows.length > 2 ? 1 : 0;
  const [active, setActive] = useState(first);
  useEffect(() => setActive(first), [first, query]);
  const index = Math.min(active, rows.length - 1);
  const chosen = rows[index]?.item;

  useEffect(() => {
    list.current?.querySelector('[data-active="true"]')?.scrollIntoView?.({ block: "nearest" });
  }, [index, rows.length]);

  const choose = (path: string) => {
    if (path === BROWSE) {
      onClose();
      onBrowse();
    } else if (path === current) {
      onClose();
    } else if (!blocked) {
      onClose();
      onSwitch(path);
    }
  };

  const key = (e: React.KeyboardEvent) => {
    if (isComposing(e)) return;
    if (e.key === "ArrowDown" || (e.key === "n" && e.ctrlKey)) {
      e.preventDefault();
      setActive((index + 1) % rows.length);
    } else if (e.key === "ArrowUp" || (e.key === "p" && e.ctrlKey)) {
      e.preventDefault();
      setActive((index - 1 + rows.length) % rows.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (chosen) choose(chosen);
    }
    // Escape is `Modal`'s, and Tab is too.
  };

  return (
    <Modal onClose={onClose} className="scrim palette-scrim" initialFocus={box}>
      <div className="palette" onClick={(e) => e.stopPropagation()}>
        <div className="palette-box">
          <input
            ref={box}
            className="palette-input"
            value={query}
            placeholder="Switch to a recent workspace"
            aria-label="Switch workspace"
            role="combobox"
            aria-expanded
            aria-controls="workspace-list"
            aria-activedescendant={chosen ? `workspace:${chosen}` : undefined}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={key}
          />
        </div>

        <div className="palette-list" id="workspace-list" role="listbox" ref={list}>
          {blocked && <p className="palette-searching">{blocked}</p>}
          {rows.map((row, i) =>
            row.item === BROWSE ? (
              <button
                key={BROWSE}
                type="button"
                id={`workspace:${BROWSE}`}
                role="option"
                aria-selected={i === index}
                data-active={i === index}
                className={`palette-row${i === index ? " on" : ""}`}
                onMouseMove={() => setActive(i)}
                onClick={() => choose(BROWSE)}
              >
                <span className="palette-label">Open folder…</span>
                <span className="finder-where">choose one that isn't listed</span>
              </button>
            ) : (
              <FolderRow
                key={row.item}
                row={row}
                active={i === index}
                isOpen={row.item === current}
                disabled={!!blocked && row.item !== current}
                onPick={() => choose(row.item)}
                onHover={() => setActive(i)}
                onForget={() => onForget(row.item)}
              />
            ),
          )}
          {folders.length === 0 && (
            <p className="palette-empty">Folders you open are listed here.</p>
          )}
        </div>

        <footer className="palette-foot">
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd> move
          </span>
          <span>
            <kbd>↵</kbd> switch
          </span>
          <span>
            <kbd>esc</kbd> close
          </span>
        </footer>
      </div>
    </Modal>
  );
}

function FolderRow({
  row,
  active,
  isOpen,
  disabled,
  onPick,
  onHover,
  onForget,
}: {
  row: Scored<string>;
  active: boolean;
  isOpen: boolean;
  disabled: boolean;
  onPick: () => void;
  onHover: () => void;
  onForget: () => void;
}) {
  const name = basename(row.item);
  const start = row.item.length - name.length;
  const runs = (text: string, spans: number[]) =>
    marked(text, spans).map((run, i) =>
      run.on ? <mark key={i}>{run.text}</mark> : <span key={i}>{run.text}</span>,
    );

  return (
    <div className="workspace-row" onMouseMove={onHover}>
      <button
        type="button"
        id={`workspace:${row.item}`}
        role="option"
        aria-selected={active}
        aria-disabled={disabled || undefined}
        data-active={active}
        className={`palette-row${active ? " on" : ""}`}
        onClick={onPick}
      >
        <span className="palette-label">
          {runs(
            name,
            row.spans.filter((at) => at >= start).map((at) => at - start),
          )}
        </span>
        <span className="finder-where">{parentDir(row.item)}</span>
        {isOpen && <span className="palette-when">open</span>}
      </button>
      {!isOpen && (
        <button
          type="button"
          className="workspace-forget"
          aria-label={`Remove ${name} from recent workspaces`}
          data-tip="Remove from this list. The folder isn't touched."
          onClick={onForget}
        >
          ✕
        </button>
      )}
    </div>
  );
}
