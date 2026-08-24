import { useCallback, useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import * as api from "../lib/api";
import type { Theme } from "../lib/api";
import { basename } from "../lib/format";
import { bytes, fade } from "../lib/terminal";

/**
 * The terminal dock: a real shell, in the window the agent works in.
 *
 * The emulator is xterm.js rather than something written here, and that is the
 * whole reason a full-screen program works in this pane at all. A terminal is
 * not a log with colours in it — it is a screen the program addresses by
 * coordinate, and anything that renders output as appended text breaks `vim`,
 * `htop`, `less`, and every progress bar that redraws its own line. What the
 * backend sends is bytes; what this does is hand them to something that already
 * knows what they mean. See `src-tauri/src/terminal.rs`.
 *
 * Loaded lazily by `App`, with the rest of the panels. It is the largest module
 * in the frontend after Settings and most sessions never open it.
 */
export function TerminalDock({
  workspace,
  theme,
  onClose,
}: {
  /**
   * The folder the shell starts in, and the identity of the dock.
   *
   * Changing workspace tears this component down and builds a new one — see the
   * `key` in `App`. A shell left running in the old folder would keep its own
   * `cd` and quietly disagree with everything else on screen about where the
   * window is pointed.
   */
  workspace: string | null;
  /** Only to re-theme on a change; the colours themselves are read from CSS. */
  theme: Theme;
  onClose: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const fit = useRef<FitAddon | null>(null);
  /**
   * The live session's id, in a ref rather than state.
   *
   * Every reader of it is an event handler — a keystroke, a resize — and none
   * of them should re-run because it arrived. It is also read by the cleanup
   * that closes the shell, which must see the *current* value and not the one
   * captured when the effect ran.
   */
  const session = useRef<string | null>(null);
  /**
   * Every shell this pane has started, including the ones already gone.
   *
   * The single `session` id is not enough to tear down by, and the log of a dev
   * session is what showed it: a component that mounts, opens a shell, and is
   * replaced before that open resolves can end up with a live shell nothing
   * holds the id of. Closing the whole set costs nothing — an id that has
   * already exited is closed quietly — and it cannot miss one.
   */
  const opened = useRef<Set<string>>(new Set());
  const [problem, setProblem] = useState<string | null>(null);
  const [ended, setEnded] = useState<number | null | "gone">(null);
  /**
   * Bumped to start over.
   *
   * Restart is not a separate path: changing this rebuilds `start`, which runs
   * the effect's cleanup — closing the old shell and disposing the emulator it
   * was drawing into — before building the next one. A restart that only
   * spawned a shell would leave the dead terminal's DOM under the new one.
   */
  const [generation, setGeneration] = useState(0);

  /** Builds the shell, and returns the teardown for it. */
  const start = useCallback(() => {
    const element = host.current;
    if (!element) return;

    const emulator = new Terminal({
      // Not `--mono`: a prompt draws its separators and icons from the
      // private-use area, and the app's face has nothing there. See
      // `--mono-terminal` in the stylesheet for what is in the stack and why
      // the names are spelled out.
      fontFamily: read("--mono-terminal") || read("--mono") || "monospace",
      fontSize: 13,
      // A terminal is read in long lines and the app's body leading is set for
      // prose; at the body's ratio the rows drift apart enough that a box-drawn
      // frame stops looking like one.
      lineHeight: 1.2,
      cursorBlink: true,
      // What a shell's own scrollback would be. Past this the pane forgets, and
      // it is the emulator that forgets rather than the shell — nothing here
      // asks the backend to hold a second copy of what is already on screen.
      scrollback: 5_000,
      theme: palette(),
    });
    const fitter = new FitAddon();
    emulator.loadAddon(fitter);
    emulator.open(element);
    fitter.fit();

    term.current = emulator;
    fit.current = fitter;

    // Typed before the shell answers. Held rather than dropped: the open is a
    // round trip, and a keystroke that lands inside it is one the user has
    // already committed to.
    const early: string[] = [];
    // Set once the shell is gone, so that keystrokes after it stop being
    // queued. Without it a pane left open on a dead shell collects everything
    // typed into it forever, waiting for an id that is never coming.
    let gone = false;
    emulator.onData((data) => {
      if (gone) return;
      const id = session.current;
      if (!id) {
        early.push(data);
        return;
      }
      void api.writeTerminal(id, data).catch((e) => setProblem(String(e)));
    });

    emulator.onResize(({ rows, cols }) => {
      const id = session.current;
      if (id) void api.resizeTerminal(id, rows, cols).catch(() => {});
    });

    let live = true;
    api
      .openTerminal(
        emulator.rows,
        emulator.cols,
        (event) => {
          // A shell that has been torn down still has output and an exit in
          // flight, and this component may already be showing its replacement.
          // Without this guard the old shell's exit marked the live pane dead —
          // "shell exited 1" over a working prompt.
          if (!live) return;
          if (event.kind === "output") {
            emulator.write(bytes(event.data));
            return;
          }
          // The shell is gone. The pane stays — its scrollback is the record of
          // what happened in it — but it can no longer be typed into, and
          // saying so is better than swallowing every keystroke after.
          gone = true;
          session.current = null;
          setEnded(event.code ?? "gone");
        },
        workspace ?? undefined,
      )
      .then((id) => {
        // Unmounted while the shell was starting: close the one that just
        // opened rather than leaking it behind a component that is gone.
        if (!live) {
          void api.closeTerminal(id);
          return;
        }

        opened.current.add(id);
        session.current = id;
        // The size may already have moved — the dock can be dragged during the
        // round trip — so this is the real geometry rather than the one asked
        // for.
        void api.resizeTerminal(id, emulator.rows, emulator.cols).catch(() => {});
        for (const data of early.splice(0)) void api.writeTerminal(id, data);
        emulator.focus();
      })
      .catch((e) => setProblem(String(e)));

    return () => {
      live = false;
      gone = true;
      session.current = null;
      const shells = [...opened.current];
      opened.current.clear();
      for (const id of shells) void api.closeTerminal(id);
      emulator.dispose();
      term.current = null;
      fit.current = null;
    };
  }, [workspace, generation]);

  useEffect(() => start(), [start]);

  // The pane is dragged, the window is resized, and the sidebar opens: all of
  // them change how many columns there are, and a shell that is not told wraps
  // at the old one. `ResizeObserver` rather than a window listener because only
  // one of those three is a window event.
  useEffect(() => {
    const element = host.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      // A pane collapsed to nothing measures as zero columns, which xterm
      // rejects and which would be a lie to the shell either way.
      try {
        fit.current?.fit();
      } catch {
        // Mid-teardown, or laid out to nothing. The next observation refits.
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // Follows the app. The colours are the stylesheet's, so this only has to
  // re-read them once the new palette is in force.
  useEffect(() => {
    if (term.current) term.current.options.theme = palette();
  }, [theme]);

  return (
    <section
      className="dock"
      aria-label="Terminal"
      // Clicks anywhere in the chrome put the caret back where typing goes.
      onMouseDown={() => term.current?.focus()}
    >
      <header className="dock-bar">
        <span className="dock-title">Terminal</span>
        {workspace && <span className="dock-where">{basename(workspace)}</span>}
        {ended !== null && (
          <span className="dock-ended">
            {ended === "gone"
              ? "shell ended"
              : ended === 0
                ? "shell exited"
                : `shell exited ${ended}`}
          </span>
        )}
        <div className="spacer" />
        {ended !== null && (
          <button
            className="pill"
            onClick={() => {
              setEnded(null);
              setProblem(null);
              setGeneration((n) => n + 1);
            }}
          >
            Restart
          </button>
        )}
        {/* The glyph is not a name. `title` used to be doing that job here by
            accident; a tip is supplementary and a screen reader may skip it,
            so the label is said outright. */}
        <button
          className="dock-close"
          aria-label="Hide the terminal"
          onClick={onClose}
          data-tip="Hide the terminal (⌃`)"
        >
          ✕
        </button>
      </header>
      {problem && <p className="dock-problem">{problem}</p>}
      <div className="dock-screen" ref={host} />
    </section>
  );
}

/**
 * The emulator's colours, taken from the stylesheet.
 *
 * Read rather than restated, so the dock follows the theme — including the
 * light one, which this app derives itself. The sixteen ANSI colours are the
 * exception: those are what a program *asks* for by name, so they are the
 * palette's own accents where one fits and a conventional value where none
 * does. A `git diff` that asks for red has to be red.
 */
function palette() {
  const bg = read("--bg-sunken") || "#0b0f14";
  const fg = read("--text") || "#eef2f6";
  const accent = read("--accent") || "#7cd2ff";
  const dim = read("--text-dim") || "#90a0b0";
  return {
    background: bg,
    foreground: fg,
    cursor: accent,
    cursorAccent: bg,
    // Enough to be found on either palette without hiding what is under it.
    selectionBackground: fade(accent, 0.3),
    black: read("--lk-ink") || "#0b0f14",
    red: read("--danger") || "#ff9a9a",
    green: read("--ok") || "#a3ffb0",
    yellow: read("--warn") || "#ffbb7c",
    blue: accent,
    magenta: "#d7a5ff",
    cyan: accent,
    white: dim,
    brightBlack: read("--text-faint") || "#5c6a78",
    brightRed: "#ffb3b3",
    brightGreen: "#c2ffcb",
    brightYellow: "#ffd2a5",
    brightBlue: read("--accent-hover") || "#a4e0ff",
    brightMagenta: "#e8c8ff",
    brightCyan: read("--accent-hover") || "#a4e0ff",
    brightWhite: fg,
  };
}

/**
 * One custom property, resolved.
 *
 * Empty where there is no document at all, which is how the tests render this
 * file's siblings — and empty is what every caller's fallback is for.
 */
function read(name: string): string {
  if (typeof document === "undefined") return "";
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

