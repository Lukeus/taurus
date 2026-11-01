/**
 * The hooks file a problem or a refusal names, if it is one you can open.
 *
 * Read out of the sentence, because that's the one place the path travels: a
 * refused prompt comes back as an error string, and a problem is a source and a
 * message. Every form puts the absolute path after a space or `(in `.
 *
 * Only `.taurus/hooks.json`, yours or the project's. Those are the two the canvas
 * can open and that are yours to change. A plugin's file is neither, as a plugin
 * is replaced when it updates.
 */
const NAMED = /(?:^|[\s(])((?:\\\\\?\\)?(?:[A-Za-z]:[\\/]|\/).*?[\\/]\.taurus[\\/]hooks\.json)(?=[\s)]|$)/;

export function hooksFileIn(message: string): string | null {
  return NAMED.exec(message)?.[1] ?? null;
}
