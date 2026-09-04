import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "@tailwindcss/vite";

/**
 * Serves the screenshot harness rather than the app.
 *
 * A separate config only because the root moves: the app's entry is the
 * repository's `index.html` and this one's is `scripts/screenshots/index.html`.
 * Everything under `src` is imported unchanged from there, which is the point —
 * the images have to come from the interface that ships, not a copy of it.
 */
export default defineConfig({
  // Tailwind too, and for the same reason the root moves: the images have to
  // come from the interface that ships, and half of that interface is now
  // compiled from the class names in it.
  plugins: [react(), tailwind()],
  root: "scripts/screenshots",
  clearScreen: false,
});
