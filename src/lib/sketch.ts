import type { BinaryFiles, ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";

/**
 * A sketch file, read into what Excalidraw is given on load — or why it cannot be.
 *
 * Its own module, and type-only in what it takes from Excalidraw, so it can be
 * tested without loading a canvas library into a test environment that has no
 * canvas. The editor is the only caller, and the refusal is the part that
 * matters: an editor that opened an empty canvas over a file it failed to read
 * would save that empty canvas over the file a moment later.
 *
 * Checked for the one shape everything else depends on — an object with an
 * `elements` array — and handed over otherwise as it is. Excalidraw restores
 * its own initial data, filling defaults and migrating older versions, and doing
 * that here as well would be a second opinion on its own format.
 */
export type Parsed =
  | { ok: true; data: ExcalidrawInitialDataState }
  | { ok: false; why: string };

export function parse(text: string): Parsed {
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch (e) {
    return { ok: false, why: `It is not valid JSON: ${(e as Error).message}` };
  }
  if (
    !data ||
    typeof data !== "object" ||
    !Array.isArray((data as { elements?: unknown }).elements)
  ) {
    return {
      ok: false,
      why: "It is JSON, but not an Excalidraw drawing — there is no list of elements in it.",
    };
  }
  const scene = data as {
    elements: ExcalidrawInitialDataState["elements"];
    appState?: ExcalidrawInitialDataState["appState"];
    files?: BinaryFiles;
  };
  return {
    ok: true,
    data: {
      elements: scene.elements,
      appState: scene.appState ?? {},
      files: scene.files ?? {},
      scrollToContent: true,
    },
  };
}
