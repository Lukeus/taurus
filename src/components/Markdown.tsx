import { memo, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";

import { CopyButton } from "./CopyButton";

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

  return (
    <div className="markdown">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: Anchor,
          code: Code,
          pre: Pre,
          table: ({ node: _node, ...props }) => (
            // Wide tables scroll inside their own box rather than stretching
            // the transcript.
            <div className="md-table-wrap">
              <table {...props} />
            </div>
          ),
        }}
      >
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

function Code({
  className,
  children,
  // `react-markdown` hands every component its AST node. Spreading it onto a
  // DOM element leaks `node="[object Object]"` into the markup, so it is
  // pulled out here rather than forwarded.
  node: _node,
  ...rest
}: {
  className?: string;
  children?: React.ReactNode;
  node?: unknown;
}) {
  // `react-markdown` routes both inline spans and fenced blocks here; only the
  // fenced ones carry a `language-*` class, and inline code has no newline.
  const language = /language-(\w+)/.exec(className ?? "")?.[1];
  const body = String(children ?? "");
  const isBlock = language !== undefined || body.includes("\n");

  if (!isBlock) {
    return <code className="md-inline-code" {...rest}>{children}</code>;
  }

  return (
    <div className="md-code">
      <div className="md-code-head">
        <span className="md-code-lang">{language ?? "text"}</span>
        <CopyButton className="md-copy" text={body.replace(/\n$/, "")} />
      </div>
      <pre>
        <code className={className}>{children}</code>
      </pre>
    </div>
  );
}

/** The `pre` wrapper is supplied by `Code`, so this one just passes through. */
function Pre({ children }: { children?: React.ReactNode }) {
  return <>{children}</>;
}
