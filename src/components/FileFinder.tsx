import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";

import * as api from "../lib/api";
import { isComposing } from "../lib/keys";
import { marked, rankPaths, type Scored } from "../lib/palette";
import { Modal } from "./Modal";

/** Rows drawn. Past this somebody types another letter rather than scrolls. */
const ROWS = 50;

/**
 * One box over the workspace's files. Enter opens the file in the canvas.
 *
 * The list is walked each time this opens rather than held, so a file made a
 * moment ago in the terminal is there. It's ranked here because the walk is the
 * slow part: ranking 50,000 paths takes a few milliseconds a keystroke, and a
 * deferred query keeps typing ahead of it on a slow machine.
 */
export function FileFinder({
  open,
  onOpen,
  onClose,
}: {
  /** What the canvas has open, which is what an empty box offers first. */
  open: string[];
  onOpen: (path: string) => void;
  onClose: () => void;
}) {
  const [paths, setPaths] = useState<string[] | null>(null);
  const [cut, setCut] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const box = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const wanted = useDeferredValue(query);

  useEffect(() => {
    let current = true;
    api
      .workspaceFiles()
      .then((files) => {
        if (!current) return;
        setPaths(files.paths);
        setCut(files.cut);
      })
      .catch((e) => current && setProblem(String(e)));
    return () => {
      current = false;
    };
  }, []);

  const rows = useMemo<Scored<string>[]>(() => {
    const all = paths ?? [];
    if (wanted.trim()) return rankPaths(all, wanted, ROWS);
    const first = open.filter((path) => all.includes(path));
    const shown = new Set(first);
    return rankPaths([...first, ...all.filter((path) => !shown.has(path))], "", ROWS);
  }, [paths, wanted, open]);

  const index = Math.min(active, Math.max(rows.length - 1, 0));
  const current = rows[index]?.item;

  useEffect(() => {
    list.current?.querySelector('[data-active="true"]')?.scrollIntoView?.({ block: "nearest" });
  }, [index, rows.length]);

  const choose = (path: string) => {
    onClose();
    onOpen(path);
  };

  const key = (e: React.KeyboardEvent) => {
    if (isComposing(e)) return;
    if (e.key === "ArrowDown" || (e.key === "n" && e.ctrlKey)) {
      e.preventDefault();
      setActive(rows.length ? (index + 1) % rows.length : 0);
    } else if (e.key === "ArrowUp" || (e.key === "p" && e.ctrlKey)) {
      e.preventDefault();
      setActive(rows.length ? (index - 1 + rows.length) % rows.length : 0);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (current) choose(current);
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
            placeholder="Go to a file by name"
            aria-label="Go to file"
            role="combobox"
            aria-expanded
            aria-controls="finder-list"
            aria-activedescendant={current ? `file:${current}` : undefined}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            onKeyDown={key}
          />
        </div>

        <div className="palette-list" id="finder-list" role="listbox" ref={list}>
          {problem && <p className="palette-empty">{problem}</p>}
          {!problem && paths && rows.length === 0 && (
            <p className="palette-empty">
              {wanted.trim() ? "No file matches that." : "This workspace has no files."}
            </p>
          )}
          {rows.map((row, i) => (
            <FileRow
              key={row.item}
              row={row}
              active={i === index}
              isOpen={open.includes(row.item)}
              onPick={() => choose(row.item)}
              onHover={() => setActive(i)}
            />
          ))}
          {cut && (
            <p className="palette-searching">
              This workspace is too large to list in full, so some files may be missing.
            </p>
          )}
        </div>

        <footer className="palette-foot">
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd> move
          </span>
          <span>
            <kbd>↵</kbd> open
          </span>
          <span>
            <kbd>esc</kbd> close
          </span>
        </footer>
      </div>
    </Modal>
  );
}

function FileRow({
  row,
  active,
  isOpen,
  onPick,
  onHover,
}: {
  row: Scored<string>;
  active: boolean;
  isOpen: boolean;
  onPick: () => void;
  onHover: () => void;
}) {
  const start = row.item.lastIndexOf("/") + 1;
  const inName = row.spans.filter((at) => at >= start).map((at) => at - start);
  const inFolder = row.spans.filter((at) => at < start);
  const runs = (text: string, spans: number[]) =>
    marked(text, spans).map((run, i) =>
      run.on ? <mark key={i}>{run.text}</mark> : <span key={i}>{run.text}</span>,
    );

  return (
    <button
      type="button"
      id={`file:${row.item}`}
      role="option"
      aria-selected={active}
      data-active={active}
      className={`palette-row${active ? " on" : ""}`}
      onMouseMove={onHover}
      onClick={onPick}
    >
      <span className="palette-label">{runs(row.item.slice(start), inName)}</span>
      {start > 0 && (
        <span className="finder-where">{runs(row.item.slice(0, start - 1), inFolder)}</span>
      )}
      {isOpen && <span className="palette-when">open</span>}
    </button>
  );
}
