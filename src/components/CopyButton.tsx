import { useState } from "react";

/**
 * Copies text, and says so for a moment.
 *
 * Shared rather than written per card because the acknowledgement is the part
 * that matters and the part most easily left out: a copy button that does
 * nothing visible is indistinguishable from one that failed, and the clipboard
 * is somewhere the user cannot check without leaving the app.
 */
export function CopyButton({
  text,
  className,
  label = "copy",
}: {
  /** Produced on click rather than up front, so a large table is only
   * serialized when somebody asks for it. */
  text: string | (() => string);
  className?: string;
  label?: string;
}) {
  // Said either way, for the reason above: a refused copy that said nothing
  // looked like a button nobody had pressed.
  const [said, setSaid] = useState<"copied" | "couldn't copy" | null>(null);
  const say = (word: "copied" | "couldn't copy") => {
    setSaid(word);
    setTimeout(() => setSaid(null), 1200);
  };

  return (
    <button
      className={className}
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(
            typeof text === "function" ? text() : text,
          );
          say("copied");
        } catch {
          // Clipboard access can be refused. One word in the button, not an
          // alert: it's said where the person is looking.
          say("couldn't copy");
        }
      }}
    >
      {said ?? label}
    </button>
  );
}
