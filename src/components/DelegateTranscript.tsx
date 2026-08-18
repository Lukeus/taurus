import { useEffect, useState } from "react";

import * as api from "../lib/api";
import { entriesFromMessages, type Entry } from "../state/store";
import { ResizeHandle, useResizableWidth } from "./ResizeHandle";
import { Transcript } from "./Transcript";

/** Wider than the other drawers: this one holds a conversation, not a list. */
const WIDTH = { initial: 560, min: 380, max: 900 };

/**
 * What a delegate did, in full.
 *
 * The parent's transcript records a delegation as what it is — one call, one
 * paragraph back — because a second conversation inlined into the first is
 * exactly what delegating exists to avoid. That leaves one question it cannot
 * answer, and it is the question asked whenever the paragraph is thin or wrong:
 * what was the child actually doing. This is where that is answered, one step
 * away rather than in the way.
 *
 * Read-only, and not a resume. A delegate's conversation happened inside
 * somebody else's turn; it has no provider bound to it and nothing to continue.
 *
 * It is drawn by the same {@link Transcript} the conversation itself uses, so a
 * child's tool calls fold into runs and its prose renders as markdown without a
 * second renderer that could disagree about either.
 */
export function DelegateTranscript({
  sessionId,
  subagentId,
  agent,
  onClose,
}: {
  sessionId: string;
  subagentId: string;
  agent: string;
  onClose: () => void;
}) {
  const pane = useResizableWidth({
    storageKey: "taurus.delegateWidth",
    grow: -1,
    ...WIDTH,
  });
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .readSubagentTranscript(sessionId, subagentId)
      .then((messages) => live && setEntries(entriesFromMessages(messages)))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, [sessionId, subagentId]);

  return (
    <div className="scrim" onClick={onClose}>
      <div className="drawer-dock" onClick={(e) => e.stopPropagation()}>
        <ResizeHandle pane={pane} label="Delegate transcript width" />
        <aside className="drawer delegate-drawer" style={{ width: pane.width }}>
          <header className="drawer-head">
            <h2>{agent}</h2>
            <button className="drawer-close" onClick={onClose} aria-label="Close">
              ✕
            </button>
          </header>

          <p className="drawer-intro">
            The conversation this delegation had. Its own context, its own tools,
            and nothing of it in the transcript that spawned it.
          </p>

          {error && <p className="drawer-empty">{error}</p>}

          {!error && entries === null && (
            <p className="drawer-empty">Reading the transcript…</p>
          )}

          {entries && (
            <div className="delegate-body">
              <Transcript
                entries={entries}
                busy={false}
                follow={false}
                // A delegate has no `ask_user` — sub-agents are not registered
                // with it — so no question card can appear here to answer.
                onAnswer={() => {}}
                empty={
                  <p className="drawer-empty">
                    This delegate was recorded, but wrote nothing before it
                    stopped.
                  </p>
                }
              />
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}
