/**
 * The choice between two versions of one file: the one on screen, unsaved,
 * and the one somebody else wrote while it was being typed.
 *
 * Both are kept and neither is chosen, which is the whole rule: the one thing
 * that must not happen is the app deciding whose work to throw away. The canvas
 * and the notes pane each drew this by hand; they share it now, and keep their
 * own look through `className`.
 *
 * Not "wrote over it" — nothing was overwritten, which is the entire point. The
 * save was refused, so both versions exist, and the sentence has to say that
 * rather than describe a loss that did not happen.
 */
export function ConflictBanner({
  className,
  title,
  onTakeTheirs,
  onKeepMine,
}: {
  /** The pane's own class; the sentence block takes it with `-say`. */
  className: string;
  /** What changed, in the pane's words. */
  title: string;
  onTakeTheirs: () => void;
  onKeepMine: () => void;
}) {
  return (
    <div className={className} role="alert">
      <div className={`${className}-say`}>
        <b>{title}</b>
        <span>Your version is still here, unsaved.</span>
      </div>
      <button className="pill" onClick={onTakeTheirs}>
        Take theirs
      </button>
      <button className="pill primary" onClick={onKeepMine}>
        Keep mine
      </button>
    </div>
  );
}
