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
  const [copied, setCopied] = useState(false);

  return (
    <button
      className={className}
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(
            typeof text === "function" ? text() : text,
          );
          setCopied(true);
          setTimeout(() => setCopied(false), 1200);
        } catch {
          // Clipboard access can be refused; saying nothing beats an alert.
        }
      }}
    >
      {copied ? "copied" : label}
    </button>
  );
}
