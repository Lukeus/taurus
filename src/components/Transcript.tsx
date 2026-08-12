import { useEffect, useRef, useState } from "react";

import { Markdown } from "./Markdown";
import { duration, plural } from "../lib/format";
import type { Entry } from "../state/store";

type ToolEntry = Extract<Entry, { kind: "tool" }>;

/** Icon per tool, so a long transcript is scannable without reading. */
const TOOL_GLYPH: Record<string, string> = {
  read_file: "◧",
  write_file: "✎",
  edit_file: "✎",
  list_dir: "▤",
  glob: "❋",
  grep: "⌕",
  run_command: "❯",
  web_search: "◍",
  fetch_url: "⤓",
  load_skill: "◈",
  run_skill_script: "▷",
  propose_skill: "✦",
};

/**
 * How a step is counted in the run header, and how its row is tinted.
 *
 * `wrote` is the distinction that matters: everything else a turn does can be
 * repeated, and a write cannot. `net` is the other one worth seeing at a
 * glance — it is the only category where something left the machine. Reads are
 * the background noise of the run.
 */
const TOOL_CLASS: Record<string, "read" | "wrote" | "ran" | "net" | "skill"> = {
  read_file: "read",
  list_dir: "read",
  glob: "read",
  grep: "read",
  write_file: "wrote",
  edit_file: "wrote",
  run_command: "ran",
  web_search: "net",
  fetch_url: "net",
  load_skill: "skill",
  run_skill_script: "skill",
};

const CLASS_NOUN: Record<string, string> = {
  read: "read",
  wrote: "edited",
  ran: "command",
  // Covers both web tools: what they have in common, and the thing worth
  // counting, is that each one is a round trip off this machine.
  net: "request",
  skill: "skill",
  other: "tool",
};

export function Transcript({
  entries,
  busy,
  empty,
}: {
  entries: Entry[];
  busy: boolean;
  /** Shown in place of the transcript before there is anything to show. */
  empty: React.ReactNode;
}) {
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
        {empty}
      </div>
    );
  }

  return (
    <div className="transcript" ref={container}>
      {group(entries).map((item) =>
        Array.isArray(item) ? (
          <ToolRun key={item[0].id} steps={item} />
        ) : (
          <EntryView key={item.id} entry={item} />
        ),
      )}
      {busy && <div className="working">working…</div>}
      <div ref={bottom} />
    </div>
  );
}

/**
 * Folds each run of consecutive tool calls into one item.
 *
 * A turn that greps, reads two files, edits them and runs the tests is one
 * step of the conversation; drawn as six cards it buries the sentence either
 * side of it. Runs of one are grouped too — the header is still the right
 * place for its status, and a lone call that becomes two mid-stream must not
 * change shape as it does.
 */
export function group(entries: Entry[]): (Entry | ToolEntry[])[] {
  const out: (Entry | ToolEntry[])[] = [];
  for (const entry of entries) {
    const last = out[out.length - 1];
    if (entry.kind !== "tool") {
      out.push(entry);
    } else if (Array.isArray(last)) {
      last.push(entry);
    } else {
      out.push([entry]);
    }
  }
  return out;
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
      // Reached only if a caller renders an entry outside `group`.
      return <ToolRun steps={[entry]} />;

    case "notice":
      return entry.rule ? (
        <div className="rule">
          <span className="micro">{entry.rule.label}</span>
          <div className="rule-line" />
          <span className="rule-note">{entry.rule.note}</span>
        </div>
      ) : (
        <div className={`notice ${entry.tone}`}>{entry.text}</div>
      );
  }
}

function Thinking({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <div className="rule">
        <span className="micro">Reasoning</span>
        <div className="rule-line" />
        <button onClick={() => setOpen(!open)}>{open ? "hide" : "show"}</button>
      </div>
      {open && <pre className="thinking-body">{text}</pre>}
    </>
  );
}

function ToolRun({ steps }: { steps: ToolEntry[] }) {
  const [open, setOpen] = useState(true);

  const failed = steps.some((s) => s.status === "error");
  const running = steps.some((s) => s.status === "running");
  const elapsed = span(steps);

  return (
    <div className={`run${open ? " open" : ""}`}>
      <button className="run-head" onClick={() => setOpen(!open)}>
        <span className={`dot ${failed ? "error" : running ? "warn" : "ok"}`} />
        <span className="run-steps">
          {plural(steps.length, "step")}
          {elapsed !== null && ` · ${duration(elapsed)}`}
        </span>
        <div className="spacer" />
        <span className="run-breakdown">{breakdown(steps)}</span>
        <span className="run-chevron">{open ? "⌄" : "›"}</span>
      </button>
      {open && steps.map((step) => <ToolRow key={step.id} step={step} />)}
    </div>
  );
}

function ToolRow({ step }: { step: ToolEntry }) {
  const [open, setOpen] = useState(false);
  const kind = TOOL_CLASS[step.name] ?? "other";

  return (
    <div className={`run-row ${kind} ${step.status}`}>
      <button
        className="run-row-head"
        disabled={!step.output}
        onClick={() => setOpen(!open)}
        title={step.output ? "Show what it returned" : undefined}
      >
        <span className="glyph">{TOOL_GLYPH[step.name] ?? "●"}</span>
        <span className="run-row-text">{step.preview}</span>
        <span className="run-row-status">
          {step.status === "running" ? "…" : step.status === "ok" ? "✓" : "failed"}
        </span>
      </button>
      {/* What a delegation is doing while it does it. Shown without needing
          the row opened: the point is that a long call looks alive, and that
          is no use behind a click. Dropped once it finishes, when the result
          is the more useful thing to have in the same space. */}
      {step.status === "running" && step.steps.length > 0 && (
        <ul className="run-substeps">
          {step.steps.slice(-MAX_VISIBLE_SUBSTEPS).map((label, i) => (
            <li key={`${i}-${label}`}>{label}</li>
          ))}
        </ul>
      )}
      {open && step.output && <pre className="tool-output">{step.output}</pre>}
    </div>
  );
}

/**
 * A delegation can make dozens of calls. The last few say what it is doing
 * now; the whole list would push the conversation off the screen.
 */
const MAX_VISIBLE_SUBSTEPS = 4;

/**
 * How long the run took, or `null` when it cannot be known.
 *
 * A resumed conversation carries no timestamps, and a run still in flight has
 * no end. Both leave the header saying only how many steps there were, which
 * is the truth in each case.
 */
export function span(steps: ToolEntry[]): number | null {
  const starts = steps.map((s) => s.startedAt).filter((t) => t !== undefined);
  const ends = steps.map((s) => s.endedAt).filter((t) => t !== undefined);
  if (starts.length !== steps.length || ends.length !== steps.length) return null;
  return Math.max(...ends) - Math.min(...starts);
}

/** `2 read · 2 edited · 1 command` — the shape of the run, in one line. */
export function breakdown(steps: ToolEntry[]): string {
  const counts = new Map<string, number>();
  for (const step of steps) {
    const kind = TOOL_CLASS[step.name] ?? "other";
    counts.set(kind, (counts.get(kind) ?? 0) + 1);
  }
  return [...counts]
    .map(([kind, n]) => `${n} ${CLASS_NOUN[kind]}${n === 1 ? "" : plurals(kind)}`)
    .join(" · ");
}

/** `read` and `edited` are already past participles; the nouns take an s. */
function plurals(kind: string): string {
  return kind === "read" || kind === "wrote" ? "" : "s";
}
