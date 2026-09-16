import type { BinaryFiles, ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";

import { shows, type View } from "./sketchView";

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

export function parse(text: string, view: View | null = null): Parsed {
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
  const appState = scene.appState ?? {};
  const files = scene.files ?? {};
  // Back where it was left, if that still shows some of the drawing — see
  // `sketchView.ts` for why the view is kept here rather than in the file.
  // Otherwise centred on the content: a sketch opened scrolled to wherever its
  // author left the viewport can open onto empty canvas with the drawing off
  // to one side.
  if (view && shows(view, scene.elements ?? [])) {
    return {
      ok: true,
      data: {
        elements: scene.elements,
        appState: {
          ...appState,
          zoom: { value: view.zoom as never },
          scrollX: view.scrollX,
          scrollY: view.scrollY,
        },
        files,
        scrollToContent: false,
      },
    };
  }
  return {
    ok: true,
    data: { elements: scene.elements, appState, files, scrollToContent: true },
  };
}
