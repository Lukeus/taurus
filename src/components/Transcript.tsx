import { memo, useCallback, useEffect, useRef, useState } from "react";


import { ChartCard } from "./ChartCard";
import { FlowCard } from "./FlowCard";
import { SequenceCard } from "./SequenceCard";
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
  remember: "⚑",
};

/**
 * How a step is counted in the run header, and how its row is tinted.
 *
 * `wrote` is the distinction that matters: everything else a turn does can be
 * repeated, and a write cannot. `net` is the other one worth seeing at a
 * glance — it is the only category where something left the machine. Reads are
 * the background noise of the run.
 */
const TOOL_CLASS: Record<
  string,
  "read" | "wrote" | "ran" | "net" | "skill" | "kept"
> = {
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
  // Its own category rather than a read or a write. Nothing in the workspace
  // moved, but something was kept — and a note is the one step in a turn whose
  // effect is on the *next* conversation, which is worth being able to spot
  // while scanning back through this one.
  remember: "kept",
};

const CLASS_NOUN: Record<string, string> = {
  read: "read",
  wrote: "edited",
  ran: "command",
  // Covers both web tools: what they have in common, and the thing worth
  // counting, is that each one is a round trip off this machine.
  net: "request",
  skill: "skill",
  kept: "note",
  other: "tool",
};

export type TranscriptProps = {
  entries: Entry[];
  busy: boolean;
  /**
   * Stop has been pressed and the turn has not finished unwinding.
   *
   * Only ever read while `busy`, and only to change what the marker says: a
   * cancel takes as long as the in-flight tool call takes to notice, and a
   * `working…` that carried on through that read as a Stop that had not
   * registered.
   */
  stopping?: boolean;
  /**
   * The conversation on screen is being replaced by another.
   *
   * Drawn as a fade that only starts after a sixth of a second — see
   * `.transcript.pending`. An ordinary reopen is faster than that and shows
   * nothing at all, which is the point: a dim that flashes on every switch
   * would be worse than the wait it is reporting.
   */
  pending?: boolean;
  /** Shown in place of the transcript before there is anything to show. */
  empty: React.ReactNode;
  /** Answers a question card, releasing the tool call parked behind it. */
  onAnswer: (id: string, answers: Answer[]) => void | Promise<void>;
  /** Opens a delegation's own conversation. Absent where there is nowhere to
   * open one — inside a delegate's transcript, which cannot delegate further. */
  onOpenDelegate?: (transcript: { session: string; agent: string }) => void;
  /**
   * Whether to follow new entries to the bottom.
   *
   * True for the live conversation, which is being written as it is read. False
   * for a record of one that already finished: it is read from the top, and
   * opening it at the last line is opening it at the end of the book.
   */
  follow?: boolean;
};

export function Transcript({
  entries,
  busy,
  stopping = false,
  pending = false,
  empty,
  onAnswer,
  onOpenDelegate,
  follow = true,
}: TranscriptProps) {
  const bottom = useRef<HTMLDivElement>(null);
  const container = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  // Held steady before they go any further down. Everything below this is
  // memoized, and a memo compares its props — so a caller writing
  // `onAnswer={() => …}` inline, which is the natural way to write it, would
  // hand every turn a new function on every token and undo the lot. Making the
  // component robust to that is worth more than a rule callers have to know:
  // `DelegateTranscript` already passes one, correctly, because there is
  // nothing there to answer.
  const answer = useStable(onAnswer);
  const openDelegate = useStable(onOpenDelegate);

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

  /*
   * Stay at the foot of the stream.
   *
   * `scrollIntoView` rather than `scrollTop = scrollHeight`, which looks like
   * the cheaper way to say the same thing and is not the same thing. `.turn`
   * carries `content-visibility: auto`, so a turn scrolled off is measured by
   * its `contain-intrinsic-size` estimate rather than its contents — jumping to
   * `scrollHeight` therefore aims past the real bottom, the turns it lands on
   * render at their true height, the document shrinks under the scroll, and the
   * view snaps back up. Measured: it lands at the *top* of a long conversation.
   * Asking for an element instead lets the browser iterate that to a fixed
   * point, which is the whole reason the API exists.
   *
   * The cost this used to carry — a full-transcript layout on every batched
   * frame, ~30 a second while streaming — is paid down by the windowing rather
   * than by scrolling differently: off-screen turns no longer take part in it.
   */
  useEffect(() => {
    if (follow && pinned.current) bottom.current?.scrollIntoView({ block: "end" });
  }, [entries, follow]);

  // Above the empty case, not below it. This is a hook, and a hook behind an
  // early return is called on some renders and not others — so the first token
  // of a fresh conversation, which is exactly the moment the transcript stops
  // being empty, would take the component down with it.
  const conversation = useStableTurns(entries);

  if (entries.length === 0) {
    return (
      <div className="transcript empty" ref={container}>
        {empty}
      </div>
    );
  }

  return (
    <div
      className={`transcript${pending ? " pending" : ""}`}
      aria-busy={pending || undefined}
      ref={container}
    >
      {conversation.map((turn, i) => (
        <TurnView
          key={turn.prompt?.id ?? `preamble-${i}`}
          turn={turn}
          // Only ever the last one: the thing still being worked on is the
          // question most recently asked, and the marker belongs on its rail
          // rather than floating under the conversation as a whole.
          working={busy && i === conversation.length - 1}
          stopping={stopping}
          onAnswer={answer}
          onOpenDelegate={openDelegate}
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
 * One function identity for the life of the component, always calling the
 * newest one it was given.
 *
 * The identity is what the memos compare; the freshness is what keeps a
 * callback from closing over a stale render. Without the second half this would
 * be a cache that answers questions with last week's answer.
 */
function useStable<A extends unknown[]>(
  fn: (...args: A) => void,
): (...args: A) => void;
function useStable<A extends unknown[]>(
  fn: ((...args: A) => void) | undefined,
): ((...args: A) => void) | undefined;
function useStable<A extends unknown[]>(
  fn: ((...args: A) => void) | undefined,
): ((...args: A) => void) | undefined {
  const held = useRef(fn);
  held.current = fn;

  const stable = useCallback((...args: A) => held.current?.(...args), []);

  // Absence is meaningful further down — a row offers to open a delegate's
  // conversation only where there is somewhere to open one — so an absent
  // callback has to stay absent rather than becoming a function that does
  // nothing.
  return fn === undefined ? undefined : stable;
}

/**
 * `turns(entries)`, carrying forward the objects it built last time for the
 * turns that did not change.
 *
 * `turns` builds fresh objects on every call, and fresh objects are what React
 * reads as "this is different, draw it again" — so one token appended to the
 * last turn used to redraw every turn above it, and a long conversation got
 * slower to type into the longer it ran.
 *
 * Nothing about the transcript makes that necessary. The reducer preserves the
 * identity of every entry it did not touch, so a turn whose entries are all the
 * same objects is provably the same turn, and can keep the identity it already
 * had. That is the whole of what lets `TurnView` be memoized: without it the
 * memo compares two freshly-built turns, finds them different, and saves
 * nothing.
 *
 * Building the list is still O(entries) per render. It is the drawing that was
 * expensive, and this is what stops paying for it.
 */
function useStableTurns(entries: Entry[]): Turn[] {
  const held = useRef<Turn[]>([]);
  // Written during the render rather than in an effect, which is normally the
  // wrong side of that line: a render React throws away — a StrictMode double
  // pass, an interrupted one — leaves this holding turns from a render that
  // never reached the screen. It is safe here because of what `reuse` asks. It
  // carries an object forward only when the entries in it are the *same
  // objects*, so a turn from an abandoned render is either identical to the one
  // that replaces it or is not reused at all. There is no state to be stale.
  held.current = reuse(held.current, turns(entries));
  return held.current;
}

/**
 * `next`, with every turn that matches the one `previous` held at the same
 * position replaced by that one.
 *
 * Pulled out of the hook because this is the whole property the memo depends
 * on, and a property worth a test of its own: a refactor that stops turns
 * carrying their identity forward would cost nothing visible and quietly
 * restore the behaviour this replaced.
 */
export function reuse(previous: Turn[], next: Turn[]): Turn[] {
  return next.map((turn, i) => {
    const held = previous[i];
    return held && unchanged(held, turn) ? held : turn;
  });
}

/**
 * Whether two turns hold the same entries, by identity.
 *
 * Deliberately not a deep comparison. Every entry the reducer rewrites is a new
 * object and every entry it leaves alone is the same one, so identity is the
 * exact question — and a deep walk would cost more than the render it is trying
 * to avoid.
 */
function unchanged(a: Turn, b: Turn): boolean {
  if (a.prompt !== b.prompt || a.body.length !== b.body.length) return false;

  return a.body.every((item, i) => {
    const other = b.body[i];
    // A folded run of tool calls is rebuilt by `group` on every call, so the
    // arrays never match by identity even when nothing in them moved.
    if (Array.isArray(item) || Array.isArray(other)) {
      return (
        Array.isArray(item) &&
        Array.isArray(other) &&
        item.length === other.length &&
        item.every((step, k) => step === other[k])
      );
    }
    return item === other;
  });
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
const TurnView = memo(function TurnView({
  turn,
  working,
  stopping,
  onAnswer,
  onOpenDelegate,
}: {
  turn: Turn;
  working: boolean;
  /** Cancelling. Only ever read when `working`. */
  stopping: boolean;
  onAnswer: (id: string, answers: Answer[]) => void | Promise<void>;
  onOpenDelegate?: (transcript: { session: string; agent: string }) => void;
}) {
  return (
    <section className={`turn${turn.prompt ? "" : " unprompted"}`}>
      {turn.prompt && <Prompt entry={turn.prompt} />}
      {turn.body.map((item) =>
        Array.isArray(item) ? (
          <div className="turn-step" key={item[0].id}>
            <ToolRun steps={item} onOpenDelegate={onOpenDelegate} />
          </div>
        ) : (
          <div className="turn-step" key={item.id}>
            <EntryView
              entry={item}
              onAnswer={onAnswer}
              onOpenDelegate={onOpenDelegate}
            />
          </div>
        ),
      )}
      {working && (
        <div className="turn-step working" aria-live="polite">
          {stopping ? "stopping…" : "working…"}
        </div>
      )}
    </section>
  );
});

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

/**
 * Memoized on the entry itself, which is what stops the cards in a turn being
 * redrawn while the sentence after them is still arriving. A table the model
 * drew four tool calls ago has not changed; recomputing its layout thirty times
 * a second to prove that is the work worth not doing.
 */
const EntryView = memo(function EntryView({
  entry,
  onAnswer,
  onOpenDelegate,
}: {
  entry: Entry;
  onAnswer: (id: string, answers: Answer[]) => void | Promise<void>;
  onOpenDelegate?: (transcript: { session: string; agent: string }) => void;
}) {
  if (entry.kind === "tool" && entry.view) {
    switch (entry.view.type) {
      case "table":
        return <TableCard view={entry.view} />;
      case "chart":
        return <ChartCard view={entry.view} />;
      case "sequence":
        return <SequenceCard view={entry.view} />;
      case "flow":
        return <FlowCard view={entry.view} />;
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
      return <ToolRun steps={[entry]} onOpenDelegate={onOpenDelegate} />;

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
});

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

/**
 * A folded run of tool calls.
 *
 * Memoized with a comparison of its own because `group` rebuilds the array on
 * every render: the steps inside it are the same objects, but the array holding
 * them never is, so the default shallow compare would find a difference every
 * time and the memo would do nothing. What it saves is the common shape of a
 * turn — a run of calls, and then the answer streaming in underneath it.
 */
const ToolRun = memo(function ToolRun({
  steps,
  onOpenDelegate,
}: {
  steps: ToolEntry[];
  onOpenDelegate?: (transcript: { session: string; agent: string }) => void;
}) {
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
      {open &&
        steps.map((step) => (
          <ToolRow key={step.id} step={step} onOpenDelegate={onOpenDelegate} />
        ))}
    </div>
  );
},
(a, b) =>
  a.onOpenDelegate === b.onOpenDelegate &&
  a.steps.length === b.steps.length &&
  a.steps.every((step, i) => step === b.steps[i]));

/**
 * Tools whose progress reports are their own output rather than labels.
 *
 * These get a terminal: monospace, whitespace kept, and a view that stays put
 * once the command finishes. Everything else finishes fast enough that the
 * collapsed row is the better reading experience, and streaming all of them
 * would turn a transcript into a wall.
 */
const STREAMS_OUTPUT = new Set(["run_command", "run_skill_script"]);

function ToolRow({
  step,
  onOpenDelegate,
}: {
  step: ToolEntry;
  onOpenDelegate?: (transcript: { session: string; agent: string }) => void;
}) {
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
      {/* A delegation's own conversation, one click away rather than in the
          way. Outside the head button rather than in it — a button inside a
          button is not a thing — and present while the call is still running
          too: a delegation that looks stuck is the one worth looking into. */}
      {step.transcript && onOpenDelegate && (
        <button
          className="run-row-delegate"
          onClick={() => step.transcript && onOpenDelegate(step.transcript)}
        >
          Read what this {step.transcript.agent} did →
        </button>
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
