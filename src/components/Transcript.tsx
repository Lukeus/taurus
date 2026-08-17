import { useEffect, useRef, useState } from "react";


import { ChartCard } from "./ChartCard";
import { Markdown } from "./Markdown";
import { Attachments } from "./Attachments";
import { QuestionsCard } from "./QuestionsCard";
import { TableCard } from "./TableCard";
import { duration, plural } from "../lib/format";
import type { Answer } from "../lib/api";
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
  onAnswer,
}: {
  entries: Entry[];
  busy: boolean;
  /** Shown in place of the transcript before there is anything to show. */
  empty: React.ReactNode;
  /** Answers a question card, releasing the tool call parked behind it. */
  onAnswer: (id: string, answers: Answer[]) => void;
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

  const conversation = turns(entries);

  return (
    <div className="transcript" ref={container}>
      {conversation.map((turn, i) => (
        <TurnView
          key={turn.prompt?.id ?? `preamble-${i}`}
          turn={turn}
          // Only ever the last one: the thing still being worked on is the
          // question most recently asked, and the marker belongs on its rail
          // rather than floating under the conversation as a whole.
          working={busy && i === conversation.length - 1}
          onAnswer={onAnswer}
        />
      ))}
      <div ref={bottom} />
    </div>
  );
}

type UserEntry = Extract<Entry, { kind: "user" }>;

/** One question and everything it caused, in the order it happened. */
export type Turn = {
  /**
   * The message that began it.
   *
   * Null only for a conversation's preamble — the note a session opens with
   * when its model has no native tool calling arrives before anybody has asked
   * anything, and a rail hanging off nothing would say it belonged to a
   * question that was never put.
   */
  prompt: UserEntry | null;
  body: (Entry | ToolEntry[])[];
};

/**
 * Cuts the transcript into turns.
 *
 * The unit the eye needs is the exchange, not the entry. Drawn as one flat
 * column, a question and the six things that answered it are seven siblings
 * with equal claim on the page, and the only way to find where a turn began is
 * to read for it. Grouped, the question heads its own answer and the rail down
 * the side says how far it reaches.
 */
export function turns(entries: Entry[]): Turn[] {
  const out: { prompt: UserEntry | null; body: Entry[] }[] = [];
  for (const entry of entries) {
    if (entry.kind === "user") {
      out.push({ prompt: entry, body: [] });
    } else {
      if (out.length === 0) out.push({ prompt: null, body: [] });
      out[out.length - 1].body.push(entry);
    }
  }
  // Runs of tool calls are folded per turn rather than across the transcript.
  // They could not have spanned two anyway — a user message ends whatever was
  // running — and folding here keeps the two groupings from having to agree.
  return out.map(({ prompt, body }) => ({ prompt, body: group(body) }));
}

/**
 * A turn, drawn as a thread.
 *
 * Everything the turn produced hangs off one rail that starts at the question,
 * so the answer is visibly the answer *to that* — and a long turn stays one
 * object on the page however many steps it took. The alternative, and what this
 * replaced, was the question pinned to the right margin and its answer to the
 * left, sharing no edge and connected by nothing.
 */
function TurnView({
  turn,
  working,
  onAnswer,
}: {
  turn: Turn;
  working: boolean;
  onAnswer: (id: string, answers: Answer[]) => void;
}) {
  return (
    <section className={`turn${turn.prompt ? "" : " unprompted"}`}>
      {turn.prompt && <Prompt entry={turn.prompt} />}
      {turn.body.map((item) =>
        Array.isArray(item) ? (
          <div className="turn-step" key={item[0].id}>
            <ToolRun steps={item} />
          </div>
        ) : (
          <div className="turn-step" key={item.id}>
            <EntryView entry={item} onAnswer={onAnswer} />
          </div>
        ),
      )}
      {working && (
        <div className="turn-step working" aria-live="polite">
          working…
        </div>
      )}
    </section>
  );
}

/** The question, at the head of the thread that answers it. */
function Prompt({ entry }: { entry: UserEntry }) {
  return (
    <div className="turn-step prompt">
      {/* Above the text, matching the order they were sent in and the order
          the model reads them: the picture, then the question. */}
      {entry.images && entry.images.length > 0 && (
        <Attachments images={entry.images} />
      )}
      <div className="prompt-text">{entry.text}</div>
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
 *
 * A call that drew something is the exception, and stands alone. Folding a
 * table into a run would file the answer under "6 steps · 11s" behind a
 * disclosure triangle, and a question card there would be a question nobody
 * saw. These are not steps on the way to the reply; they are part of it.
 */
export function group(entries: Entry[]): (Entry | ToolEntry[])[] {
  const out: (Entry | ToolEntry[])[] = [];
  for (const entry of entries) {
    const last = out[out.length - 1];
    if (entry.kind !== "tool" || entry.view) {
      out.push(entry);
    } else if (Array.isArray(last)) {
      last.push(entry);
    } else {
      out.push([entry]);
    }
  }
  return out;
}

function EntryView({
  entry,
  onAnswer,
}: {
  entry: Entry;
  onAnswer: (id: string, answers: Answer[]) => void;
}) {
  if (entry.kind === "tool" && entry.view) {
    switch (entry.view.type) {
      case "table":
        return <TableCard view={entry.view} />;
      case "chart":
        return <ChartCard view={entry.view} />;
      case "questions":
        return (
          <QuestionsCard
            view={entry.view}
            status={entry.status}
            output={entry.output}
            onAnswer={onAnswer}
          />
        );
      // No case for `plan`. It is pinned above the composer instead — see
      // `PlanPanel` — so the call falls through to its own row here, which is
      // what the run header counts. The view is still on the entry: that is
      // where `pinnedPlan` reads it from.
    }
  }

  switch (entry.kind) {
    case "user":
      // Reached only if a caller renders an entry outside `turns`, which is
      // what heads each thread with its own question.
      return <Prompt entry={entry} />;

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

/**
 * Tools whose progress reports are their own output rather than labels.
 *
 * These get a terminal: monospace, whitespace kept, and a view that stays put
 * once the command finishes. Everything else finishes fast enough that the
 * collapsed row is the better reading experience, and streaming all of them
 * would turn a transcript into a wall.
 */
const STREAMS_OUTPUT = new Set(["run_command", "run_skill_script"]);

function ToolRow({ step }: { step: ToolEntry }) {
  const [open, setOpen] = useState(false);
  const kind = TOOL_CLASS[step.name] ?? "other";
  const terminal = STREAMS_OUTPUT.has(step.name);

  // While it runs, what has been streamed; once it is done, the result — which
  // is the authoritative copy, truncated at both ends rather than the tail
  // alone. Same place on screen either way, so the row does not jump when the
  // command exits.
  const streamed = step.steps.join("");
  const body = step.status === "running" ? streamed : (step.output ?? streamed);

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

      {terminal && body && (
        <Terminal text={body} following={step.status === "running"} expanded={open} />
      )}

      {/* What a delegation is doing while it does it. Shown without needing
          the row opened: the point is that a long call looks alive, and that
          is no use behind a click. Dropped once it finishes, when the result
          is the more useful thing to have in the same space. */}
      {!terminal && step.status === "running" && step.steps.length > 0 && (
        <ul className="run-substeps">
          {step.steps.slice(-MAX_VISIBLE_SUBSTEPS).map((label, i) => (
            <li key={`${i}-${label}`}>{label}</li>
          ))}
        </ul>
      )}
      {!terminal && open && step.output && (
        <pre className="tool-output">{step.output}</pre>
      )}
    </div>
  );
}

/**
 * A command's output, following it as it arrives.
 *
 * Scrolled rather than grown: a `cargo build` would otherwise push the
 * conversation off the screen and take the composer with it. The view sticks to
 * the bottom while the command runs and stops the moment the user scrolls up,
 * the same bargain the transcript itself makes.
 */
function Terminal({
  text,
  following,
  expanded,
}: {
  text: string;
  following: boolean;
  expanded: boolean;
}) {
  const box = useRef<HTMLPreElement>(null);
  const pinned = useRef(true);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const onScroll = () => {
      pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
    };
    el.addEventListener("scroll", onScroll);
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  useEffect(() => {
    const el = box.current;
    if (el && following && pinned.current) el.scrollTop = el.scrollHeight;
  }, [text, following]);

  return (
    <pre
      ref={box}
      className={`tool-stream${expanded ? " expanded" : ""}${following ? " live" : ""}`}
    >
      {text.replace(/\n+$/, "")}
    </pre>
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
