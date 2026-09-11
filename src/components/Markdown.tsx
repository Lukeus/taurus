import { createContext, memo, useContext, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";

import { grammarFor, paint } from "../lib/ink";
import { CopyButton } from "./CopyButton";
import { MermaidBlock } from "./MermaidBlock";
import { SketchEmbed, sketchName } from "./SketchEmbed";

/**
 * Markdown rendering for assistant output.
 *
 * Two constraints shape this. The text arrives a token at a time, so the
 * parser has to cope with half-finished constructs — an unclosed `**`, a code
 * fence with no terminator — on every render. And the text comes from a model,
 * so raw HTML is never enabled: `react-markdown` ignores HTML unless you add
 * `rehype-raw`, and we deliberately do not.
 */

/**
 * Parsing on every token is wasteful on a fast model and invisible to the eye.
 * Coalescing to ~16/second keeps it live without re-parsing a long document
 * hundreds of times.
 */
const STREAM_FRAME_MS = 60;

export const Markdown = memo(function Markdown({
  text,
  streaming,
}: {
  text: string;
  streaming: boolean;
}) {
  const throttled = useThrottled(text, streaming ? STREAM_FRAME_MS : 0);
  const shown = text.length === 0 ? "" : throttled;
  const parts = useMemo(() => blocks(shown), [shown]);

  return (
    <Streaming.Provider value={streaming}>
      <div className="markdown">
        {/* Keyed by where each stretch starts, which never moves as the text
            grows. The stretch being written keeps its key when the paragraph
            after it begins, so what it drew stays mounted. */}
        {parts.map((part) => (
          <Block key={part.at} text={part.text} />
        ))}
      </div>
    </Streaming.Provider>
  );
});

/**
 * One stretch of an answer, parsed on its own.
 *
 * Memoized on its text, which is the reason for cutting the answer up. Every
 * stretch but the last is the same string frame after frame, so only the
 * paragraph being written is parsed again — where parsing the whole answer on
 * every frame made each frame cost more than the one before it.
 */
const Block = memo(function Block({ text }: { text: string }) {
  return (
    <ReactMarkdown remarkPlugins={PLUGINS} components={COMPONENTS}>
      {text}
    </ReactMarkdown>
  );
});

/** A stretch of Markdown and the offset it starts at. */
export type Stretch = { at: number; text: string };

/**
 * Cuts Markdown where parsing the pieces apart gives what parsing it whole
 * would.
 *
 * A cut goes before a line that starts a fresh block: one that follows a blank
 * line outside a fence, begins at the margin, and cannot be the next item of a
 * list. A blank line ends a paragraph, a line at the margin ends a list or a
 * quote, and nothing else in CommonMark reaches across that — so each piece
 * parses to the same blocks it would have been part of.
 *
 * Every rule here errs towards not cutting, because a missed cut costs a
 * re-parse and a wrong one changes what is on the page. A line that might
 * continue a list is not cut before, an indented line is not, and text that
 * holds anything able to reach across a blank line is not cut at all: a link
 * or footnote definition, which applies to the whole document, and the raw
 * HTML blocks that run to their own end marker.
 */
export function blocks(text: string): Stretch[] {
  if (WHOLE.test(text)) return [{ at: 0, text }];

  const out: Stretch[] = [];
  let start = 0;
  let fence: { char: string; length: number } | null = null;
  let afterBlank = false;
  let at = 0;
  while (at < text.length) {
    const newline = text.indexOf("\n", at);
    const end = newline === -1 ? text.length : newline;
    const line = text.slice(at, end);

    if (fence) {
      const close = FENCE.exec(line);
      if (
        close &&
        close[2][0] === fence.char &&
        close[2].length >= fence.length &&
        close[3].trim() === ""
      ) {
        fence = null;
      }
      afterBlank = false;
    } else if (line.trim() === "") {
      afterBlank = true;
    } else {
      if (afterBlank && at > start && FRESH.test(line) && !LIST_ITEM.test(line)) {
        out.push({ at: start, text: text.slice(start, at) });
        start = at;
      }
      afterBlank = false;
      const open = FENCE.exec(line);
      // A backtick fence's info string may not hold a backtick; one that does
      // is inline code, and opens nothing.
      if (open && !(open[2][0] === "`" && open[3].includes("`"))) {
        fence = { char: open[2][0], length: open[2].length };
      }
    }
    at = end + 1;
  }
  out.push({ at: start, text: text.slice(start) });
  return out;
}

/** A code fence's opening or closing line: indent, the run, what follows. */
const FENCE = /^( {0,3})(`{3,}|~{3,})(.*)$/;
/** A line at the margin that is not raw HTML. */
const FRESH = /^[^\s<]/;
/** A bullet or an ordered marker, which after a blank line continues a list. */
const LIST_ITEM = /^([-+*]|\d{1,9}[.)])(\s|$)/;
/** What can reach across a blank line: a definition, or a raw HTML block that
 *  ends only at its own marker. */
const WHOLE = /^ {0,3}\[[^\]]+\]:|<(!--|\?|!\[CDATA\[|pre\b|script\b|style\b|textarea\b)/im;

/**
 * Whether the text a block sits in is still arriving.
 *
 * A ```mermaid fence has to know: half a diagram is not one, and a fence three
 * lines into being written would otherwise flicker through refusals on the way
 * to a picture.
 *
 * Handed down through context rather than closed over by the `code` renderer.
 * A renderer that closes over it is a new component every time it changes, and
 * React remounts whatever a new component draws — so the moment an answer
 * finished, every code block in it re-highlighted and every diagram lost the
 * view it was showing.
 */
const Streaming = createContext(false);

const PLUGINS = [remarkGfm];

/** One set of renderers for the life of the module. Their identity is what
 *  React uses to decide a block is the same block, so it must never move. */
const COMPONENTS = { a: Anchor, code: Code, pre: Pre, img: Image, table: Table };

/** Returns `value`, but changing at most once per `ms`. */
function useThrottled(value: string, ms: number): string {
  const [shown, setShown] = useState(value);
  const pending = useRef(value);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    pending.current = value;
    if (ms === 0) {
      setShown(value);
      return;
    }
    if (timer.current !== null) return;
    timer.current = window.setTimeout(() => {
      timer.current = null;
      setShown(pending.current);
    }, ms);
  }, [value, ms]);

  // Once streaming stops, show everything immediately rather than waiting out
  // the last frame.
  useEffect(() => {
    if (ms === 0 && timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
      setShown(pending.current);
    }
  }, [ms]);

  useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    },
    [],
  );

  return shown;
}

/**
 * Opens links in the user's browser.
 *
 * Without this a click navigates the webview itself, replacing the app with
 * whatever the model linked to and losing the session.
 */
function Anchor({
  href,
  children,
}: {
  href?: string;
  children?: React.ReactNode;
}) {
  return (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        if (href) void openUrl(href).catch(() => {});
      }}
    >
      {children}
    </a>
  );
}

type CodeProps = {
  className?: string;
  children?: React.ReactNode;
  // `react-markdown` hands every component its AST node. Spreading it onto a
  // DOM element leaks `node="[object Object]"` into the markup, so it is
  // pulled out rather than forwarded.
  node?: unknown;
};

function Code({ className, children, node: _node, ...rest }: CodeProps) {
  const streaming = useContext(Streaming);
  // `react-markdown` routes both inline spans and fenced blocks here; only the
  // fenced ones carry a `language-*` class, and inline code has no newline.
  const language = /language-(\w+)/.exec(className ?? "")?.[1];
  const body = String(children ?? "");
  const isBlock = language !== undefined || body.includes("\n");

  if (!isBlock) {
    return <code className="md-inline-code" {...rest}>{children}</code>;
  }

  // A diagram rather than its source, where it can be read as one. The fence
  // keeps a **source** toggle, because a picture is easier to read and harder
  // to check than the text it came from.
  if (language === "mermaid") {
    return <MermaidBlock source={body} streaming={streaming} />;
  }

  return (
    <div className="md-code">
      <div className="md-code-head">
        <span className="md-code-lang">{language ?? "text"}</span>
        <CopyButton className="md-copy" text={body.replace(/\n$/, "")} />
      </div>
      <pre>
        <Painted source={body} language={language} className={className} />
      </pre>
    </div>
  );
}

/**
 * A fenced block, coloured if the language is one the scanner knows.
 *
 * The text is what goes on the page either way — an unknown language comes
 * back as a single plain run, which renders as exactly the `<code>` that was
 * here before. That is why there is no branch on whether colouring worked:
 * there is nothing to fall back to, because the fallback is the same code
 * path with one run in it.
 *
 * Memoized on the two things that decide the answer. This runs inside a
 * transcript that re-renders on every streamed token, and re-scanning every
 * finished block in the conversation to draw one new character is the kind of
 * cost that does not show up until the conversation is long.
 */
function Painted({
  source,
  language,
  className,
}: {
  source: string;
  language: string | undefined;
  className?: string;
}) {
  const runs = useMemo(() => paint(source, grammarFor(language)), [source, language]);
  return (
    <code className={className}>
      {runs.map((run, i) => (
        <span key={i} className={`ink-${run.kind}`}>
          {run.text}
        </span>
      ))}
    </code>
  );
}

/**
 * An image, unless it is a sketch.
 *
 * A note embeds a sketch the way Markdown embeds anything, as an image whose
 * address is the file — so a note stays ordinary Markdown that any other viewer
 * shows as a broken image with its name on, which is the honest degradation
 * rather than a syntax only this app reads. Every other image is exactly the
 * `<img>` it was before.
 */
function Image({
  src,
  alt,
  node: _node,
  ...rest
}: React.ImgHTMLAttributes<HTMLImageElement> & { node?: unknown }) {
  const sketch = sketchName(typeof src === "string" ? src : undefined);
  if (sketch !== null) return <SketchEmbed name={sketch} alt={alt} />;
  return <img src={src} alt={alt} {...rest} />;
}

/** Wide tables scroll inside their own box rather than stretching the
 *  transcript. */
function Table({ node: _node, ...props }: { node?: unknown }) {
  return (
    <div className="md-table-wrap">
      <table {...props} />
    </div>
  );
}

/** The `pre` wrapper is supplied by `Code`, so this one just passes through. */
function Pre({ children }: { children?: React.ReactNode }) {
  return <>{children}</>;
}
