import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "@tailwindcss/vite";

import { excalidrawAssets } from "./scripts/excalidraw-assets.mjs";

// This file runs in Node, and the one thing it reads from Node is below. Said
// here rather than by adding Node's types to a project that is otherwise a page.
declare const process: { env: Record<string, string | undefined> };

// Tauri serves the dev server on a fixed port and needs the build output in
// dist/. `clearScreen: false` keeps Rust compiler errors visible.
//
// `excalidrawAssets` serves the sketch editor's fonts from this origin, since
// the CSP refuses the CDN Excalidraw would otherwise fetch them from.
//
// Source maps only in a debug build. Tauri embeds the whole of dist/ in the
// app, and the maps were 22 MB of a 31 MB bundle that nothing in it reads.
// `TAURI_ENV_DEBUG` is set by `tauri build --debug`; the dev server makes its
// own maps whatever this says.
export default defineConfig({
  plugins: [react(), tailwind(), excalidrawAssets()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "dist", target: "es2022", sourcemap: !!process.env.TAURI_ENV_DEBUG },
});
