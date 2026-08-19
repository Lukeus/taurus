import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { Note } from "../lib/api";
import { plural, when } from "../lib/format";

/**
 * What earlier conversations in this workspace left for the next one.
 *
 * Two reasons this is a drawer rather than a file nobody mentions. It is part
 * of what is in the model's context before a conversation starts, and context
 * you cannot see is behaviour you cannot explain — the same argument the
 * standing brief makes next door in Skills. And a note is written without being
 * approved first, which is only a fair trade if there is somewhere to go and
 * take one back out.
 */
export function MemoryDrawer({
  onClose,
  onOpenSession,
}: {
  onClose: () => void;
  /** Opens the conversation a note came from. Absent while a turn is running,
   *  because switching conversations under a running turn is not offered. */
  onOpenSession?: (id: string) => void;
}) {
  const [notes, setNotes] = useState<Note[] | null>(null);
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    api
      .listNotes()
      .then(setNotes)
      .catch((e) => {
        setNotes([]);
        setFailed(String(e));
      });
  }, []);

  const forget = async (id: string) => {
    try {
      setNotes(await api.forgetNote(id));
      setFailed(null);
    } catch (e) {
      setFailed(String(e));
    }
  };

  return (
    <div className="scrim" onClick={onClose}>
      <aside className="drawer" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-head">
          <h2>Memory</h2>
          <button className="drawer-close" onClick={onClose}>
            Close
          </button>
        </header>

        <p className="drawer-intro">
          Written by the agent when it learns something worth carrying past the
          end of a conversation. Each of these is read into the prompt of the
          next conversation in this workspace, so what is here is what it starts
          out believing.
        </p>

        {failed && <div className="settings-problem">{failed}</div>}

        {notes === null ? (
          <p className="drawer-loading">Reading…</p>
        ) : notes.length === 0 ? (
          <p className="drawer-empty">
            Nothing has been written down for this workspace yet. There is
            nothing to do about that — the agent writes these itself, when a
            conversation turns up something the next one would otherwise have to
            rediscover.
          </p>
        ) : (
          <ul className="card-list">
            {notes.map((note) => (
              <li key={note.id} className="card">
                <div className="card-body">
                  <div className="card-title">{note.text}</div>
                  <div className="card-row">
                    <span className="micro">{when(note.at)}</span>
                    {onOpenSession && (
                      <button
                        className="drawer-close"
                        title="Open the conversation this was written in"
                        onClick={() => onOpenSession(note.session)}
                      >
                        where it came from
                      </button>
                    )}
                    <div className="spacer" />
                    <button
                      title="Stop carrying this into new conversations"
                      onClick={() => forget(note.id)}
                    >
                      Forget
                    </button>
                  </div>
                </div>
              </li>
            ))}
          </ul>
        )}

        <p className="drawer-foot">
          {plural(notes?.length ?? 0, "note")} · also `taurus notes list`
        </p>
      </aside>
    </div>
  );
}
