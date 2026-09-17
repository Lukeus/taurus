import { useEffect, useRef, useState } from "react";
import { Attachments } from "./Attachments";
import { CommandMenu, commandQuery, matches } from "./CommandMenu";
import { ContextMeter } from "./ContextMeter";
import type { Attachment, CommandSummary, OnScreen } from "../lib/api";
import { basename } from "../lib/format";
import { isImage, toAttachments } from "../lib/images";
import type { Outgoing } from "../state/store";
import * as api from "../lib/api";

/**
 * What the box says once a draft is put in it.
 *
 * Extracted for the same reason `onScreenFor` is: nothing here renders, and
 * the decision is the whole of it. A draft is offered by a button that has no
 * idea whether somebody is mid-sentence — the query box and the recipe list
 * are three inches from the composer, and both are places you might have
 * started typing before something failed — so it is added to what is there
 * rather than put in its place. Nothing typed is ever lost to a canned
 * sentence; the cost is a blank line.
 */
export function withDraft(typed: string, draft: string): string {
  return typed.trim() ? `${typed.trim()}\n\n${draft}` : draft;
}

/**
 * What a conversation had in its composer when it was last on screen.
 *
 * The images are here as well as the text because they are half the message:
 * a pasted screenshot is not something anybody wants to find again in their
 * clipboard history because they clicked another conversation.
 */
export type Parked = { text: string; images: Attachment[] };

/*
 * Exported for its own tests. Everything interesting about it is stateful —
 * what it holds on to across a conversation change, what Enter does while a
 * turn runs, what the line under the box says — and none of that is visible in
 * a render of `App`.
 */
export function Composer({
  busy,
  stopping,
  ready,
  vision,
  workspace,
  unattended,
  library,
  onScreen,
  draft,
  focus,
  sessionKey,
  parked,
  queued,
  onPark,
  onPickWorkspace,
  onUnattended,
  onSend,
  onSendQueued,
  onUnqueue,
  onStop,
  onUsage,
}: {
  busy: boolean;
  /** Pressed Stop, and the turn has not finished unwinding yet. */
  stopping: boolean;
  ready: boolean;
  /**
   * Whether this session's model reads images.
   *
   * Decides whether a paste or a drop is taken at all. Accepting one on a model
   * that cannot see would be an invitation to a refusal — the backend would
   * turn it down, correctly, one round trip later.
   */
  vision: boolean;
  workspace: string | null;
  /**
   * Whether this conversation runs with nobody to answer it.
   *
   * Beside Stop because it belongs to the same question — what happens while
   * this works — and because the moment somebody wants it is the moment they
   * are looking at a turn they are about to walk away from.
   */
  unattended: boolean;
  /**
   * A signature of how many skills and agents there are.
   *
   * Not shown. It is what the `/` namespace re-reads on: the list below is the
   * last scan the backend did, and this changing is the only signal that it did
   * another one. See where `App` builds it.
   */
  library: string;
  /**
   * What the Data pane is showing, when it is what is on screen.
   *
   * Two jobs, and the second is why it is a prop rather than something the
   * store works out at send time. It travels with the message, and it is drawn
   * as a chip above the box — because context the user cannot see is behaviour
   * they cannot explain, and this is the only moment they can see it.
   */
  onScreen: OnScreen | null;
  /**
   * Something to say, handed over from a button outside the composer.
   *
   * An object rather than a string so that clicking the same button twice is
   * two events. `null` until something offers one, which for most sessions is
   * never.
   */
  draft: { text: string } | null;
  /**
   * A request from somewhere else in the window to put the cursor here.
   *
   * A counter rather than a boolean, so asking twice is two events. What it
   * closes is the last gap in getting around without the mouse: once focus is
   * in the terminal, the canvas or the query box, every key belongs to that
   * thing, and the way back to the one control the whole window is arranged
   * around was a click.
   */
  focus: number;
  /**
   * Which conversation is being typed at, captured at mount.
   *
   * The composer is keyed on this, so it is constant for the life of one
   * instance — which is what makes it safe to hand back on the way out. Read
   * from a ref during teardown it would already name the conversation being
   * switched *to*, and every draft would be parked under the wrong one.
   */
  sessionKey: string;
  /** What was left in the box last time this conversation was open. */
  parked: Parked | null;
  /**
   * A message typed while the previous turn was still running.
   *
   * Drawn above the box, and this is the whole visible half of the queue: the
   * store holds it, and until something says so on screen a message that has
   * not been sent yet is indistinguishable from one that has.
   */
  queued: Outgoing | null;
  onPark: (sessionKey: string, draft: Parked) => void;
  onPickWorkspace: () => void;
  onUnattended: (unattended: boolean) => void;
  onSend: (
    text: string,
    images: Attachment[],
    onScreen: OnScreen | null,
  ) => void;
  onSendQueued: () => void;
  onUnqueue: () => void;
  onStop: () => void;
  /** Opens the context account. The meter this sits behind says how full the
   *  window is; this is where "of what" is answered, and one click apart is
   *  the right distance between the two. */
  onUsage: () => void;
}) {
  const [text, setText] = useState(parked?.text ?? "");
  const [commands, setCommands] = useState<CommandSummary[]>([]);
  const [active, setActive] = useState(0);
  const [images, setImages] = useState<Attachment[]>(parked?.images ?? []);
  const [attachError, setAttachError] = useState<string | null>(null);
  // Tracked with a counter rather than a boolean: dragging over a child fires
  // `dragleave` on the parent, so a boolean flickers the highlight off while
  // the file is still over the composer.
  const [dragDepth, setDragDepth] = useState(0);
  const box = useRef<HTMLTextAreaElement>(null);

  const attach = async (files: File[]) => {
    const wanted = files.filter(isImage);
    // A drag carrying no image at all is not this component's business — a
    // file dropped on the composer is a mistake worth naming, but text dragged
    // from another window is not.
    if (wanted.length === 0) {
      if (files.length > 0) {
        setAttachError("Only images can be attached. Use PNG, JPEG, WebP, or GIF.");
      }
      return;
    }
    const { attachments, errors } = await toAttachments(wanted, images.length);
    if (attachments.length > 0) setImages((held) => [...held, ...attachments]);
    setAttachError(errors.length > 0 ? errors.join(" ") : null);
  };

  /*
   * Takes a draft from elsewhere in the window and puts the cursor after it.
   *
   * Added to rather than substituted for what is already typed. A draft is
   * offered by a button that has no idea whether somebody is mid-sentence, and
   * throwing away a half-written question to make room for a canned one is the
   * app deciding it knows better. Appending costs a blank line and loses
   * nothing.
   *
   * The cursor goes to the end because that is where the sentence continues:
   * every one of these drafts ends on the ask, and the next thing typed
   * qualifies it.
   */
  /*
   * Hands whatever is in the box back on the way out.
   *
   * The composer used to outlive the conversation it was typed into: switching
   * in the rail changed the transcript underneath a half-written question,
   * which then went to whichever conversation happened to be open when Enter
   * was finally pressed — with any pasted screenshots along with it. Keying
   * this component on the conversation fixes that and would, on its own,
   * replace one wrong behaviour with another: silently throwing the sentence
   * away. So it is parked instead, and comes back when that conversation does.
   *
   * Registered once and read through a ref, because what it has to save is the
   * state at teardown and an effect that re-ran per keystroke to keep a
   * closure fresh would be a subscription rebuilt on every letter typed.
   */
  const held = useRef<Parked>({ text: "", images: [] });
  held.current = { text, images };
  useEffect(() => {
    return () => onPark(sessionKey, held.current);
    // Both are constant for this instance: `sessionKey` because the component
    // is keyed on it, `onPark` because `App` holds it steady.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Asked for from the palette or its shortcut. Skips the first render, where
  // the counter is only its starting value and nothing has requested anything.
  const asked = useRef(focus);
  useEffect(() => {
    if (focus === asked.current) return;
    asked.current = focus;
    box.current?.focus();
  }, [focus]);

  useEffect(() => {
    if (!draft) return;
    setText((typed) => withDraft(typed, draft.text));
    const box_ = box.current;
    if (!box_) return;
    box_.focus();
    // After the value lands, not before — the caret is set on the text that is
    // in the box now, and React has not written the new one yet.
    requestAnimationFrame(() => {
      const end = box_.value.length;
      box_.setSelectionRange(end, end);
    });
  }, [draft]);

  // Fetched once the composer is usable, again whenever a workspace change
  // could have brought a different library with it, and again whenever a rescan
  // found a different number of things to offer.
  useEffect(() => {
    if (!ready) return;
    api.listCommands().then(setCommands).catch(() => setCommands([]));
  }, [ready, workspace, library]);

  const query = commandQuery(text);
  const shown = query === null ? [] : matches(commands, query);
  // Clamped rather than reset: the list narrows as the user types, and a
  // highlight left pointing past the end would send the wrong command on Enter.
  const index = Math.min(active, Math.max(shown.length - 1, 0));

  const complete = (command: CommandSummary) => {
    // Trailing space, because every one of these takes arguments and the next
    // thing to happen is typing them.
    setText(`/${command.name} `);
    setActive(0);
  };

  const submit = () => {
    // No longer refused while a turn runs. The box stays typeable through one
    // on purpose, and the send is held rather than dropped — see `queued` in
    // the store, and the line below the box that says which of the two just
    // happened.
    if (!text.trim()) return;
    onSend(text, images, onScreen);
    setText("");
    setImages([]);
    setAttachError(null);
    setActive(0);
    // Sending with Enter never lost focus; sending with the button did, so the
    // next thing typed went nowhere. Both routes end in the same place.
    box.current?.focus();
  };

  return (
    <footer className="composer">
      {/* The same argument as the dataset line below, applied to the window the
          message is about to go into: above the box, where you are already
          looking, and silent until there is something worth saying. */}
      <ContextMeter onOpen={onUsage} />
      {/* Above the box rather than inside it: this is not something you typed
          and must not read as though it were. It says what the message is
          about to carry, in the one place you are already looking. */}
      {onScreen?.data && (
        <div className="composer-context" data-tip={onScreen.data.path}>
          <span className="dataset-mark">▦</span>
          <span>
            asking about <b>{onScreen.data.dataset}</b>
          </span>
          {onScreen.data.sql && <span className="micro">· and the query below</span>}
        </div>
      )}
      {/* The canvas's own chip, and it earns its place more than the dataset's
          does: a selection is invisible the moment focus leaves the editor, so
          without this the message carries forty lines the user can no longer
          see it carrying. */}
      {onScreen?.document && (
        <div className="composer-context" data-tip={onScreen.document.path}>
          <span className="dataset-mark">¶</span>
          <span>
            asking about <b>{onScreen.document.path.split("/").pop()}</b>
          </span>
          {onScreen.document.selection && (
            <span className="micro">
              ·{" "}
              {onScreen.document.selection.from === onScreen.document.selection.to
                ? `line ${onScreen.document.selection.from}`
                : `lines ${onScreen.document.selection.from}–${onScreen.document.selection.to}`}
            </span>
          )}
        </div>
      )}
      {/*
        * A message typed ahead, sitting where it can be seen.
        *
        * The row is not decoration: without it the queue would be a promise
        * the app made silently, and the failure mode of a silent promise is
        * the one this replaced — an Enter that appeared to do nothing.
        *
        * It says two different things depending on what happened to the turn
        * in front of it. Still running: this goes next, and there is an ✕ to
        * change your mind. Finished badly, or stopped: it did *not* go, and
        * the sentence is still here with a button to send it by hand. See the
        * drain in `send` for why a turn that broke does not fire it for you.
        */}
      {queued && (
        <div className={`composer-queued${busy ? "" : " held"}`}>
          <span className="dataset-mark">↳</span>
          <span className="composer-queued-text">{queued.text}</span>
          <div className="spacer" />
          {!busy && (
            <button className="quiet" onClick={onSendQueued}>
              Send it
            </button>
          )}
          <button className="quiet" onClick={onUnqueue} aria-label="Discard this message">
            ✕
          </button>
        </div>
      )}
      <div
        className={`composer-box${dragDepth > 0 ? " dropping" : ""}`}
        // The whole box is the target, not the textarea: someone dragging a
        // screenshot aims at the thing that looks like the message, and the
        // textarea is one row tall until it is typed into.
        onDragEnter={(e) => {
          if (!vision || !ready) return;
          e.preventDefault();
          setDragDepth((d) => d + 1);
        }}
        onDragOver={(e) => {
          if (vision && ready) e.preventDefault();
        }}
        onDragLeave={() => setDragDepth((d) => Math.max(0, d - 1))}
        onDrop={(e) => {
          if (!vision || !ready) return;
          e.preventDefault();
          setDragDepth(0);
          void attach([...e.dataTransfer.files]);
        }}
      >
        {shown.length > 0 && (
          <CommandMenu commands={shown} active={index} onPick={complete} />
        )}

        {images.length > 0 && (
          <Attachments images={images} onRemove={(i) =>
            setImages((held) => held.filter((_, n) => n !== i))
          } />
        )}

        {attachError && <p className="composer-problem">{attachError}</p>}

        <textarea
          ref={box}
          value={text}
          placeholder={ready ? "Ask Taurus to do something…" : "Connect a model to begin"}
          disabled={!ready}
          rows={1}
          onChange={(e) => {
            setText(e.target.value);
            setActive(0);
          }}
          onPaste={(e) => {
            const files = [...e.clipboardData.files];
            if (files.length === 0) return;
            // Only once there is an image in it: pasting a file path as text
            // is still a paste, and stealing it would break copying a path in.
            if (!files.some(isImage)) return;
            if (!vision) {
              setAttachError(
                "This model cannot read images. Switch to a vision model — on Ollama that is one like qwen3-vl or llava.",
              );
              return;
            }
            e.preventDefault();
            void attach(files);
          }}
          onKeyDown={(e) => {
            // The menu takes the keys it needs and lets every other one
            // through, so typing never has to wait for it to be dismissed.
            if (shown.length > 0) {
              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                const step = e.key === "ArrowDown" ? 1 : -1;
                setActive((index + step + shown.length) % shown.length);
                return;
              }
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
                e.preventDefault();
                complete(shown[index]);
                return;
              }
              if (e.key === "Escape") {
                e.preventDefault();
                // Settles the name so the menu closes without discarding what
                // was typed — Escape here means "stop suggesting", not "undo".
                setText(`${text} `);
                return;
              }
            }
            // Enter sends; Shift+Enter is a newline, matching every chat UI.
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
        />
        <div className="composer-foot">
          <button
            className="pill"
            // The same rule the rail's workspace row follows: a switch closes
            // the conversation and restarts the MCP servers it changes.
            disabled={busy}
            onClick={onPickWorkspace}
            data-tip={
              busy
                ? "Stop the running turn before switching workspace"
                : (workspace ?? "Choose a workspace")
            }
          >
            ▤ {workspace ? basename(workspace) : "no workspace"}
          </button>
          {/* Two words for the whole of what a turn does when it reaches
              something it needs a person for. Here rather than in Settings
              because it is not a preference: it is about this conversation and
              the next few hours, and it is decided while looking at a turn. */}
          <button
            className={`pill${unattended ? " on" : ""}`}
            onClick={() => onUnattended(!unattended)}
            data-tip={
              unattended
                ? "Running unattended: anything that needs a decision is refused rather than left waiting, and questions are skipped. Nothing extra is allowed."
                : "You are asked before anything that needs a decision, and the turn waits. Switch to unattended to leave a long one running."
            }
          >
            {unattended ? "◐ unattended" : "◑ asks you"}
          </button>
          <div className="spacer" />
          {/* The hint is the only place the slash namespace announces itself,
              and only while there is something in it to run. Paste is the
              same: a model that cannot see must not advertise it. */}
          {/* What Enter will actually do, which stops being "send" for as
              long as a turn is running. Said only while there is something in
              the box for it to do it to, so the line does not spend a turn
              announcing a rule about text that has not been typed. */}
          <span className="composer-hint">
            {busy && text.trim() ? (
              "↵ sends when this turn ends · ⇧↵ newline"
            ) : (
              <>
                ↵ send · ⇧↵ newline{commands.length > 0 && " · / commands"}
                {vision && " · paste an image"}
              </>
            )}
          </span>
          {busy ? (
            // Disabled while it takes, because a second press does nothing the
            // first did not — and a button that still says "Stop" after being
            // pressed reads as one that did not register.
            <button
              className="danger composer-send"
              onClick={onStop}
              disabled={stopping}
            >
              {stopping ? "Stopping…" : "Stop"}
            </button>
          ) : (
            <button
              className="primary composer-send"
              onClick={submit}
              disabled={!ready || !text.trim()}
            >
              Send
            </button>
          )}
        </div>
      </div>
    </footer>
  );
}
