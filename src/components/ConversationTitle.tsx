import { useCallback, useRef, useState } from "react";

/**
 * The conversation's name, in the topbar, editable in place.
 *
 * A title is derived from the first thing asked, which is a good name for about
 * as long as the conversation is about that. Past a few turns it is a name for
 * something the conversation has moved on from, and the rail — which is the
 * app's spine — is a column of them. So the one place the name is already shown
 * at full width is the place it can be changed.
 *
 * In place rather than behind a dialog: renaming is a small enough act that
 * being asked to confirm it would cost more than getting it wrong. Getting it
 * wrong costs one more edit, and emptying the field puts the derived title back.
 */
export function ConversationTitle({
  title,
  renamable,
  onRename,
}: {
  /** What the conversation is called now — given or derived, whichever is in
   *  force. This is what the field opens holding. */
  title: string;
  /**
   * Whether there is a transcript to write a title into.
   *
   * A conversation with no turns is not on disk yet, so there is nothing to
   * name. It reads as plain text rather than offering an edit that would come
   * back as an error about a session that does not exist.
   */
  renamable: boolean;
  onRename: (title: string) => void;
}) {
  /** The text being typed, or null when not editing. */
  const [draft, setDraft] = useState<string | null>(null);
  /**
   * Set by Escape so the blur that follows does not save what Escape just
   * discarded. React does not fire blur on unmount, so this is belt to that
   * brace rather than the mechanism.
   */
  const abandoned = useRef(false);

  /**
   * Stable, so React runs it when the field mounts and not on every render.
   * An inline callback ref is a new function each time, which React answers by
   * detaching and reattaching — and reselecting the text after every keystroke,
   * which makes the field impossible to type in.
   */
  const selectAll = useCallback((el: HTMLInputElement | null) => el?.select(), []);

  const commit = () => {
    if (draft === null) return;
    const next = draft.trim();
    setDraft(null);
    if (abandoned.current) {
      abandoned.current = false;
      return;
    }
    // Includes opening the field and closing it untouched, which is the most
    // common way this ends and should cost nothing.
    if (next === title) return;
    onRename(next);
  };

  if (draft === null) {
    if (!renamable) {
      return <span className="topbar-title">{title}</span>;
    }
    return (
      <button
        className="topbar-title topbar-rename"
        data-tip="Rename this conversation"
        onClick={() => setDraft(title)}
      >
        {title}
      </button>
    );
  }

  return (
    <input
      className="topbar-title topbar-title-input"
      aria-label="Conversation title"
      value={draft}
      // Held open rather than trimmed as it is typed: a space between two words
      // is a space, and normalizing on the way in fights the person typing.
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          commit();
        } else if (e.key === "Escape") {
          e.preventDefault();
          abandoned.current = true;
          setDraft(null);
        }
      }}
      // Selected rather than placed at the end: the field opens holding a name
      // that is usually being replaced rather than appended to.
      ref={selectAll}
      placeholder="Name this conversation"
    />
  );
}
