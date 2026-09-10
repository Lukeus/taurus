/**
 * Excalidraw's fonts, served from the app's own origin — and its unused
 * translations, left out of the build.
 *
 * Excalidraw fetches its handwritten faces at runtime, from a CDN unless told
 * otherwise. The app's CSP is `default-src 'self'`, so a CDN font is a font that
 * silently never arrives — every sketch would draw in the fallback face, and
 * nothing would say why. `window.EXCALIDRAW_ASSET_PATH` points it at this
 * origin instead, and this plugin is what answers there: the dev server serves
 * the files straight out of `node_modules`, and a build copies them into
 * `dist/excalidraw/fonts/`.
 *
 * Copied at build rather than committed, for the reason `scripts/conpty.mjs`
 * fetches ConPTY rather than checking it in: half a megabyte of binaries the
 * repository neither builds nor owns, pinned by the lockfile instead.
 *
 * # Why Xiaolai is left out
 *
 * It is the CJK face, and it is 12 MB — more than every other font, script and
 * stylesheet in the app put together. The others are about half a megabyte
 * between them. Excalidraw asks for Xiaolai only for glyphs no other face has,
 * by unicode range, so leaving it out costs exactly this: Chinese, Japanese and
 * Korean text in a sketch draws in the system's own face rather than the
 * handwritten one. It still draws, still exports, and still saves. See
 * `docs/known-gaps.md`.
 */

import { createReadStream, readdirSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

/** Where the fonts are served from, relative to the app's origin. */
export const ASSET_BASE = "/excalidraw/";

/** Every family Excalidraw ships, minus the one this deliberately does not. */
const FAMILIES = [
  "Assistant",
  "Cascadia",
  "ComicShanns",
  "Excalifont",
  "Liberation",
  "Lilita",
  "Nunito",
  "Virgil",
];

const TYPES = { ".woff2": "font/woff2", ".woff": "font/woff", ".ttf": "font/ttf" };

/**
 * Excalidraw's translations, every one but English.
 *
 * `SketchEditor` sets `langCode="en"`, because the app around it is in English
 * and a canvas speaking another language inside it would be a locale nobody
 * chose. Excalidraw still imports each translation lazily by name, so without
 * this every one of them is emitted into the build to sit there unread — about
 * a megabyte, for fifty-two languages. Each is swapped for an empty module, and the
 * one that is ever loaded is the one that is kept.
 *
 * A build-time swap rather than a fork or a patch: the package is untouched, and
 * the day `langCode` follows a setting is the day this line comes out.
 */
const UNUSED_LOCALE =
  /[\\/]@excalidraw[\\/]excalidraw[\\/]dist[\\/](?:prod|dev)[\\/]locales[\\/](?!en-)[^\\/]+\.js$/;

/**
 * The package's `fonts/` directory.
 *
 * Found by resolving the package entry and walking to its sibling, because the
 * export map names only the entry, the stylesheet and the types — a deep import
 * of `dist/prod/fonts` is refused by design.
 */
function fontsDir() {
  const root = fileURLToPath(new URL("../", import.meta.url));
  const require = createRequire(join(root, "package.json"));
  return join(dirname(require.resolve("@excalidraw/excalidraw")), "fonts");
}

/** Every font file under the shipped families, as forward-slash paths. */
function shipped(dir) {
  const out = [];
  const walk = (at) => {
    for (const name of readdirSync(at)) {
      const path = join(at, name);
      if (statSync(path).isDirectory()) walk(path);
      else out.push(relative(dir, path).split(sep).join("/"));
    }
  };
  for (const family of FAMILIES) walk(join(dir, family));
  return out;
}

export function excalidrawAssets() {
  const dir = fontsDir();
  const files = new Set(shipped(dir));

  return {
    name: "taurus:excalidraw-assets",

    load(id) {
      if (UNUSED_LOCALE.test(id)) return "export default {};";
      return null;
    },

    configureServer(server) {
      server.middlewares.use(`${ASSET_BASE}fonts/`, (req, res) => {
        const rel = decodeURIComponent((req.url ?? "").split("?")[0]).replace(/^\/+/, "");
        // An exact match against the list rather than a path join checked
        // afterwards: nothing outside it is reachable, `..` included. And a 404
        // rather than falling through, which would hand a font request the
        // app's `index.html` with a 200 — a font that fails to parse instead of
        // one that is plainly missing, which is what Xiaolai is meant to be.
        if (!files.has(rel)) {
          res.statusCode = 404;
          res.end();
          return;
        }
        const ext = rel.slice(rel.lastIndexOf("."));
        res.setHeader("Content-Type", TYPES[ext] ?? "application/octet-stream");
        createReadStream(join(dir, rel)).pipe(res);
      });
    },

    generateBundle() {
      for (const rel of files) {
        this.emitFile({
          type: "asset",
          fileName: `${ASSET_BASE.slice(1)}fonts/${rel}`,
          source: readFileSync(join(dir, rel)),
        });
      }
    },
  };
}
