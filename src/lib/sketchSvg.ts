import "./excalidrawEnv";

import { exportToSvg, getNonDeletedElements, restoreElements } from "@excalidraw/excalidraw";

/**
 * A sketch, drawn as a picture for a note to show.
 *
 * Loaded lazily, and only by an embed in a note that is being read — the same
 * chunk the editor lives in, so a window that shows one has paid for the other.
 *
 * The scene is restored before it is drawn. `exportToSvg` expects every element
 * whole, and a file written by hand or by an older Excalidraw may leave fields
 * out; restoring is how Excalidraw itself reads one, and doing it here means an
 * embed draws exactly what the editor would open.
 *
 * At one scene unit to one CSS pixel — see `exportScale` below.
 *
 * Drawn on a transparent ground so it sits on the note's own panel, and inverted
 * for the dark theme the way Excalidraw's own dark export does — so a black line
 * drawn on a light canvas is a light line on a dark note, rather than a black
 * one nobody can see.
 */
export async function draw(text: string, dark: boolean): Promise<SVGSVGElement> {
  const scene = JSON.parse(text) as {
    elements?: unknown;
    appState?: Record<string, unknown>;
    files?: Record<string, unknown>;
  };
  if (!Array.isArray(scene.elements)) {
    throw new Error("It is not an Excalidraw drawing — there is no list of elements in it.");
  }
  type Opts = Parameters<typeof exportToSvg>[0];
  const elements = restoreElements(scene.elements as Parameters<typeof restoreElements>[0], null);
  return exportToSvg({
    elements: getNonDeletedElements(elements),
    appState: {
      ...(scene.appState ?? {}),
      exportBackground: false,
      exportWithDarkMode: dark,
      // One scene unit to one CSS pixel. Left out, Excalidraw fills it from the
      // device's pixel ratio — which on any Retina screen drew every embed at
      // twice its size, clipped at the edge of the note. Found by photographing
      // one; the browser already renders at the device's density on its own.
      exportScale: 1,
    } as Opts["appState"],
    files: (scene.files ?? null) as Opts["files"],
    exportPadding: 12,
  });
}
