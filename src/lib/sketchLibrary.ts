/**
 * Where the sketch editor's library is kept: Excalidraw's persistence adapter,
 * backed by `~/.taurus/sketch-library.excalidrawlib`.
 *
 * One library for every sketch, in every folder, the way Excalidraw's own is.
 * A file rather than the webview's storage because the shapes are work, not a
 * preference, and clearing site data shouldn't take them. The format is
 * Excalidraw's own, so a library downloaded from the public site can be put in
 * its place.
 *
 * Kept free of Excalidraw imports, so it can be tested without loading the
 * editor. Excalidraw restores whatever version of item it's handed, so the
 * items go through as they are.
 */
import * as api from "./api";

/** Where the file is, as it's named to a person. */
export const LIBRARY_FILE = "~/.taurus/sketch-library.excalidrawlib";

/** A library's items as this module sees them: opaque. */
type Items = readonly unknown[];

/**
 * An adapter for `useHandleLibrary`.
 *
 * `report` is told of anything that went wrong, once, in a sentence a person
 * can act on. A file that's there but can't be read is never written over for
 * as long as this adapter lives: the shapes in it are still somebody's, and
 * the fix is theirs to make. That's the rule `settings.json` follows.
 */
export function libraryAdapter<Loaded, Saved extends Items>(report: (message: string) => void) {
  let unreadable = false;
  let reported = false;
  const once = (message: string) => {
    if (reported) return;
    reported = true;
    report(message);
  };
  return {
    /** `Loaded` is the editor's own type for what it reads: the file is
     *  checked to be a library, and each item is Excalidraw's to restore. */
    async load(): Promise<{ libraryItems: Loaded } | null> {
      let text: string | null;
      try {
        text = await api.readSketchLibrary();
      } catch (e) {
        unreadable = true;
        once(`${String(e)} Shapes you add to the library won't be saved until it can be read.`);
        return null;
      }
      if (text === null) return null;
      const items = libraryItemsOf(text);
      if (items === null) {
        unreadable = true;
        once(
          `${LIBRARY_FILE} isn't a library Taurus can read, so shapes you add won't be saved over it. Fix or delete the file, then open the sketch again.`,
        );
        return null;
      }
      return { libraryItems: items as Loaded };
    },
    async save({ libraryItems }: { libraryItems: Saved }): Promise<void> {
      if (unreadable) return;
      try {
        await api.saveSketchLibrary(libraryFile(libraryItems));
      } catch (e) {
        once(String(e));
      }
    },
  };
}

/**
 * The items in a library file, or `null` if it isn't one.
 *
 * Both of Excalidraw's versions: 2 lists items under `libraryItems`, and 1
 * listed bare element groups under `library`.
 */
export function libraryItemsOf(text: string): Items | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  const file = parsed as { type?: unknown; libraryItems?: unknown; library?: unknown };
  if (file.type !== "excalidrawlib") return null;
  if (Array.isArray(file.libraryItems)) return file.libraryItems;
  if (Array.isArray(file.library)) return file.library;
  return null;
}

/** The file for `items`, in the shape Excalidraw's own export writes. */
export function libraryFile(items: Items): string {
  return JSON.stringify({ type: "excalidrawlib", version: 2, source: "taurus", libraryItems: items }, null, 2);
}
