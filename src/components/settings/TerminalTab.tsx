// The Terminal tab: which shell the dock starts, the font it draws in, and
// whether it marks each command.

import { useEffect, useState } from "react";

import * as api from "../../lib/api";
import { useStore } from "../../state/store";
import { DEFAULT_TERMINAL_FONT_SIZE } from "../../lib/limits";
import { bare, families, fixedPitch } from "../../lib/fixedPitch";
import { Problem } from "../Problem";
import { Field, FromProject } from "./Field";

const MIN_SIZE = 9;
const MAX_SIZE = 24;

/**
 * The dock's shell and how it looks.
 *
 * Each field commits on blur or Enter, like the limits on Behavior: a shell
 * name typed a letter at a time would otherwise be checked against the PATH
 * at every letter, and refused at every one but the last.
 *
 * The shell applies to the next one opened. The font applies to the one
 * that's open, because changing it under a running shell is harmless and
 * waiting for a new one to see it would make the field look broken.
 */
export function TerminalTab() {
  const status = useStore((s) => s.status);
  const refresh = useStore((s) => s.refresh);
  const settings = status?.settings;
  const savedShell = settings?.terminal_shell ?? "";
  const savedFont = settings?.terminal_font ?? "";
  const savedSize = settings?.terminal_font_size ?? DEFAULT_TERMINAL_FONT_SIZE;

  const [shell, setShell] = useState(savedShell);
  const [family, setFamily] = useState(savedFont);
  const [size, setSize] = useState(String(savedSize));
  const [shellError, setShellError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The store is the authority, as for the limits on Behavior.
  useEffect(() => setShell(savedShell), [savedShell]);
  useEffect(() => setFamily(savedFont), [savedFont]);
  useEffect(() => setSize(String(savedSize)), [savedSize]);

  const commitShell = async () => {
    if (shell.trim() === savedShell) return setShell(savedShell);
    setShellError(null);
    try {
      await api.setTerminalShell(shell);
      await refresh();
    } catch (e) {
      // Left as typed, so the name that was refused is the one on screen
      // beside the reason.
      setShellError(String(e));
    }
  };

  const commitFont = async () => {
    const parsed = Number.parseInt(size, 10);
    const next = Number.isFinite(parsed)
      ? Math.min(MAX_SIZE, Math.max(MIN_SIZE, parsed))
      : savedSize;
    setSize(String(next));
    if (family.trim() === savedFont && next === savedSize) return;
    setError(null);
    try {
      await api.setTerminalFont(family, next);
      await refresh();
    } catch (e) {
      setFamily(savedFont);
      setSize(String(savedSize));
      setError(`The font stayed as it was: ${e}`);
    }
  };

  // The saved font and the theme's mono face, whichever the terminal would
  // have drawn in and won't. Said here because the terminal itself just looks
  // like the font didn't take.
  const chosen = savedFont.trim().replace(/"/g, "");
  const skipped = [...(chosen ? [`"${chosen}"`] : []), ...families(monoFace())].filter(
    (family) => !fixedPitch(family),
  );

  const enter = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") e.currentTarget.blur();
  };

  return (
    <>
      <Field
        label="Shell"
        hint="Empty starts your login shell. A name is looked up on this app's PATH; a full path is used as it is. Applies to the next shell you open."
      >
        <input
          className="font-mono"
          aria-label="Shell"
          placeholder="your login shell"
          value={shell}
          onChange={(e) => setShell(e.target.value)}
          onBlur={() => void commitShell()}
          onKeyDown={enter}
        />
      </Field>
      {shellError && <Problem>{shellError}</Problem>}

      <div className="grid grid-cols-[1fr_96px] gap-3">
        <Field label="Font" hint="Empty uses the app's own, which has the symbols prompts draw with.">
          <input
            aria-label="Terminal font"
            placeholder="the app's monospace"
            value={family}
            onChange={(e) => setFamily(e.target.value)}
            onBlur={() => void commitFont()}
            onKeyDown={enter}
          />
        </Field>
        <Field label="Size" hint={`${MIN_SIZE}–${MAX_SIZE}px`}>
          <input
            inputMode="numeric"
            aria-label="Terminal font size"
            value={size}
            onChange={(e) => setSize(e.target.value)}
            onBlur={() => void commitFont()}
            onKeyDown={enter}
          />
        </Field>
      </div>
      {skipped.length > 0 && (
        <Problem>
          {skipped.map((family) => `${bare(family)} isn't fixed-width`).join(", and ")}, so
          the terminal skips it and draws in the next face that is.
        </Problem>
      )}

      {/* Moved here from Behavior, where it was the one switch about the
          terminal among switches about the agent. */}
      <label className="settings-check">
        <input
          type="checkbox"
          checked={settings?.shell_integration ?? true}
          onChange={(e) => {
            const on = e.target.checked;
            setError(null);
            api
              .setShellIntegration(on)
              .then(() => refresh())
              .catch((err) => setError(String(err)));
          }}
        />
        <span>
          Mark each command
          <span className="hint">
            Lets you ask Taurus about a command you ran, and lets Taurus read
            them. Works with zsh and bash. Applies to the next shell you open.
          </span>
        </span>
      </label>
      <FromProject field="shell_integration" />

      {error && <Problem>{error}</Problem>}
    </>
  );
}

/** The theme's mono face as it stands, or the stylesheet's when no theme sets one. */
function monoFace(): string {
  return getComputedStyle(document.documentElement).getPropertyValue("--lk-mono-face").trim();
}
