import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "@tailwindcss/vite";

import { excalidrawAssets } from "./scripts/excalidraw-assets.mjs";

// Tauri serves the dev server on a fixed port and needs the build output in
// dist/. `clearScreen: false` keeps Rust compiler errors visible.
//
// `excalidrawAssets` serves the sketch editor's fonts from this origin, since
// the CSP refuses the CDN Excalidraw would otherwise fetch them from.
export default defineConfig({
  plugins: [react(), tailwind(), excalidrawAssets()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "dist", target: "es2022", sourcemap: true },
});
