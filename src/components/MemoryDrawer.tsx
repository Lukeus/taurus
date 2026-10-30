import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { Note } from "../lib/api";
import { when } from "../lib/format";
import { Drawer } from "./Drawer";
import { Problem } from "./Problem";

/**
 * What earlier conversations in this workspace left for the next one.
 *
 * Two reasons this is a drawer rather than a file nobody mentions. It is part
 * of what is in the model's context before a conversation starts, and context
 * you cannot see is behaviour you cannot explain — the same argument the
 * standing brief makes next door in Skills. And a note is written without being
 * approved first, which is only a fair trade if there is somewhere to go and
 * take one back out.
 *
 * You can write one too, and change any of them. A note of yours is carried
 * exactly as the model's are, so "staging has the migration, production
 * doesn't" can be said once rather than at the top of every conversation. It
 * is marked as yours, because the model's are the ones you'd want to check.
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
  const [draft, setDraft] = useState("");
  /** The note being changed, and its text so far. One at a time. */
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    api
      .listNotes()
      .then(setNotes)
      .catch((e) => {
        setNotes([]);
        setFailed(String(e));
      });
  }, []);

  /** Runs a change and redraws from the list it answers with. */
  const change = async (run: () => Promise<Note[]>) => {
    setSaving(true);
    try {
      setNotes(await run());
      setFailed(null);
      return true;
    } catch (e) {
      setFailed(String(e));
      return false;
    } finally {
      setSaving(false);
    }
  };

  const forget = (id: string) => change(() => api.forgetNote(id));

  const add = async () => {
    if (await change(() => api.addNote(draft))) setDraft("");
  };

  const save = async () => {
    if (!editing) return;
    const { id, text } = editing;
    if (await change(() => api.editNote(id, text))) setEditing(null);
  };

  return (
    <Drawer title="Memory" onClose={onClose}>
      <p className="drawer-intro">
        Written by the agent when it learns something worth carrying past the
        end of a conversation. Each of these is read into the prompt of the
        next conversation in this workspace, so what is here is what it starts
        out believing.
      </p>

      <section className="section memory-add">
        <label className="micro" htmlFor="memory-new">
          Write one yourself
        </label>
        <textarea
          id="memory-new"
          rows={2}
          value={draft}
          placeholder="e.g. the migration has run on staging, not production; schema.sql is the source of truth"
          onChange={(e) => setDraft(e.target.value)}
        />
        <div className="card-row">
          <span className="hint">A sentence or two, up to 2 KB.</span>
          <div className="spacer" />
          <button disabled={saving || !draft.trim()} onClick={add}>
            Add memory
          </button>
        </div>
      </section>

      {failed && <Problem>{failed}</Problem>}

      {notes === null ? (
        <p className="drawer-loading">Reading…</p>
      ) : notes.length === 0 ? (
        <p className="drawer-empty">
          Nothing has been written down for this workspace yet. The agent
          writes these itself, when a conversation turns up something the next
          one would otherwise have to rediscover — or write one above.
        </p>
      ) : (
        <ul className="card-list">
          {notes.map((note) => {
            const yours = note.session === "";
            const changing = editing?.id === note.id ? editing : null;
            return (
              <li key={note.id} className="card">
                <div className="card-body">
                  {changing ? (
                    <textarea
                      rows={3}
                      aria-label="Memory"
                      value={changing.text}
                      autoFocus
                      onChange={(e) => setEditing({ id: note.id, text: e.target.value })}
                    />
                  ) : (
                    <div className="card-title">{note.text}</div>
                  )}
                  <div className="card-row">
                    <span className="micro">{when(note.at)}</span>
                    {yours && <span className="tag">by you</span>}
                    {onOpenSession && !yours && (
                      <button
                        className="drawer-close"
                        data-tip="Open the conversation this was written in"
                        onClick={() => onOpenSession(note.session)}
                      >
                        where it came from
                      </button>
                    )}
                    <div className="spacer" />
                    {changing ? (
                      <>
                        <button className="quiet" onClick={() => setEditing(null)}>
                          Cancel
                        </button>
                        <button
                          className="primary"
                          disabled={saving || !changing.text.trim()}
                          onClick={save}
                        >
                          Save
                        </button>
                      </>
                    ) : (
                      <>
                        <button
                          className="quiet"
                          data-tip="Change what is carried into new conversations"
                          onClick={() => setEditing({ id: note.id, text: note.text })}
                        >
                          Edit
                        </button>
                        <button
                          data-tip="Stop carrying this into new conversations"
                          onClick={() => forget(note.id)}
                        >
                          Forget
                        </button>
                      </>
                    )}
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      <p className="drawer-foot">
        {notes?.length === 1 ? "1 memory" : `${notes?.length ?? 0} memories`} · also `taurus memory list`
      </p>
    </Drawer>
  );
}
