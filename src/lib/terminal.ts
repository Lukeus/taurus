/**
 * The two pieces of the terminal dock that are arithmetic rather than emulator.
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
