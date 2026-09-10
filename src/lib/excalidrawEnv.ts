/**
 * Where Excalidraw fetches its fonts from, set before Excalidraw is evaluated.
 *
 * Its own module so that importing it first is enough: ES modules evaluate in
 * the order they are imported, and the sketch editor imports this on the line
 * above `@excalidraw/excalidraw`. Excalidraw reads the global when it loads a
 * face, which is later still — but being early by construction is cheaper
 * than being early by an argument about when a font is first requested.
 *
 * An absolute URL rather than `/excalidraw/`. Excalidraw builds each font's
 * address with `new URL(file, base)`, and a base that is itself relative only
 * works when something upstream happens to resolve it first. Resolved here
 * against the page, it is right in the dev server (`http://localhost:1420`),
 * in the screenshot harness, and in the packaged app, whose origin is
 * `tauri://localhost` on one platform and `http://tauri.localhost` on another.
 *
 * `scripts/excalidraw-assets.mjs` is what answers at that address.
 */

declare global {
  interface Window {
    EXCALIDRAW_ASSET_PATH?: string | string[];
  }
}

window.EXCALIDRAW_ASSET_PATH = new URL("/excalidraw/", window.location.href).href;

export {};
