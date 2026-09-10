import { createContext, useContext, useEffect, useRef, useState } from "react";

import * as api from "../lib/api";
import type { Scope } from "../bindings/Scope";
import { useWindowTheme } from "../lib/windowTheme";

/**
 * Where a note being read can find its sketches.
 *
 * Provided by the notes pane around the Markdown it renders, and absent
 * everywhere else. A sketch lives beside its note, in the same notebook, so the
 * one thing an embed needs to find it is which notebook that is — and a reply
 * in the transcript that happens to contain `![x](x.excalidraw)` has no notebook
 * at all, which is why it gets a placeholder rather than a guess.
 */
export const SketchHost = createContext<{
  scope: Scope;
  /** Opens the sketch in the editor, leaving the note. */
  open: (name: string) => void;
} | null>(null);

/**
 * A sketch, drawn into a note being read.
 *
 * Spans rather than divs throughout, because Markdown puts an image inside a
 * paragraph and a block element inside a `<p>` is markup the browser silently
 * rearranges.
 *
 * Read fresh every time it mounts — which is every time the note is switched to
 * Read — so an embed shows the sketch as it is now rather than as it was when
 * the note was opened. There is no second copy of a sketch anywhere, least of
 * all a cached picture of one.
 */
export function SketchEmbed({ name, alt }: { name: string; alt?: string }) {
  const host = useContext(SketchHost);
  const theme = useWindowTheme();
  const box = useRef<HTMLSpanElement>(null);
  const [state, setState] = useState<"reading" | "drawn" | "missing" | "unreadable">(
    "reading",
  );
  const [why, setWhy] = useState("");
  const scope = host?.scope ?? null;

  useEffect(() => {
    if (scope === null) return;
    let current = true;
    setState("reading");
    api
      .readPage(scope, "sketch", name)
      .then(async (page) => {
        const { draw } = await import("../lib/sketchSvg");
        const svg = await draw(page.text, theme === "dark");
        if (!current || !box.current) return;
        box.current.replaceChildren(svg);
        setState("drawn");
      })
      .catch((e) => {
        if (!current) return;
        const message = String(e);
        box.current?.replaceChildren();
        setWhy(message);
        setState(message.includes("no sketch called") ? "missing" : "unreadable");
      });
    return () => {
      current = false;
    };
  }, [scope, name, theme]);

  const title = alt?.trim() || name;

  if (!host) {
    return (
      <span className="sketch-embed">
        <span className="md-mermaid-note">
          A sketch, "{title}", which is drawn where the note it belongs to is
          read.
        </span>
      </span>
    );
  }

  return (
    <span className="sketch-embed">
      <span className="sketch-embed-head">
        <span className="md-code-lang">sketch</span>
        <span className="sketch-embed-name">{title}</span>
        <span className="spacer" />
        {state !== "missing" && (
          <button className="md-copy" onClick={() => host.open(name)}>
            open
          </button>
        )}
      </span>
      <span
        ref={box}
        className="sketch-embed-body"
        role="img"
        aria-label={`Sketch: ${title}`}
      />
      {state === "missing" && (
        // Not an error colour. A note pointing at a sketch that was renamed or
        // deleted is an ordinary thing to have in a notebook, and the fix is
        // one line in the note.
        <span className="md-mermaid-note">
          There is no sketch called '{name}' in this notebook. If it was
          renamed, change the name in this line of the note.
        </span>
      )}
      {state === "unreadable" && <span className="md-mermaid-note">{why}</span>}
    </span>
  );
}

/**
 * The sketch an image's address names, or `null` for any other image.
 *
 * A sketch is a `.excalidraw` file beside the note, named with the angle
 * brackets a name with a space needs — `![Auth flow](<Auth flow.excalidraw>)` —
 * which Markdown hands over percent-encoded. Anything with a separator in it
 * points outside the notebook and is left as the image it says it is.
 */
export function sketchName(src: string | undefined): string | null {
  if (!src) return null;
  let path: string;
  try {
    path = decodeURIComponent(src);
  } catch {
    return null;
  }
  path = path.replace(/^\.\//, "");
  if (!path.endsWith(".excalidraw") || /[\\/]/.test(path)) return null;
  const name = path.slice(0, -".excalidraw".length);
  return name === "" ? null : name;
}
