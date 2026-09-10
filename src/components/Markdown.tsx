import { memo, useEffect, useMemo, useRef, useState } from "react";
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

  // Rebuilt when `streaming` changes rather than declared once, because a
  // ```mermaid fence has to know: half a diagram is not one, and a fence three
  // lines into being written would otherwise flicker through refusals on the
  // way to a picture. Cheap, since the document is being re-parsed anyway.
  const components = useMemo(
    () => ({
      a: Anchor,
      code: (props: CodeProps) => <Code {...props} streaming={streaming} />,
      pre: Pre,
      img: Image,
      table: ({ node: _node, ...props }: { node?: unknown }) => (
        // Wide tables scroll inside their own box rather than stretching
        // the transcript.
        <div className="md-table-wrap">
          <table {...props} />
        </div>
      ),
    }),
    [streaming],
  );

  return (
    <div className="markdown">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text.length === 0 ? "" : throttled}
      </ReactMarkdown>
    </div>
  );
});

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

function Code({
  className,
  children,
  node: _node,
  streaming,
  ...rest
}: CodeProps & { streaming: boolean }) {
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

/** The `pre` wrapper is supplied by `Code`, so this one just passes through. */
function Pre({ children }: { children?: React.ReactNode }) {
  return <>{children}</>;
}
