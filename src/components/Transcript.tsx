import { useEffect, useRef, useState } from "react";

import { Markdown } from "./Markdown";
import type { Entry } from "../state/store";

/** Icon per tool, so a long transcript is scannable without reading. */
const TOOL_GLYPH: Record<string, string> = {
  read_file: "◧",
  write_file: "✎",
  edit_file: "✎",
  list_dir: "▤",
  glob: "❋",
  grep: "⌕",
  run_command: "❯",
  load_skill: "◈",
  run_skill_script: "▷",
  propose_skill: "✦",
};

export function Transcript({ entries, busy }: { entries: Entry[]; busy: boolean }) {
  const bottom = useRef<HTMLDivElement>(null);
  const container = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  // Follow the stream, but stop fighting the user the moment they scroll up.
  useEffect(() => {
    const el = container.current;
    if (!el) return;
    const onScroll = () => {
      pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    };
    el.addEventListener("scroll", onScroll);
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  useEffect(() => {
    if (pinned.current) bottom.current?.scrollIntoView({ block: "end" });
  }, [entries]);

  if (entries.length === 0) {
    return (
      <div className="transcript empty" ref={container}>
        <div className="welcome">
          <h1>Taurus</h1>
          <p>
            A local agent shell. It reads and edits files in your workspace, runs
            commands, and writes down what it learns as reusable skills.
          </p>
          <p className="hint">
            Ask it to explore the project, make a change, or run something.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="transcript" ref={container}>
      {entries.map((entry) => (
        <EntryView key={entry.id} entry={entry} />
      ))}
      {busy && <div className="working">working…</div>}
      <div ref={bottom} />
    </div>
  );
}

function EntryView({ entry }: { entry: Entry }) {
  switch (entry.kind) {
    case "user":
      return (
        <div className="entry user">
          <div className="bubble">{entry.text}</div>
        </div>
      );

    case "assistant":
      return (
        <div className="entry assistant">
          {entry.thinking && <Thinking text={entry.thinking} />}
          {entry.text && (
            // `open` means the model is still writing into this entry, which
            // is what tells the renderer to coalesce parses while streaming.
            <Markdown text={entry.text} streaming={entry.open} />
          )}
        </div>
      );

    case "tool":
      return <ToolCard entry={entry} />;

    case "notice":
      return <div className={`notice ${entry.tone}`}>{entry.text}</div>;
  }
}

function Thinking({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="thinking">
      <button className="disclosure" onClick={() => setOpen(!open)}>
        {open ? "▾" : "▸"} reasoning
      </button>
      {open && <pre className="thinking-body">{text}</pre>}
    </div>
  );
}

function ToolCard({ entry }: { entry: Extract<Entry, { kind: "tool" }> }) {
  const [open, setOpen] = useState(false);
  const glyph = TOOL_GLYPH[entry.name] ?? "●";

  return (
    <div className={`tool ${entry.status}`}>
      <button className="tool-head" onClick={() => setOpen(!open)}>
        <span className="glyph">{glyph}</span>
        <span className="tool-preview">{entry.preview}</span>
        <span className="tool-status">
          {entry.status === "running" ? "…" : entry.status === "ok" ? "✓" : "✕"}
        </span>
      </button>
      {open && entry.output && <pre className="tool-output">{entry.output}</pre>}
    </div>
  );
}
