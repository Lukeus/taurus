/**
 * The pieces of the terminal dock that are arithmetic rather than emulator.
 *
 * Kept out of `TerminalDock` so that testing them does not mean loading a
 * terminal emulator: that module carries the largest import in the frontend,
 * and neither of these has anything to do with it.
 */

/**
 * Base64 to the bytes it stands for.
 *
 * Bytes rather than a string on purpose — see the module note in
 * `src-tauri/src/terminal.rs`. A read from a pty returns whatever the kernel
 * had ready, so a chunk boundary lands in the middle of a multi-byte character
 * whenever a terminal is busy; only the emulator can see both halves. Decoding
 * to text here would turn every one of those into a replacement character.
 */
export function bytes(data: string): Uint8Array {
  const binary = atob(data);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
  return out;
}

/**
 * A colour at partial opacity, as `rgba`.
 *
 * Spelled out rather than handed to `color-mix`, which is a stylesheet feature:
 * the emulator parses its own colours and understands hex and `rgba` and little
 * else, so a `color-mix(...)` here is a selection highlight that silently does
 * not draw. The fallback is the accent, which is what every caller is asking
 * for a shade of.
 */
export function fade(color: string, alpha: number): string {
  const hex = color.trim().replace("#", "");
  const full =
    hex.length === 3
      ? hex
          .split("")
          .map((c) => c + c)
          .join("")
      : hex;
  if (!/^[0-9a-f]{6}$/i.test(full)) return `rgba(124, 210, 255, ${alpha})`;
  const n = parseInt(full, 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
}

/**
 * Acknowledges output once the emulator has drawn it, so the shell can send
 * more. See `Credit` in `src-tauri/src/terminal.rs`.
 *
 * Output can arrive before the pane knows which shell it came from: the
 * channel is live before `terminal_open` answers with the id. What is drawn in
 * that window is held and acknowledged in one go once `bind` names the shell,
 * because a shell whose first screen is never acknowledged has that much less
 * room for the rest of its life.
 */
export function acknowledger(ack: (id: string, bytes: number) => void) {
  let shell: string | null = null;
  let held = 0;
  return {
    /** The emulator has finished with `bytes` of output. */
    drawn(bytes: number) {
      if (shell) ack(shell, bytes);
      else held += bytes;
    },
    /** Names the shell, and acknowledges what was drawn before it had one. */
    bind(id: string) {
      shell = id;
      if (held > 0) ack(id, held);
      held = 0;
    },
  };
}

/** Lines of output "Ask Taurus" quotes. The end, because that's where a
 *  failure is; the model can read the rest with `read_terminal`. */
export const ASK_LINES = 60;

/** Characters of output "Ask Taurus" quotes, for output with very long lines. */
export const ASK_CHARS = 4_000;

/**
 * What "Ask Taurus about this" puts in the composer.
 *
 * The command and the end of its output, quoted, so the message is complete on
 * its own: readable in the transcript a week later, and answerable without a
 * tool call. A failure ends with a question, because that's the question; a
 * success ends open, for the person to say what they want to know. Either way
 * it's a draft, not a send.
 */
export function askAboutBlock(
  block: { id: number; command: string; exit?: number; running: boolean; full_screen: boolean },
  output: string,
): string {
  const command = block.command || "a command";
  const status = block.running
    ? "it's still running"
    : block.exit === undefined
      ? "it ended without reporting a status"
      : block.exit === 0
        ? null
        : `it exited ${block.exit}`;
  const lead = status
    ? `I ran this in the terminal and ${status}:`
    : "I ran this in the terminal:";

  let body = output;
  let cut = false;
  const lines = body.split("\n");
  if (lines.length > ASK_LINES) {
    body = lines.slice(-ASK_LINES).join("\n");
    cut = true;
  }
  if (body.length > ASK_CHARS) {
    body = body.slice(-ASK_CHARS);
    cut = true;
  }
  const shown = block.full_screen
    ? "(a full-screen program; nothing it drew is kept)"
    : body.trim() === ""
      ? "(no output)"
      : body;
  const quoted = fenced(`$ ${command}\n${cut ? "…\n" : ""}${shown}`);
  const more = cut
    ? `\n\nThat's the end of it; the whole output is command #${block.id} in the terminal.`
    : "";
  const ask = status && !block.running ? "\n\nWhat went wrong?" : "";
  return `${lead}\n\n${quoted}${more}${ask}`;
}

/**
 * `text` in a code fence longer than any run of backticks inside it, so output
 * that itself contains a fence (a Markdown file, a test snapshot) can't close
 * this one early.
 */
function fenced(text: string): string {
  const longest = Math.max(0, ...(text.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}\n${text}\n${fence}`;
}

/** A duration as a person says it: `850ms`, `4.2s`, `2m05s`. */
export function duration(ms: number): string {
  if (ms < 1_000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1_000).toFixed(1)}s`;
  const seconds = Math.floor((ms % 60_000) / 1_000);
  return `${Math.floor(ms / 60_000)}m${String(seconds).padStart(2, "0")}s`;
}

/**
 * How a command ended, as one of four words the stylesheet colors.
 *
 * `unknown` is a command that ended without a status — the shell exited under
 * it — which is neither a pass nor a failure and isn't drawn as either.
 */
export function blockState(block: {
  running: boolean;
  exit?: number;
}): "running" | "ok" | "failed" | "unknown" {
  if (block.running) return "running";
  if (block.exit === undefined) return "unknown";
  return block.exit === 0 ? "ok" : "failed";
}

/** The character beside the color, the same three the dock's tabs use (see
 *  `mark` in `lib/jobs`), so the two read as one vocabulary. */
export function blockMark(block: { running: boolean; exit?: number }): string {
  switch (blockState(block)) {
    case "ok":
      return "✓";
    case "failed":
      return "✗";
    case "unknown":
      return "–";
    default:
      return "…";
  }
}

/** What a command's mark says to a screen reader, and in its tooltip. */
export function describeBlock(block: {
  command: string;
  running: boolean;
  exit?: number;
  duration_ms?: number;
}): string {
  const command = block.command || "A command";
  const time = block.duration_ms === undefined ? "" : `, ${duration(block.duration_ms)}`;
  switch (blockState(block)) {
    case "running":
      return `${command}, still running`;
    case "ok":
      return `${command}, succeeded${time}`;
    case "failed":
      return `${command}, exited ${block.exit}${time}`;
    default:
      return `${command}, ended without a status${time}`;
  }
}
