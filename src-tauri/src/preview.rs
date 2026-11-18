//! A dev server's page as a canvas tab.
//!
//! The page is an `<iframe>` in the app's own page, so it is a tab like a file
//! is: it sits in the layout, scrolls, resizes and goes under ⌘K without
//! anything here knowing. Two things a cross-origin iframe can't do on its own
//! are done from here instead:
//!
//! - **Say what it logged.** [`FRAME_SCRIPT`] is injected into every frame by
//!   this plugin, runs only in a loopback iframe, and posts the page's console
//!   errors up to the app with `postMessage`.
//! - **Be photographed.** [`preview_snapshot`] photographs the app's own
//!   webview cropped to the tab, which includes the iframe whatever its origin.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use serde::Deserialize;
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::oneshot;

use taurus_tools::builtin::preview::{Logged, Look, LookMode, LookRequest, PreviewEye};

use crate::commands::CmdResult;

/// Runs in every frame of the app's webview, and does nothing except in an
/// iframe served from this machine.
///
/// What it posts is the page talking: the app reads it as untrusted, checks
/// it came from the iframe it made, and would label it that way for a model.
///
/// `ipc` records whether Tauri's IPC is reachable from inside the iframe, so
/// the claim that a dev server's page gets no command is checked, not assumed.
const FRAME_SCRIPT: &str = r##"(() => {
  if (window === window.top) return;
  if (!/^(localhost|127\.0\.0\.1)$/.test(location.hostname)) return;
  if (window.__taurusPreview) return;
  window.__taurusPreview = true;

  const post = (body) => {
    try { window.parent.postMessage({ taurusPreview: 1, ...body }, "*"); } catch {}
  };
  const text = (a) => {
    try {
      if (typeof a === "string") return a;
      if (a instanceof Error) return a.stack || String(a);
      return JSON.stringify(a);
    } catch { return String(a); }
  };
  const log = (level, args) =>
    post({ kind: "log", level, text: args.map(text).join(" ").slice(0, 2000) });

  for (const level of ["error", "warn"]) {
    const original = console[level];
    console[level] = function (...args) { log(level, args); return original.apply(this, args); };
  }
  addEventListener("error", (e) => log("error", [e.message + (e.filename ? ` (${e.filename}:${e.lineno})` : "")]));
  addEventListener("unhandledrejection", (e) => log("error", ["Unhandled rejection:", e.reason]));
  addEventListener("DOMContentLoaded", () => post({ kind: "ready", url: location.href, title: document.title }));

  // Whether the app's IPC is in this frame at all. Only looked for, never
  // called: calling a command to see if it answers would be the page running
  // that command, which is what this is here to notice.
  const internals = window.__TAURI_INTERNALS__;
  post({
    kind: "ipc",
    state: internals && typeof internals.invoke === "function" ? "PRESENT" : "absent",
  });

  // The page as text, for a look in outline mode: headings, the text that
  // belongs to each element, and the things a person would click or type in.
  // Built here because only here can the page be read; the app is another
  // origin.
  const outline = () => {
    const MAX = 16000;
    const out = [];
    let size = 0;
    const say = (depth, line) => {
      if (size > MAX) return;
      line = String(line).replace(/\s+/g, " ").trim();
      if (!line) return;
      const text = "  ".repeat(depth) + line;
      out.push(text);
      size += text.length + 1;
    };
    const shown = (el) => {
      const style = getComputedStyle(el);
      return style.display !== "none" && style.visibility !== "hidden" && el.getAttribute("aria-hidden") !== "true";
    };
    const label = (el) => el.getAttribute("aria-label") || el.getAttribute("title") || "";
    const SKIP = ["script", "style", "noscript", "template", "svg", "head"];
    const GROUPS = ["nav", "main", "header", "footer", "aside", "form", "dialog", "section", "ul", "ol", "table"];
    const walk = (el, depth) => {
      for (const child of el.children) {
        const tag = child.tagName.toLowerCase();
        if (SKIP.includes(tag) || !shown(child)) continue;
        if (/^h[1-6]$/.test(tag)) { say(depth, "#".repeat(Number(tag[1])) + " " + child.innerText); continue; }
        if (tag === "button" || child.getAttribute("role") === "button") {
          say(depth, `[button] ${child.innerText || label(child)}${child.disabled ? " (disabled)" : ""}`);
          continue;
        }
        if (tag === "a") { say(depth, `[link] ${child.innerText || label(child)} -> ${child.getAttribute("href") || ""}`); continue; }
        if (tag === "input" || tag === "textarea" || tag === "select") {
          const kind = tag === "input" ? `input ${child.type || "text"}` : tag;
          const name = (child.labels && child.labels[0] && child.labels[0].innerText) || label(child) || child.placeholder || child.name || "";
          const value = child.value ? ` = "${String(child.value).slice(0, 80)}"` : "";
          say(depth, `[${kind}] ${name}${value}`);
          continue;
        }
        if (tag === "img") { say(depth, `[image] ${child.alt || "(no alt text)"}`); continue; }
        // Text written straight into the element, not inside a child of it.
        const own = [...child.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join(" ").trim();
        if (GROUPS.includes(tag)) {
          say(depth, `<${tag}>${label(child) ? " " + label(child) : ""}`);
          if (own) say(depth + 1, own.slice(0, 300));
          walk(child, depth + 1);
          continue;
        }
        if (own) say(depth, (tag === "li" ? "- " : "") + own.slice(0, 300));
        walk(child, depth);
      }
    };
    if (document.body) walk(document.body, 0);
    return out.join("\n") + (size > MAX ? "\n[... the rest of the page left out]" : "");
  };
  addEventListener("message", (e) => {
    if (e.source !== window.parent) return;
    const data = e.data;
    if (!data || data.taurusPreview !== 1 || data.kind !== "outline-request") return;
    let text;
    try { text = outline(); } catch (err) { text = "[the outline couldn't be built: " + String(err) + "]"; }
    post({ kind: "outline", id: data.id, text });
  });
})();"##;

/// The plugin that puts [`FRAME_SCRIPT`] in every frame.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("preview")
        .js_init_script_on_all_frames(FRAME_SCRIPT)
        .build()
}

/// A box in the app's page, in CSS pixels, with the page's `innerWidth`.
///
/// The interface size setting zooms the app's webview, so a CSS pixel is not
/// one of the view's points; the view's width over `viewport` is the zoom.
#[derive(Clone, Copy, Deserialize)]
pub struct Area {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    viewport: f64,
}

/// How wide a picture headed for a model is allowed to be, in pixels. Past
/// this the providers shrink it anyway, after charging for what was sent.
const MODEL_PIXELS_WIDE: f64 = 1280.0;

/// A PNG of `area` of the app's page. `fit` caps its width in pixels.
async fn picture(app: &AppHandle, area: Area, fit: Option<f64>) -> CmdResult<Vec<u8>> {
    let window = app.get_webview_window("main").ok_or("no main window")?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let width = f64::from(window.inner_size().map_err(|e| e.to_string())?.width) / scale;
    let main: &tauri::Webview = window.as_ref();
    let zoom = if area.viewport > 0.0 {
        width / area.viewport
    } else {
        1.0
    };
    let points_wide = area.width * zoom;
    let fit_width = fit
        .map(|pixels| pixels / scale)
        .filter(|&points| points < points_wide);
    let png = taurus_webshot::snapshot(
        main,
        Some(taurus_webshot::Area {
            x: area.x * zoom,
            y: area.y * zoom,
            width: points_wide,
            height: area.height * zoom,
            fit_width,
        }),
    )
    .await?;
    Ok(png)
}

fn base64(png: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(png)
}

/// The tab as it's drawn, as a PNG `data:` URL, at full size: this one is for
/// the person.
#[tauri::command]
pub async fn preview_snapshot(app: AppHandle, area: Area) -> CmdResult<String> {
    let png = picture(&app, area, None).await?;
    Ok(format!("data:image/png;base64,{}", base64(&png)))
}

/// What the window says it showed, for a look. See [`UiPreviewEye`].
#[derive(Deserialize)]
pub struct LookAnswer {
    url: String,
    title: String,
    viewport: String,
    /// Where the page is, for the picture taken here.
    area: Area,
    outline: Option<String>,
    console: Vec<Logged>,
}

type Waiting = Arc<DashMap<String, oneshot::Sender<Result<LookAnswer, String>>>>;

/// The looks the window hasn't answered yet, by call id.
#[derive(Clone, Default)]
pub struct LookWaits(Waiting);

/// The model's eye on the preview: asks the window, waits for it, and takes
/// the picture itself from where the window says the page is.
///
/// The window settles the width and gathers what the page logged and, for an
/// outline, what it says; only Rust can photograph it, and only after the
/// window has finished moving it.
pub struct UiPreviewEye {
    app: AppHandle,
    waits: LookWaits,
}

impl UiPreviewEye {
    pub fn new(app: AppHandle, waits: LookWaits) -> Self {
        Self { app, waits }
    }
}

/// How long the window gets. A look is a width change, a frame or two and a
/// message to the page; anything near this long is a window that isn't
/// coming back.
const LOOK_WAIT: Duration = Duration::from_secs(15);

#[async_trait]
impl PreviewEye for UiPreviewEye {
    async fn look(&self, request: LookRequest) -> Result<Look, String> {
        let (tx, rx) = oneshot::channel();
        let id = request.id.clone();
        self.waits.0.insert(id.clone(), tx);
        // Taken out however this ends, so a canceled look leaves nothing.
        struct Unpark<'a>(&'a Waiting, String);
        impl Drop for Unpark<'_> {
            fn drop(&mut self) {
                self.0.remove(&self.1);
            }
        }
        let _unpark = Unpark(&self.waits.0, id);

        self.app
            .emit(EVENT_PREVIEW_LOOK, &request)
            .map_err(|e| format!("the window couldn't be asked: {e}"))?;
        let answer = tokio::time::timeout(LOOK_WAIT, rx)
            .await
            .map_err(|_| "The window didn't answer. Is the app's window open?".to_string())?
            .map_err(|_| "The window went away before answering.".to_string())??;

        let picture = match request.mode {
            LookMode::Picture => Some(base64(
                &picture(&self.app, answer.area, Some(MODEL_PIXELS_WIDE)).await?,
            )),
            LookMode::Outline => None,
        };
        Ok(Look {
            url: answer.url,
            title: answer.title,
            viewport: answer.viewport,
            picture,
            outline: answer.outline,
            console: answer.console,
        })
    }
}

/// Asks the window for a look. Carries a [`LookRequest`].
pub const EVENT_PREVIEW_LOOK: &str = "preview-look";

/// The window's answer to a look: what it showed, or why it couldn't.
#[tauri::command]
pub async fn preview_look_answer(
    waits: State<'_, LookWaits>,
    id: String,
    answer: Option<LookAnswer>,
    error: Option<String>,
) -> CmdResult<()> {
    if let Some((_, tx)) = waits.0.remove(&id) {
        let _ = tx.send(match answer {
            Some(answer) => Ok(answer),
            None => Err(error.unwrap_or_else(|| "The window couldn't look.".into())),
        });
    }
    Ok(())
}
