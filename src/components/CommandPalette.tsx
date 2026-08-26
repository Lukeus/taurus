import { useEffect, useMemo, useRef, useState } from "react";

import * as api from "../lib/api";
import type { SearchResults, SessionMeta } from "../lib/api";
import { when } from "../lib/format";
import { marked, rank, type Action, type Scored } from "../lib/palette";
import { Modal } from "./Modal";

/**
 * One box over the whole app: what you can do, and what you have already done.
 *
 * Before this, the window had exactly one keyboard shortcut in it and the only
 * way to reach an old conversation was to recognise its title in a list that
 * only ever grows. Those are the same missing thing seen from two sides — one
 * input, one ranking, and two kinds of answer under it.
 *
 * Three groups, in the order they can be produced:
 *
 * - **Do** — every panel, and the handful of verbs that are not in one. Ranked
 *   locally, so it answers before the second keystroke lands.
 * - **Conversations** — matched on title, also local, also instant.
 * - **In conversations** — the transcripts themselves, which means reading
 *   files, so it is debounced and arrives underneath what is already there.
 *   The list above never waits for it and never jumps when it lands: the two
 *   local groups keep their place, and this one fills in below them.
 *
 * That last property is the reason the groups are in fixed order rather than
 * interleaved by score. A row that was under the cursor and then moved because
 * a slower answer outranked it is a row you have already pressed Enter on.
 */
export function CommandPalette({
  actions,
  sessions,
  onOpenSession,
  onClose,
}: {
  actions: Action[];
  /** What the rail is showing. Matched on title, without touching the disk. */
  sessions: SessionMeta[];
  /**
   * Opens a conversation.
   *
   * The second argument is the text to jump to once it is on screen, and it is
   * `null` for a row picked by title — a title match says which conversation,
   * not where in it, and scrolling to the first mention of a word that happens
   * to be in the title would land somewhere nobody asked about.
   */
  onOpenSession: (id: string, find: string | null) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [found, setFound] = useState<SearchResults | null>(null);
  const [searching, setSearching] = useState(false);
  const [everywhere, setEverywhere] = useState(false);
  const box = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);

  /*
   * The transcript search, once typing has paused.
   *
   * Not on every keystroke: this reads every transcript in the workspace, and
   * a search fired per character would spend most of its time answering
   * prefixes nobody meant. Not so late that it feels broken either — 180ms is
   * about the gap between words rather than between letters.
   *
   * Two characters is the floor. One letter matches nearly every conversation,
   * which is a list that costs a disk read per entry to produce and tells you
   * nothing.
   */
  useEffect(() => {
    const wanted = query.trim();
    if (wanted.length < 2) {
      setFound(null);
      setSearching(false);
      return;
    }
    setSearching(true);
    let current = true;
    const timer = setTimeout(() => {
      api
        .searchSessions(wanted, everywhere)
        .then((results) => current && setFound(results))
        // A failed search leaves the two local groups alone rather than
        // taking the whole palette down: the actions are still the answer to
        // most of what this box is opened for.
        .catch(() => current && setFound(null))
        .finally(() => current && setSearching(false));
    }, 180);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [query, everywhere]);

  const rows = useMemo(
    () => build(actions, sessions, found, query),
    [actions, sessions, found, query],
  );

  // Clamped rather than reset, for the reason the composer's command menu is:
  // the list narrows as the query grows, and a highlight left pointing past
  // the end would run the wrong thing on Enter.
  const index = Math.min(active, Math.max(rows.length - 1, 0));
  const current = rows[index];

  // Keeps the highlight on screen when it is moved by the keyboard. `nearest`
  // rather than `center`, so walking down a long list scrolls a line at a time
  // instead of jumping the list under the cursor.
  useEffect(() => {
    list.current
      ?.querySelector('[data-active="true"]')
      ?.scrollIntoView({ block: "nearest" });
  }, [index, rows.length]);

  const choose = (row: Row) => {
    if (row.kind === "action") {
      if (row.action.unavailable) return;
      onClose();
      row.action.run();
      return;
    }
    onClose();
    onOpenSession(row.session.id, row.excerpt ? query.trim() : null);
  };

  const key = (e: React.KeyboardEvent) => {
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
            placeholder="Type a command, or something you said"
            aria-label="Command palette"
            role="combobox"
            aria-expanded
            aria-controls="palette-list"
            aria-activedescendant={current ? rowId(current) : undefined}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            onKeyDown={key}
          />
          <button
            type="button"
            className={`palette-scope${everywhere ? " on" : ""}`}
            data-tip="Search transcripts in every workspace, not only this one"
            onClick={() => {
              setEverywhere((on) => !on);
              box.current?.focus();
            }}
          >
            everywhere
          </button>
        </div>

        <div className="palette-list" id="palette-list" role="listbox" ref={list}>
          {rows.length === 0 && !searching && (
            <p className="palette-empty">
              {query.trim()
                ? "Nothing matches that."
                : "Nothing to show yet."}
            </p>
          )}
          {rows.map((row, i) => {
            const heading = row.group !== rows[i - 1]?.group ? row.group : null;
            return (
              <div key={rowId(row)}>
                {heading && <div className="palette-group">{heading}</div>}
                <Row
                  row={row}
                  active={i === index}
                  onPick={() => choose(row)}
                  onHover={() => setActive(i)}
                />
              </div>
            );
          })}
          {searching && <p className="palette-searching">Reading transcripts…</p>}
          {found && found.more > 0 && (
            /* Said rather than left implied: a list that stops without saying
               so reads as the whole list. */
            <p className="palette-searching">
              {found.more} more {found.more === 1 ? "conversation" : "conversations"}{" "}
              matched. Type another word.
            </p>
          )}
        </div>

        <footer className="palette-foot">
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd> move
          </span>
          <span>
            <kbd>↵</kbd> run
          </span>
          <span>
            <kbd>esc</kbd> close
          </span>
        </footer>
      </div>
    </Modal>
  );
}

/** What a row is, and everything needed to draw and run it. */
type Row =
  | { kind: "action"; group: string; action: Action; spans: number[] }
  | {
      kind: "session";
      group: string;
      session: SessionMeta;
      spans: number[];
      /** The message to scroll to, for a row that came from a transcript hit. */
      message?: number;
      /** The text around the hit, and where the hit sits in it. */
      excerpt?: { text: string; from: number; to: number };
      /** How many hits this conversation has, when more than the one shown. */
      hits?: number;
    };

const rowId = (row: Row) =>
  row.kind === "action"
    ? `action:${row.action.id}`
    : `session:${row.session.id}:${row.message ?? "title"}`;

/** How many conversations to offer by title before the list is scrolling. */
const TITLES = 6;

/**
 * The whole result list, in the order it is drawn.
 *
 * A plain function rather than three pieces of state: the order is the feature
 * — see the note about rows moving under the cursor — and an order assembled
 * in one place cannot be half-updated.
 */
function build(
  actions: Action[],
  sessions: SessionMeta[],
  found: SearchResults | null,
  query: string,
): Row[] {
  const rows: Row[] = [];

  for (const hit of rank(actions, query, (a) => ({
    label: a.label,
    hidden: a.keywords,
  }))) {
    rows.push({
      kind: "action",
      group: hit.item.group,
      action: hit.item,
      spans: hit.spans,
    });
  }

  // Titles, matched locally. A session with no turns has no title to match, so
  // it can only appear here when nothing has been typed.
  const named = sessions.filter((s) => s.title || !query.trim());
  const byTitle: Scored<SessionMeta>[] = rank(
    named,
    query,
    (s) => ({ label: s.title || "New conversation" }),
    TITLES,
  );
  for (const hit of byTitle) {
    rows.push({
      kind: "session",
      group: "Conversations",
      session: hit.item,
      spans: hit.spans,
    });
  }

  // And the transcripts. A conversation already offered by title is not
  // offered again by content — it is the same conversation, and two rows for
  // it is one row of noise plus a second chance to pick the wrong one.
  const already = new Set(byTitle.map((hit) => hit.item.id));
  for (const hit of found?.sessions ?? []) {
    if (already.has(hit.session.id)) continue;
    const first = hit.matches[0];
    rows.push({
      kind: "session",
      group: "In conversations",
      session: hit.session,
      spans: [],
      message: first?.message,
      excerpt: first && { text: first.excerpt, from: first.from, to: first.to },
      hits: hit.hits,
    });
  }

  return rows;
}

function Row({
  row,
  active,
  onPick,
  onHover,
}: {
  row: Row;
  active: boolean;
  onPick: () => void;
  onHover: () => void;
}) {
  const blocked = row.kind === "action" ? row.action.unavailable : undefined;
  const label =
    row.kind === "action" ? row.action.label : row.session.title || "New conversation";

  return (
    <button
      type="button"
      id={rowId(row)}
      role="option"
      aria-selected={active}
      data-active={active}
      className={`palette-row${active ? " on" : ""}${blocked ? " blocked" : ""}`}
      // Hover moves the highlight rather than drawing a second one. Two
      // different "this one" marks on screen at once is a question about which
      // of them Enter belongs to.
      onMouseMove={onHover}
      onClick={onPick}
      disabled={!!blocked}
    >
      <span className="palette-label">
        {marked(label, row.spans).map((run, i) =>
          run.on ? (
            <mark key={i}>{run.text}</mark>
          ) : (
            <span key={i}>{run.text}</span>
          ),
        )}
      </span>

      {row.kind === "action" && blocked && <span className="palette-why">{blocked}</span>}
      {row.kind === "action" && !blocked && row.action.shortcut && (
        /* The only place anybody finds out a shortcut exists. */
        <kbd className="palette-key">{row.action.shortcut}</kbd>
      )}

      {row.kind === "session" && (
        <>
          {row.excerpt && (
            <span className="palette-excerpt">
              {row.excerpt.text.slice(0, row.excerpt.from)}
              <mark>{row.excerpt.text.slice(row.excerpt.from, row.excerpt.to)}</mark>
              {row.excerpt.text.slice(row.excerpt.to)}
            </span>
          )}
          <span className="palette-when">
            {row.hits && row.hits > 1 ? `${row.hits} hits · ` : ""}
            {when(row.session.updated)}
          </span>
        </>
      )}
    </button>
  );
}
