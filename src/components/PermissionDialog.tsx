import { useEffect } from "react";

import type { PermissionDecision, PermissionRequest } from "../lib/api";
import { chord, isChord } from "../lib/keys";
import { DiffView } from "./DiffView";
import { Modal } from "./Modal";

const EFFECT_LABEL: Record<string, string> = {
  write: "wants to change a file",
  execute: "wants to run a command",
  network: "wants to reach the network",
  read: "wants to read",
};

/**
 * The permission gate.
 *
 * Deliberately shows the exact call rather than a summary: the user is
 * approving this command line or this file write, not the tool in general.
 * The scope sentence sits above the buttons because it is the thing that
 * distinguishes them — which of these grants is wider, and by how much.
 *
 * The one `Modal` with no `onClose`: this is answered, not dismissed. Escape
 * would have to mean one of the buttons, and none of them is what a person
 * pressing Escape has decided.
 *
 * Two chords, and the reason they exist here and almost nowhere else is that
 * this is the most-pressed control in the app. A turn that greps, reads, edits
 * and runs the tests stops at this dialog four times, and until now every one
 * of those was a reach for the mouse or a count of tab stops. The window-wide
 * shortcut list is short on purpose — see `App` — but that argument is about a
 * shared namespace, and a modal has none: while this is open it is the only
 * thing on screen, so a chord bound here can collide with nothing and needs no
 * discovery surface beyond the key printed on the button it fires.
 *
 * Deliberately not bare Enter and bare Escape, which is what makes this safe
 * rather than merely fast. The dialog can appear under someone mid-sentence,
 * and the whole argument for not focusing the affirmative below is that a
 * stray keystroke must not become a grant. A chord is not a stray keystroke.
 * The two widest grants have no key at all: "always" is a standing decision,
 * and a standing decision is worth a deliberate press.
 */
export function PermissionDialog({
  request,
  onDecide,
}: {
  request: PermissionRequest;
  onDecide: (decision: PermissionDecision) => void;
}) {
  // Captured, for the same reason `Modal` captures Escape: nothing inside this
  // panel has a use for either chord, and a control that swallowed one would
  // leave a key printed on a button that does nothing.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      // Shift is refused explicitly, the way `App` refuses it on ⌘N: `isChord`
      // leaves that modifier to its caller, because ⌘⇧P and ⌘P are two
      // different chords there. Here there is no shifted variant to reach —
      // and a grant is not something to hand out to a near miss.
      if (e.shiftKey) return;
      if (isChord(e, "Enter")) {
        e.preventDefault();
        onDecide("allow_once");
      } else if (isChord(e, "Backspace")) {
        e.preventDefault();
        onDecide("deny");
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
    // Rebound per request: `onDecide` closes over which call is being answered,
    // and a handler left over from the previous one would answer the wrong
    // question.
  }, [onDecide, request.id]);

  return (
    <Modal>
      <div className="dialog" role="alertdialog" aria-labelledby="perm-title">
        <div className="dialog-head">
          <span className={`effect ${request.effect}`}>{VERB[request.effect]}</span>
          <h2 id="perm-title">
            Taurus {EFFECT_LABEL[request.effect] ?? "wants permission"}
          </h2>
        </div>

        <pre className="dialog-detail">
          {request.effect === "execute" && <span className="prompt">❯ </span>}
          {request.preview}
        </pre>

        {/* Present only for the two tools that can work out a before and an
            after. A command line and a URL have none, and the line above is
            already the whole of what is being approved for those. */}
        {request.diff && <DiffView diff={request.diff} />}

        {/* The scope sentence describes the standing grant, so it goes away
            with it: in an untrusted workspace there is nowhere to keep one, and
            a footnote about a button that is not there reads as a bug. */}
        {request.offer_always && (
          <p className="dialog-footnote">{request.always_scope}.</p>
        )}

        <div className="dialog-actions">
          {/* Not `autoFocus`. Focus lands on the dialog itself — see `Modal` —
              because a prompt that opens with the affirmative already focused
              turns a stray Enter, from someone typing when it appeared, into a
              grant they never read. Tab reaches it in one press. */}
          <button className="primary" onClick={() => onDecide("allow_once")}>
            Allow once
            <span className="key">{chord("↵")}</span>
          </button>
          {/* Absent in a workspace whose own config is not being read: there
              is no workspace layer to write a standing decision into, so the
              engine would honor this once and quietly forget it. Offering a
              permanence that will not happen is worse than not offering it. */}
          {request.offer_always && (
            <button
              onClick={() => onDecide("allow_always")}
              data-tip={request.always_scope}
            >
              {request.always_global_scope ? "Always here" : "Allow always"}
            </button>
          )}
          {/* Absent where the wider grant is not on offer — running commands,
              where "every project you ever open" is the wrong unit. */}
          {request.always_global_scope && (
            <button
              onClick={() => onDecide("allow_always_global")}
              data-tip={request.always_global_scope}
            >
              Always everywhere
            </button>
          )}
          <div className="spacer" />
          <button className="danger" onClick={() => onDecide("deny")}>
            Deny
            <span className="key">{chord("⌫")}</span>
          </button>
        </div>
      </div>
    </Modal>
  );
}

/** The chip above the call, in the imperative the user is being asked about. */
const VERB: Record<string, string> = {
  read: "read",
  write: "write",
  execute: "run",
  network: "network",
};
