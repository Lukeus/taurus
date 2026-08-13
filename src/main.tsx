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

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
