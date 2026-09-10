import "./excalidrawEnv";

import {
  exportToCanvas,
  exportToSvg,
  getNonDeletedElements,
  restoreElements,
} from "@excalidraw/excalidraw";

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
 *
 * # Why the picture carries no fonts of its own
 *
 * Left to itself, the exporter fetches each face a sketch uses, cuts it down to
 * the glyphs on it, and embeds the result in the SVG's stylesheet as a `data:`
 * URL. Under the packaged app's CSP none of that survives: the cutting is
 * WebAssembly, which `default-src 'self'` will not compile; the fallback it
 * then takes is a CDN URL, which the CSP refuses as a font; and a `data:` face
 * would be refused too, since `font-src` falls back to `'self'`. Every embed's
 * text drew in a serif in the app, while the screenshot harness, which has no
 * CSP, showed it handwritten. Found by serving a built harness under the app's
 * policy and photographing an embed.
 *
 * So the fonts are not inlined — which is how Excalidraw draws its own library
 * previews — and the text uses the faces registered with the document. Those
 * exist only once Excalidraw has loaded a scene's fonts, which the editor does
 * and a note that has only ever been read does not. `exportToCanvas` does it
 * first thing, for exactly the text it is handed and from this origin, and a
 * one-pixel canvas is the cheapest way to ask it to.
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
  const elements = getNonDeletedElements(
    restoreElements(scene.elements as Parameters<typeof restoreElements>[0], null),
  );
  const words = elements.filter((element) => element.type === "text");
  if (words.length > 0) {
    await exportToCanvas({
      elements: words,
      files: null,
      getDimensions: () => ({ width: 1, height: 1, scale: 1 }),
    });
  }
  return exportToSvg({
    elements,
    skipInliningFonts: true,
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
