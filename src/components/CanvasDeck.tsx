import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, type MutableRefObject } from "react";

import type { Selection } from "../lib/api";
import { tabLabels, type CanvasTabs } from "../lib/canvasTabs";
import { useGlide } from "../lib/motion";
import { onTabKeys } from "../lib/tabs";
import { CanvasSlot, type TabMark } from "./CanvasSlot";
import { ResizeHandle, type Resizable } from "./ResizeHandle";

type Flush = MutableRefObject<(() => Promise<boolean>) | null>;

/**
 * The files open beside the conversation: a row of tabs and one `CanvasSlot` per
 * file.
 *
 * One slot per file rather than one slot that swaps, because a slot already owns
 * everything true of a file that isn't yet true on disk: its autosave, its
 * conflict, its reloads when the turn writes it. A file in a hidden tab goes on
 * doing all of that.
 *
 * The slots stay in this tree while Changes has the column, so nothing remounts
 * when it shuts. Hidden tabs are stacked and invisible rather than `display:
 * none`. A hidden tab keeps its scroll position and undo history only if the
 * editor stays laid out, and an editor laid out to zero height paints every line.
 */
export function CanvasDeck({
  canvas,
  marks,
  workspace,
  showing,
  pane,
  onShow,
  onClose,
  onNamed,
  onMark,
  onTyped,
  onSelect,
  onAsk,
  flushRef,
}: {
  canvas: CanvasTabs;
  marks: Record<string, TabMark | undefined>;
  workspace: string | null;
  /** Whether the column is this one's. Changes takes it while it is open. */
  showing: boolean;
  pane: Resizable;
  onShow: (path: string) => void;
  onClose: (path: string) => void;
  onNamed: (path: string, shown: string) => void;
  onMark: (path: string, mark: TabMark) => void;
  /** Somebody typed or pasted in one of the files. */
  onTyped?: () => void;
  /** Only ever about the file that's showing. */
  onSelect: (selection: Selection | null) => void;
  onAsk: (text: string) => void;
  /** Writes what every tab is waiting to write. Answers for the one showing. */
  flushRef: Flush;
}) {
  const { tabs, active } = canvas;

  /*
   * What's highlighted in each file. Only the one on screen travels with a
   * message, and coming back to a tab brings its selection back with it.
   */
  const selections = useRef(new Map<string, Selection | null>());
  const activeNow = useRef(active);
  activeNow.current = active;
  const selected = useCallback(
    (path: string, selection: Selection | null) => {
      selections.current.set(path, selection);
      if (path === activeNow.current) onSelect(selection);
    },
    [onSelect],
  );
  useEffect(() => {
    onSelect(selections.current.get(active) ?? null);
  }, [active, onSelect]);

  /* Each slot's own flush, gathered so `App` has one thing to call. */
  const flushes = useRef(new Map<string, Flush>());
  const flushFor = (path: string): Flush => {
    let flush = flushes.current.get(path);
    if (!flush) {
      flush = { current: null };
      flushes.current.set(path, flush);
    }
    return flush;
  };
  useEffect(() => {
    flushRef.current = async () => {
      const all = [...flushes.current.entries()];
      const answers = await Promise.all(
        all.map(async ([path, flush]) => [path, (await flush.current?.()) ?? true] as const),
      );
      return answers.find(([path]) => path === activeNow.current)?.[1] ?? true;
    };
    return () => {
      flushRef.current = null;
    };
  }, [flushRef]);

  // Closed tabs forgotten, so a file reopened later starts with no old selection.
  useEffect(() => {
    const open = new Set(tabs.map((tab) => tab.path));
    for (const path of [...flushes.current.keys()]) if (!open.has(path)) flushes.current.delete(path);
    for (const path of [...selections.current.keys()]) if (!open.has(path)) selections.current.delete(path);
  }, [tabs]);

  const wide = useMemo(() => ({ on: pane.filled, toggle: pane.fill }), [pane.filled, pane.fill]);

  const labels = useMemo(() => tabLabels(tabs.map((tab) => tab.shown ?? tab.path)), [tabs]);

  const row = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    // Absent in jsdom. A file opened off the end of the row has to come into view.
    row.current?.querySelector(".canvas-tab.on")?.scrollIntoView?.({ inline: "nearest", block: "nearest" });
  }, [active, showing]);
  useGlide(row, `${active}|${showing}|${[...labels.values()].join("|")}`, ".canvas-tab.on");

  return (
    <>
      {showing && <ResizeHandle pane={pane} label="Editor width" />}
      <div
        className={`side-slot canvas-deck${pane.easing ? " easing" : ""}`}
        ref={pane.slot}
        onInput={onTyped}
        style={{ width: pane.size, display: showing ? undefined : "none" }}
      >
        {tabs.length > 1 && (
          <div className="canvas-tabs" role="tablist" aria-label="Open files" onKeyDown={onTabKeys} ref={row}>
            {tabs.map((tab) => {
              const name = tab.shown ?? tab.path;
              const label = labels.get(name) ?? name;
              const on = tab.path === active;
              const mark = marks[tab.path];
              return (
                <div
                  key={tab.path}
                  className={`canvas-tab${on ? " on" : ""}`}
                  onAuxClick={(e) => {
                    if (e.button === 1) onClose(tab.path);
                  }}
                >
                  <button
                    role="tab"
                    aria-selected={on}
                    tabIndex={on ? 0 : -1}
                    className="canvas-tab-name"
                    title={name}
                    onClick={() => onShow(tab.path)}
                    onKeyDown={(e) => {
                      if (e.key === "Delete") onClose(tab.path);
                    }}
                  >
                    {mark?.trouble ? (
                      <span className="canvas-tab-mark trouble" role="img" aria-label={mark.trouble === "gone" ? "Gone from disk" : "Two versions to choose between"}>
                        !
                      </span>
                    ) : mark?.unsaved ? (
                      <span className="canvas-tab-mark" role="img" aria-label="Not saved yet">
                        •
                      </span>
                    ) : null}
                    {label}
                  </button>
                  <button
                    className="canvas-tab-close"
                    tabIndex={-1}
                    aria-label={`Close ${label}`}
                    onClick={() => onClose(tab.path)}
                  >
                    ✕
                  </button>
                </div>
              );
            })}
            <span className="glide" aria-hidden="true" />
          </div>
        )}
        <div className="canvas-stack">
          {tabs.map((tab) => (
            <TabSlot
              key={tab.path}
              path={tab.path}
              reveal={tab.reveal}
              workspace={workspace}
              front={tab.path === active}
              column={showing}
              flushRef={flushFor(tab.path)}
              onNamed={onNamed}
              onMark={onMark}
              onSelected={selected}
              onAsk={onAsk}
              onClose={onClose}
              wide={wide}
            />
          ))}
        </div>
      </div>
    </>
  );
}

/** A slot with its path bound in, so the callbacks it's handed stay stable. */
function TabSlot({
  path,
  onNamed,
  onMark,
  onSelected,
  onClose,
  front,
  column,
  ...rest
}: {
  path: string;
  reveal: { from: number; to: number } | null;
  workspace: string | null;
  front: boolean;
  column: boolean;
  flushRef: Flush;
  onNamed: (path: string, shown: string) => void;
  onMark: (path: string, mark: TabMark) => void;
  onSelected: (path: string, selection: Selection | null) => void;
  onAsk: (text: string) => void;
  onClose: (path: string) => void;
  wide: { on: boolean; toggle: () => void };
}) {
  const named = useCallback((shown: string) => onNamed(path, shown), [path, onNamed]);
  const marked = useCallback((mark: TabMark) => onMark(path, mark), [path, onMark]);
  const select = useCallback((s: Selection | null) => onSelected(path, s), [path, onSelected]);
  const close = useCallback(() => onClose(path), [path, onClose]);
  return (
    <CanvasSlot
      {...rest}
      path={path}
      showing={front && column}
      inColumn={column}
      onNamed={named}
      onMark={marked}
      onSelect={select}
      onClose={close}
    />
  );
}
