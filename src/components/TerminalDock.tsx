import { useCallback, useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal, type IDecoration, type IMarker } from "@xterm/xterm";
import "../vendor.css";

import * as api from "../lib/api";
import type { BackgroundJob, Block } from "../lib/api";
import { useWindowPalette } from "../lib/windowTheme";
import { basename } from "../lib/format";
import {
  acknowledger,
  askAboutBlock,
  blockMark,
  blockState,
  bytes,
  describeBlock,
  duration,
  fade,
} from "../lib/terminal";
import { DockTabs, JobScreen } from "./JobScreen";

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
 *
 * # The other tabs
 *
 * A background command gets one each — see `JobScreen`, which is a separate
 * module so that drawing one does not mean loading an emulator. The shell tab
 * is not torn down when another is selected, only hidden: it is a live session
 * whose scrollback is the only record of it, and unmounting it would end the
 * shell.
 *
 * # Commands
 *
 * A shell started with Taurus's integration marks where each command starts
 * and ends (see `crates/taurus-tools/src/shell_integration.rs`). The backend turns those
 * marks into blocks; this pane draws a mark in the left margin beside each
 * command's line, colored by how it ended, and keeps one command in the bar —
 * the latest, or whichever mark was clicked — with **Ask Taurus** beside it.
 * ⌘↑ and ⌘↓ (Ctrl+Shift off macOS) step between commands.
 *
 * The line a mark belongs on is only known here, because only the emulator
 * knows where its cursor was. So this pane watches for the same marks in the
 * bytes it draws, and pairs the n-th command it sees with the backend's block
 * number n. Both count the same marks in the same stream, so they agree.
 */
export function TerminalDock({
  workspace,
  jobs,
  watching,
  output,
  onWatch,
  onStop,
  onClose,
  onAsk,
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
  /**
   * The background commands, polled by `App`.
   *
   * Polled up there rather than here because the count outlives this
   * component: the dock is unmounted when it is hidden, and a build that
   * starts while it is closed is exactly the one worth a badge on the way in.
   */
  jobs: BackgroundJob[];
  /** Which tab is on screen. `null` is the shell. */
  watching: number | null;
  /** Everything the watched command has said, gaps marked. Also `App`'s, for
   *  the same reason: it is collected by the poll that fetches it. */
  output: string;
  onWatch: (id: number | null) => void;
  onStop: (id: number) => Promise<unknown>;
  onClose: () => void;
  /** Puts a draft in the composer, the way the other panes' "ask" buttons do. */
  onAsk: (draft: string) => void;
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
  /**
   * Why a Stop did not take, and which command it did not take on.
   *
   * Its own state rather than `problem`, which belongs to the shell: the two
   * fail at different things and a message from one shown over the other
   * describes the wrong process. Carrying the number is the same argument one
   * tab further in — a failure left standing over the next tab is a sentence
   * about a command that is not the one on screen.
   */
  const [stopFailed, setStopFailed] = useState<{ id: number; why: string } | null>(
    null,
  );
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
  /**
   * The commands this shell has run, by number, newest last.
   *
   * Only what the bar needs. The output stays in the backend, which is the one
   * place that holds it, and is fetched when somebody asks.
   */
  const [blocks, setBlocks] = useState<Block[]>([]);
  /** The command in the bar, when it isn't simply the latest. */
  const [chosen, setChosen] = useState<number | null>(null);
  const chosenRef = useRef<number | null>(null);
  chosenRef.current = chosen;
  /** Each command's line in the scrollback, by block number. */
  const markers = useRef<Map<number, IMarker>>(new Map());

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
      // Decorations — the marks beside each command — are still flagged as
      // proposed API in xterm 6, though VS Code's terminal is built on them.
      // Nothing else proposed is used here.
      allowProposedApi: true,
      // The strip beside the scrollbar where a failed command leaves a tick.
      overviewRuler: { width: 6 },
    });
    const fitter = new FitAddon();
    emulator.loadAddon(fitter);
    emulator.open(element);
    fitter.fit();

    term.current = emulator;
    fit.current = fitter;

    // Watching for the integration's marks, to know which line each command
    // is on. `B` is where the prompt ends and the command line begins, which
    // is the line a mark belongs beside; `C` is the command starting. A `B`
    // with no `C` after it — Enter on an empty line — is let go.
    setBlocks([]);
    setChosen(null);
    const lines = new Map<number, IMarker>();
    markers.current = lines;
    const decorations = new Map<number, IDecoration>();
    let typing: IMarker | null = null;
    let started = 0;
    const marks = emulator.parser.registerOscHandler(133, (data) => {
      if (data === "B") {
        typing?.dispose();
        typing = emulator.registerMarker(0);
      } else if (data === "C" || data.startsWith("C;")) {
        started += 1;
        lines.set(started, typing ?? emulator.registerMarker(0));
        typing = null;
      }
      // Not consumed: nothing else here wants them, and the emulator draws
      // nothing for a mark either way.
      return false;
    });

    // Draws, or redraws, one command's mark in the margin.
    const decorate = (block: Block) => {
      const line = lines.get(block.id);
      if (!line || line.isDisposed) return;
      decorations.get(block.id)?.dispose();
      const failed = blockState(block) === "failed";
      const decoration = emulator.registerDecoration({
        marker: line,
        width: 1,
        layer: "top",
        // A failure is also a tick on the scrollbar, so a red build an hour of
        // scrollback ago can still be found by eye.
        overviewRulerOptions: failed ? { color: read("--danger") || "#ff9a9a" } : undefined,
      });
      if (!decoration) return;
      decorations.set(block.id, decoration);
      decoration.onRender((element) => {
        element.classList.add("dock-mark");
        element.dataset.block = String(block.id);
        element.dataset.state = blockState(block);
        element.dataset.chosen = String(chosenRef.current === block.id);
        element.setAttribute("role", "button");
        element.setAttribute("aria-label", describeBlock(block));
        element.title = describeBlock(block);
        element.onmousedown = (event) => {
          // Before the dock's own mousedown, which would put focus back in
          // the emulator and start a selection there.
          event.stopPropagation();
          event.preventDefault();
          setChosen(block.id);
        };
      });
    };

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

    // ⌘↑ / ⌘↓ step between commands, the way iTerm2 and Terminal.app step
    // between marks. Ctrl+Shift elsewhere, because a bare Ctrl+arrow is a
    // word jump a shell's line editor already owns.
    const mac = typeof navigator !== "undefined" && /Mac/.test(navigator.platform);
    emulator.attachCustomKeyEventHandler((event) => {
      if (event.type !== "keydown") return true;
      if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return true;
      const chord = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && event.shiftKey;
      if (!chord) return true;
      const live = [...lines.entries()]
        .filter(([, line]) => !line.isDisposed)
        .sort(([, a], [, b]) => a.line - b.line);
      if (live.length === 0) return false;
      const from =
        lines.get(chosenRef.current ?? -1)?.line ??
        (event.key === "ArrowUp" ? Infinity : -Infinity);
      const next =
        event.key === "ArrowUp"
          ? [...live].reverse().find(([, line]) => line.line < from)
          : live.find(([, line]) => line.line > from);
      if (next) {
        setChosen(next[0]);
        emulator.scrollToLine(Math.max(0, next[1].line - 1));
      }
      return false;
    });

    let live = true;
    const acks = acknowledger((id, n) => void api.ackTerminal(id, n).catch(() => {}));
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
          if (event.kind === "block") {
            const block = event.block;
            // After everything already written has been parsed, so the line
            // its mark was on is known by the time it is drawn.
            emulator.write(new Uint8Array(0), () => {
              if (!live) return;
              setBlocks((all) => {
                const rest = all.filter((b) => b.id !== block.id);
                // The backend keeps the last hundred; so does the bar.
                return [...rest, block].sort((a, b) => a.id - b.id).slice(-100);
              });
              // A new command takes the bar back from one that was clicked.
              if (block.running) setChosen(null);
              decorate(block);
            });
            return;
          }
          if (event.kind === "output") {
            const chunk = bytes(event.data);
            // Acknowledged once drawn, not once received: what the shell waits
            // on is the emulator keeping up, and xterm parses on its own
            // schedule. See `Credit` in `src-tauri/src/terminal.rs`.
            emulator.write(chunk, () => {
              if (live) acks.drawn(chunk.length);
            });
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
        acks.bind(id);
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
      marks.dispose();
      const shells = [...opened.current];
      opened.current.clear();
      for (const id of shells) void api.closeTerminal(id);
      emulator.dispose();
      term.current = null;
      fit.current = null;
    };
  }, [workspace, generation]);

  useEffect(() => start(), [start]);

  // A mark is only redrawn when the emulator redraws its line, so choosing a
  // command from the bar or the keyboard says so to the marks directly.
  useEffect(() => {
    host.current?.querySelectorAll<HTMLElement>(".dock-mark").forEach((mark) => {
      mark.dataset.chosen = String(mark.dataset.block === String(chosen));
    });
  }, [chosen]);

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

  // Follows the window, whatever moved it: the preference, the system's
  // appearance under "system", or one custom theme swapped for another in the
  // same mode. The colours are the stylesheet's, so this only has to re-read
  // them once the new palette is in force, which is what `colours` changing
  // says.
  const colours = useWindowPalette();
  useEffect(() => {
    if (term.current) term.current.options.theme = palette();
  }, [colours]);

  // A tab whose command has gone — a workspace change forgets them — leaves
  // the pane pointed at nothing. Falling back to the shell rather than drawing
  // an empty frame, because the shell is the tab that is always there.
  const shown = jobs.find((job) => job.id === watching) ?? null;
  const onShell = shown === null;
  // The command in the bar: the one clicked, else the latest. Not shown once
  // the shell has gone — its commands went with it, so there is nothing left
  // to ask about.
  const current =
    ended !== null
      ? null
      : (blocks.find((b) => b.id === chosen) ?? blocks[blocks.length - 1] ?? null);
  const ask = (block: Block) => {
    const id = session.current;
    if (!id) return;
    void api
      .terminalBlock(id, block.id)
      .then((text) => onAsk(askAboutBlock(text.block, text.output)))
      .catch((e) => setProblem(String(e)));
  };

  return (
    <section
      className="dock"
      aria-label="Terminal"
      // Clicks anywhere in the chrome put the caret back where typing goes —
      // but only on the tab that has anywhere for it to go.
      onMouseDown={() => onShell && term.current?.focus()}
    >
      <header className="dock-bar">
        {jobs.length > 0 ? (
          <DockTabs jobs={jobs} watching={shown?.id ?? null} onWatch={onWatch} />
        ) : (
          <span className="dock-title">Terminal</span>
        )}
        {/* The folder is the shell's own, so it is said beside the shell and
            not beside a command that may have been started somewhere else. */}
        {onShell && workspace && (
          <span className="dock-where">{basename(workspace)}</span>
        )}
        {onShell && ended !== null && (
          <span className="dock-ended">
            {ended === "gone"
              ? "shell ended"
              : ended === 0
                ? "shell exited"
                : `shell exited ${ended}`}
          </span>
        )}
        {onShell && current && (
          <span className="dock-block" data-state={blockState(current)}>
            <span className="dock-block-mark" aria-hidden="true">
              {blockMark(current)}
            </span>
            <code className="dock-block-command" title={describeBlock(current)}>
              {current.command || "(command line not reported)"}
            </code>
            <span className="dock-block-meta">
              {current.running
                ? "running"
                : [
                    current.exit !== undefined && current.exit !== 0
                      ? `exit ${current.exit}`
                      : null,
                    current.duration_ms !== undefined ? duration(current.duration_ms) : null,
                  ]
                    .filter(Boolean)
                    .join(" · ")}
            </span>
            <button
              className="pill"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => ask(current)}
              aria-label={`Ask Taurus about ${describeBlock(current)}`}
            >
              Ask Taurus
            </button>
          </span>
        )}
        <div className="spacer" />
        {onShell && ended !== null && (
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
      {onShell && problem && <p className="dock-problem">{problem}</p>}
      {/* Hidden rather than unmounted: this is a live shell, and the emulator
          holding it is also the only copy of its scrollback. */}
      <div className="dock-screen" ref={host} hidden={!onShell} />
      {shown && (
        <JobScreen
          key={shown.id}
          job={shown}
          text={output}
          problem={stopFailed?.id === shown.id ? stopFailed.why : null}
          onStop={() => {
            setStopFailed(null);
            void onStop(shown.id).catch((e) =>
              setStopFailed({ id: shown.id, why: String(e) }),
            );
          }}
        />
      )}
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

