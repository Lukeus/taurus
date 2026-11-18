import { useEffect, useRef, useState } from "react";

import { previewSnapshot } from "../lib/api";
import { frames, isLoopbackHost, offerLook, wait } from "../lib/preview";
import { onTabKeys } from "../lib/tabs";
import { CameraIcon, NarrowIcon, WidenIcon } from "./icons";

/** Widths a page is checked at. `null` is the column's own width. */
const VIEWPORTS = [
  { id: "fill", label: "Fill", width: null },
  { id: "phone", label: "Phone", width: 390 },
  { id: "tablet", label: "Tablet", width: 820 },
] as const;
type Viewport = (typeof VIEWPORTS)[number]["id"];

type Logged = { level: string; text: string };

/**
 * A dev server's page as a canvas tab.
 *
 * An `<iframe>` in the layout, so it is a tab the way a file is: it resizes
 * with the column, hides behind the tab in front, and goes under ⌘K like
 * anything else on the page. What the iframe's own origin keeps from us comes
 * in two other ways: the frame script `preview.rs` injects posts the page's
 * console up here, and a look is a native picture of this tab's rectangle.
 */
export function PreviewTab({
  path,
  url: initialUrl,
  showing,
  tabbed,
  wide,
  onClose,
}: {
  /** The tab's path, which is what a look asks for it by. */
  path: string;
  url: string;
  /** Front tab of a column that's showing. */
  showing: boolean;
  tabbed: boolean;
  wide?: { on: boolean; toggle: () => void };
  onClose: () => void;
}) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [url, setUrl] = useState(initialUrl);
  const [address, setAddress] = useState(initialUrl);
  const [viewport, setViewport] = useState<Viewport>("fill");
  const [reloads, setReloads] = useState(0);
  const [logged, setLogged] = useState<Logged[]>([]);
  const [ipc, setIpc] = useState("waiting");
  const [title, setTitle] = useState<string | null>(null);
  const [look, setLook] = useState<string | null>(null);
  const [lookMs, setLookMs] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showLog, setShowLog] = useState(false);

  /*
   * What a look reads at the moment it runs, not what the render that
   * registered the looker saw. `unseen` is what the page logged since the
   * model's last look, which is what each look reports and then clears.
   */
  const showingNow = useRef(showing);
  showingNow.current = showing;
  const viewportNow = useRef(viewport);
  viewportNow.current = viewport;
  const pageUrl = useRef(initialUrl);
  const titleNow = useRef<string | null>(null);
  titleNow.current = title;
  const unseen = useRef<Logged[]>([]);
  const outlines = useRef(new Map<string, (text: string) => void>());

  /*
   * What the page posts. Only from this tab's iframe: another preview tab's
   * page posts to the same window, and a message is the page talking, so
   * where it came from is the one thing checked before anything is believed.
   */
  useEffect(() => {
    const heard = (e: MessageEvent) => {
      if (e.source !== frame.current?.contentWindow) return;
      const data = e.data as { taurusPreview?: number; kind?: string; [k: string]: unknown };
      if (!data || data.taurusPreview !== 1) return;
      if (data.kind === "log") {
        const entry = { level: String(data.level), text: String(data.text).slice(0, 2000) };
        setLogged((all) => [...all.slice(-199), entry]);
        unseen.current = [...unseen.current.slice(-199), entry];
      } else if (data.kind === "ipc") {
        setIpc(String(data.state));
      } else if (data.kind === "ready") {
        setTitle(String(data.title || data.url));
        // Where the page is now, which a link inside it may have changed.
        if (typeof data.url === "string") pageUrl.current = data.url;
      } else if (data.kind === "outline" && typeof data.id === "string") {
        outlines.current.get(data.id)?.(String(data.text));
        outlines.current.delete(data.id);
      }
    };
    window.addEventListener("message", heard);
    return () => window.removeEventListener("message", heard);
  }, []);

  /*
   * The model's look. Settles the width it asked for, waits for the tab to be
   * the one on screen (`App` brings it there) and for the page to draw at
   * that width, then says where the page is for Rust to photograph, and what
   * it logged and, for an outline, says.
   */
  useEffect(
    () =>
      offerLook(path, async (request) => {
        const changed = !!request.viewport && request.viewport !== viewportNow.current;
        if (request.viewport) setViewport(request.viewport);
        for (let waited = 0; !showingNow.current; waited += 50) {
          if (waited > 2000) throw new Error("The preview tab couldn't be brought to the front.");
          await wait(50);
        }
        await frames(2);
        // A new width reflows the page, and a page that animates its layout
        // takes a moment more than a frame to land.
        if (changed) await wait(350);
        const el = frame.current;
        if (!el) throw new Error("The preview has no page in it.");

        let outline: string | null = null;
        if (request.mode === "outline") {
          const id = `${Date.now()}-${Math.random()}`;
          outline = await Promise.race([
            new Promise<string>((done) => {
              outlines.current.set(id, done);
              el.contentWindow?.postMessage({ taurusPreview: 1, kind: "outline-request", id }, "*");
            }),
            wait(3000).then(
              () => "[The page didn't answer. It may still be loading, or not be a page this preview can read.]",
            ),
          ]);
          outlines.current.delete(id);
        }

        const r = el.getBoundingClientRect();
        const chosen = VIEWPORTS.find((v) => v.id === (request.viewport ?? viewportNow.current));
        const console = unseen.current;
        unseen.current = [];
        return {
          url: pageUrl.current,
          title: titleNow.current ?? "",
          viewport: `${chosen?.id ?? "fill"} (${chosen?.width ?? Math.round(r.width)}px)`,
          area: { x: r.left, y: r.top, width: r.width, height: r.height, viewport: window.innerWidth },
          outline,
          console,
        };
      }),
    [path],
  );

  const go = (to: string) => {
    setError(null);
    try {
      const parsed = new URL(to);
      if (!isLoopbackHost(parsed.hostname)) {
        setError(`${parsed.host} isn't on this machine. The preview opens localhost addresses only.`);
        return;
      }
    } catch {
      setError(`${to} is not an address.`);
      return;
    }
    setLogged([]);
    unseen.current = [];
    pageUrl.current = to;
    setUrl(to);
    setReloads((n) => n + 1);
  };

  const takeLook = async () => {
    const el = frame.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const started = performance.now();
    try {
      setLook(
        await previewSnapshot({
          x: r.left,
          y: r.top,
          width: r.width,
          height: r.height,
          viewport: window.innerWidth,
        }),
      );
      setLookMs(Math.round(performance.now() - started));
    } catch (e) {
      setError(String(e));
    }
  };

  const errors = logged.filter((l) => l.level === "error").length;
  const width = VIEWPORTS.find((v) => v.id === viewport)?.width ?? null;

  return (
    <div className="canvas-panel" style={showing ? undefined : { visibility: "hidden" }}>
      <section className="canvas preview-tab" aria-label={`Preview: ${url}`}>
        <header className={`canvas-head${tabbed ? " tabbed" : ""}`}>
          <form
            className="preview-address-form"
            onSubmit={(e) => {
              e.preventDefault();
              go(address);
            }}
          >
            <input
              className="preview-address"
              value={address}
              onChange={(e) => setAddress(e.target.value)}
              aria-label="Address"
              spellCheck={false}
            />
          </form>
          <button className="pane-toggle" onClick={() => setReloads((n) => n + 1)} aria-label="Reload" data-tip="Reload">
            ↻
          </button>
          {/* The same choice twice, and CSS shows one: a segmented control
              where there's room, a select where there isn't, and neither in
              a column already narrower than a phone. See `.preview-tab`. */}
          <div
            className="canvas-modes preview-viewport-seg"
            role="tablist"
            aria-label="Viewport"
            onKeyDown={onTabKeys}
          >
            {VIEWPORTS.map((v) => (
              <button
                key={v.id}
                role="tab"
                aria-selected={viewport === v.id}
                tabIndex={viewport === v.id ? 0 : -1}
                className={`seg${viewport === v.id ? " on" : ""}`}
                onClick={() => setViewport(v.id)}
              >
                {v.label}
              </button>
            ))}
          </div>
          <select
            className="preview-viewport-select"
            value={viewport}
            onChange={(e) => setViewport(e.target.value as Viewport)}
            aria-label="Viewport"
          >
            {VIEWPORTS.map((v) => (
              <option key={v.id} value={v.id}>
                {v.label}
              </option>
            ))}
          </select>
          <button
            className="pane-toggle"
            onClick={() => void takeLook()}
            aria-label="Take a picture"
            data-tip="Take a picture of the page"
          >
            <CameraIcon size={14} />
          </button>
          {wide && (
            <button
              className="pane-toggle"
              onClick={wide.toggle}
              aria-pressed={wide.on}
              aria-label="Widen the preview"
              data-tip={wide.on ? "Back to the width it was" : "Widen the preview"}
            >
              {wide.on ? <NarrowIcon size={14} /> : <WidenIcon size={14} />}
            </button>
          )}
          {!tabbed && (
            <button className="canvas-close" onClick={onClose} aria-label="Close the preview">
              ✕
            </button>
          )}
        </header>

        {error && <p className="preview-note error">{error}</p>}

        <div className="preview-stage">
          <iframe
            key={reloads}
            ref={frame}
            src={url}
            title={title ?? url}
            className="preview-frame"
            /* Everything a dev server's page needs except reaching out of
               its frame: without `allow-top-navigation` a `target="_top"`
               link or `top.location = …` can't put the page where the app
               was. `allow-same-origin` is the page's own origin, which is
               not the app's, so it gains nothing over us by it. */
            sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-downloads"
            style={width ? { width, flex: "none" } : undefined}
          />
        </div>

        <footer className="preview-status">
          <button className="preview-status-log" onClick={() => setShowLog((on) => !on)} aria-expanded={showLog}>
            {errors > 0 ? `${errors} error${errors === 1 ? "" : "s"}` : "no errors"} · {logged.length} logged
          </button>
          <span title="Whether the page can call into the app. It shouldn't.">ipc: {ipc}</span>
          {lookMs !== null && <span>look {lookMs} ms</span>}
        </footer>
        {showLog && (
          <ol className="preview-log">
            {logged.map((l, i) => (
              <li key={i} className={l.level}>
                {l.text}
              </li>
            ))}
          </ol>
        )}
        {look && (
          <button className="preview-look" onClick={() => setLook(null)} aria-label="Dismiss the picture">
            <img src={look} alt="What the preview looked like" />
          </button>
        )}
      </section>
    </div>
  );
}
