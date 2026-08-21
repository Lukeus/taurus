import React from "react";
import ReactDOM from "react-dom/client";

/*
 * The design system names these three as Google Fonts. Tauri's CSP allows no
 * remote stylesheet, so they are installed as packages and bundled instead —
 * same faces, served from the app. Only the weights the UI actually sets: an
 * unused weight is a font file downloaded and parsed for nothing.
 */
import "@fontsource/space-grotesk/600.css";
import "@fontsource/manrope/400.css";
import "@fontsource/manrope/500.css";
import "@fontsource/manrope/600.css";
import "@fontsource/source-code-pro/400.css";
import "@fontsource/source-code-pro/500.css";

import App from "./App";
import { applyTheme, cachedTheme } from "./lib/theme";
import "./styles.css";

// Before the first render, and from the local copy rather than from settings:
// the real preference arrives over IPC after the window is already on screen,
// and a light-mode user should not watch the app open dark and correct itself
// every time. App reconciles this with what Rust says as soon as it knows.
applyTheme(cachedTheme());

/*
 * A drop that nothing claimed has to land on nothing.
 *
 * `dragDropEnabled` is off in `tauri.conf.json`, so a file dragged onto the
 * window arrives as an ordinary HTML5 drag — and the browser's default for an
 * unhandled one is to *navigate* to it. Dropping a screenshot on a model
 * without vision, or anywhere outside the composer at all, replaced the whole
 * app with `file:///…/screenshot.png`: transcript, rail and live session gone
 * until a reload. It is the one stray gesture in the app that destroys state.
 *
 * On `window` rather than in a component, so it cannot be unmounted, and
 * skipped whenever something nearer already called `preventDefault` — that is
 * the composer accepting an image, and this must not take its `dropEffect`
 * away and tell the user it will not be accepted.
 */
const inert = (e: DragEvent) => {
  if (e.defaultPrevented) return;
  e.preventDefault();
  if (e.dataTransfer) e.dataTransfer.dropEffect = "none";
};
window.addEventListener("dragover", inert);
window.addEventListener("drop", inert);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
