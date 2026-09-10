import "../lib/excalidrawEnv";

import { useEffect, useMemo, useRef } from "react";
import { Excalidraw, MainMenu, hashElementsVersion, serializeAsJSON } from "@excalidraw/excalidraw";
import type { AppState, BinaryFiles } from "@excalidraw/excalidraw/types";
import { openUrl } from "@tauri-apps/plugin-opener";

import { parse } from "../lib/sketch";
import { useWindowTheme } from "../lib/windowTheme";

import "../sketch.css";

/**
 * A sketch, open in Excalidraw.
 *
 * The one surface in the app that is a dependency rather than something drawn
 * here. The notes pane draws Mermaid with the app's own engines because a
 * diagram *language* is a grammar and a layout, and both were already half
 * written. A freehand canvas is neither — it is hit-testing, selection, undo,
 * shape tools, text on a path and a hundred interactions a person expects to
 * behave exactly the way the tools they know do. Writing that here would take
 * longer than the rest of the notebook and arrive worse.
 *
 * Loaded lazily, so a window that never opens a sketch never pays for one. The
 * default export is what `React.lazy` wants, and it is why this file has one.
 *
 * # What the component does not let Excalidraw do
 *
 * Four things Excalidraw does by default are wrong inside this app, and each is
 * turned off for its own reason.
 *
 * - **Open, save to disk, export.** The file *is* the sketch, saved by the same
 *   compare-and-swap every note uses. A second way to save it — one that knows
 *   nothing about the fingerprint — would be a second writer the rule cannot
 *   see.
 * - **Its theme switch.** The window already has one. A canvas that disagreed
 *   with it would be a second answer to the same question.
 * - **Links.** A click on an element's link, and every `target="_blank"` in its
 *   menus and dialogs, would navigate the webview itself — replacing the whole
 *   app, transcript and all, with a web page. `Markdown` refuses the same thing
 *   for the same reason. Here there are two routes and both go to the system
 *   browser: `onLinkOpen` for element links, and a capture-phase click guard
 *   for every anchor Excalidraw renders.
 * - **Its AI features.** They call a service the app has not been configured
 *   for and the CSP would refuse anyway, so offering them would be offering
 *   something that cannot work.
 */
export default function SketchEditor({
  text,
  generation,
  onChange,
}: {
  /** The `.excalidraw` file as last read. Only read when `generation` moves. */
  text: string;
  /**
   * Moves when the file has to be read into the canvas again — a different
   * sketch, or the other version taken after a conflict.
   *
   * Not the fingerprint. A save changes the fingerprint and must not reload the
   * canvas: Excalidraw holds the undo history, the selection and the tool in
   * hand, and throwing those away every time the drawing was written to disk
   * would make the editor unusable in exactly the moment somebody is using it.
   */
  generation: number;
  /** Called with the new file text, once the drawing has stopped changing. */
  onChange: (text: string) => void;
}) {
  const theme = useWindowTheme();
  // Keyed on `generation` alone, deliberately — see the prop.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const parsed = useMemo(() => parse(text), [generation]);

  /*
   * What the drawing looked like at the last call that mattered.
   *
   * Excalidraw calls `onChange` on every pointer move, selection, pan and zoom,
   * and serialising a scene on each of those would be a JSON encode of every
   * element sixty times a second. So each call is reduced to a key — the scene
   * version, the canvas colour, how many images it carries — and only a call
   * whose key moved is worth serialising at all. Panning changes none of them.
   */
  const lastKey = useRef<string | null>(null);
  const timer = useRef<number | null>(null);
  const emit = useRef(onChange);
  emit.current = onChange;

  // A reload starts the comparison over.
  useEffect(() => {
    lastKey.current = null;
  }, [generation]);

  useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    },
    [],
  );

  /*
   * Every anchor Excalidraw renders goes to the system browser.
   *
   * On the document rather than on the editor's own box, because Excalidraw
   * portals some of its dialogs out of it, and capture-phase so this runs before
   * the anchor's default does. Only anchors inside Excalidraw's own containers
   * are touched — every other link in the app already has its own handling and
   * this is not the place to second-guess it.
   */
  useEffect(() => {
    const guard = (event: MouseEvent) => {
      const anchor = (event.target as Element | null)?.closest?.("a[href]");
      if (!(anchor instanceof HTMLAnchorElement)) return;
      if (!anchor.closest(".excalidraw, .excalidraw-modal-container")) return;
      event.preventDefault();
      event.stopPropagation();
      openExternally(anchor.getAttribute("href"));
    };
    document.addEventListener("click", guard, true);
    return () => document.removeEventListener("click", guard, true);
  }, []);

  if (!parsed.ok) {
    // No canvas at all. An empty one would save itself a moment later, and a
    // file this could not read would be replaced by a blank drawing — the one
    // outcome worse than showing nothing.
    return (
      <div className="sketch-problem" role="alert">
        <b>This sketch could not be read.</b>
        <span>{parsed.why}</span>
        <span>
          The file is untouched. Fix it in a text editor, or delete it from the
          list.
        </span>
      </div>
    );
  }

  return (
    <div className="sketch-edit">
      <Excalidraw
        key={generation}
        initialData={parsed.data}
        theme={theme}
        langCode="en"
        aiEnabled={false}
        UIOptions={{
          canvasActions: {
            loadScene: false,
            saveToActiveFile: false,
            export: false,
            saveAsImage: false,
            toggleTheme: null,
            clearCanvas: true,
            changeViewBackgroundColor: true,
          },
        }}
        onLinkOpen={(element, event) => {
          event.preventDefault();
          openExternally(element.link);
        }}
        onChange={(elements, appState, files) => {
          const key = keyOf(elements, appState, files);
          // The first call after a load is Excalidraw reporting what it was
          // given. Saving that would rewrite a file nobody touched into
          // Excalidraw's own formatting the moment it was opened.
          if (lastKey.current === null) {
            lastKey.current = key;
            return;
          }
          if (key === lastKey.current) return;
          lastKey.current = key;
          if (timer.current !== null) window.clearTimeout(timer.current);
          // Short, because the save loop behind this has its own debounce. This
          // one only keeps a stroke from being serialised once per point.
          timer.current = window.setTimeout(() => {
            timer.current = null;
            emit.current(serializeAsJSON(elements, appState, files, "local"));
          }, SERIALIZE_AFTER_MS);
        }}
      >
        {/*
          The menu, rebuilt without the items the component turns off above
          and without the links to Excalidraw's own accounts — which the default
          menu carries, and which would each have navigated the app away.
        */}
        <MainMenu>
          <MainMenu.DefaultItems.SearchMenu />
          <MainMenu.DefaultItems.Help />
          <MainMenu.DefaultItems.ClearCanvas />
          <MainMenu.Separator />
          <MainMenu.DefaultItems.ChangeCanvasBackground />
        </MainMenu>
      </Excalidraw>
    </div>
  );
}

/** How long a changed drawing waits before it is serialised. */
const SERIALIZE_AFTER_MS = 200;

/** What a change has to move to be worth serialising. */
function keyOf(
  elements: Parameters<typeof hashElementsVersion>[0],
  appState: AppState,
  files: BinaryFiles,
): string {
  return `${hashElementsVersion(elements)}:${appState.viewBackgroundColor}:${Object.keys(files).length}`;
}

/** A link, to the system browser — and anything else, nowhere. */
function openExternally(href: string | null | undefined) {
  if (!href || !/^(https?:|mailto:)/i.test(href)) return;
  void openUrl(href).catch(() => {});
}
