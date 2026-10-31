import { useEffect, useMemo, useRef, useState } from "react";

import { currentTool, stateOf, type AgentRun, type AgentState } from "../lib/agents";
import { reportLabel } from "../lib/delegate";
import { clip, duration, plural } from "../lib/format";
import { useNow } from "../lib/useNow";
import { useStore, type Entry } from "../state/store";
import { DrawerHead } from "./Drawer";
import { Markdown } from "./Markdown";

type ToolEntry = Extract<Entry, { kind: "tool" }>;

/** The delegation tool. A call to it is an agent in this panel. */
const SPAWN_TOOL = "spawn_subagent";

/**
 * Every agent in this conversation, as a tree: the conversation's own agent
 * at the root of each question, and the delegates it started under it.
 *
 * The transcript keeps a delegation as one call and its report, which is the
 * point of delegating, and which left no way to see what a delegate was doing
 * while it did it. This is that view. It sits beside the conversation in the
 * column Changes and the canvas share, so the question and the work answering
 * it are on screen together.
 *
 * Each delegate shows how it stands, its round, its clock, what it cost, and
 * the call it's in. Opened up, it shows every call with how it came out, what
 * it's writing, and its reasoning, streamed. Delegates that ran side by side
 * are drawn on a shared timeline under each question, so parallel work looks
 * parallel.
 *
 * Built from two sources that can each be partial: the transcript's
 * delegation calls (always there, including on a reopened conversation) and
 * the live runs the store collected (only for what this window watched).
 */
export function AgentsPanel(props: {
  model: string | null;
  busy: boolean;
  focus: string | null;
  onClose: () => void;
  onOpenTranscript: (transcript: { session: string; agent: string }) => void;
}) {
  // Subscribed here rather than handed down: these move with every streamed
  // word, and `App` deliberately doesn't redraw for that.
  const entries = useStore((s) => s.entries);
  const agents = useStore((s) => s.agents);
  const session = useStore((s) => s.session?.id);
  const restore = useStore((s) => s.restoreAgents);
  // A reopened conversation's delegates, read back from their transcripts
  // when the panel is opened rather than on every reopen: most conversations
  // are reopened to read the answer, not to watch the work behind it.
  useEffect(() => {
    if (session) void restore();
  }, [session, restore]);
  return <AgentsView {...props} entries={entries} agents={agents} />;
}

/** The panel, given what it draws. Apart from the store, for its tests. */
export function AgentsView({
  entries,
  agents,
  model,
  busy,
  focus,
  onClose,
  onOpenTranscript,
}: {
  entries: Entry[];
  agents: Readonly<Record<string, AgentRun>>;
  /** The conversation's own model, for the root of each question. */
  model: string | null;
  /** Whether the conversation's turn is running. */
  busy: boolean;
  /** A delegation to open on, by its call id. */
  focus: string | null;
  onClose: () => void;
  onOpenTranscript: (transcript: { session: string; agent: string }) => void;
}) {
  const groups = useMemo(() => questions(entries), [entries]);
  const [open, setOpen] = useState<string | null>(focus);
  // A "Watch" from a card opens that delegate, even with the panel already up.
  useEffect(() => {
    if (focus) setOpen(focus);
  }, [focus]);

  // Escape closes it, as it closes Changes beside it. Not from a field,
  // where Escape is how someone leaves the field.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const target = e.target as HTMLElement | null;
      if (target?.closest?.("input, textarea, select, [contenteditable='true']")) return;
      onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const total = groups.reduce((n, g) => n + g.calls.length, 0);
  const running = groups.some((g) =>
    g.calls.some((call) => isRunning(call, agents[call.id])),
  );
  const now = useNow(running || busy);

  return (
    <aside className="drawer agents-pane" aria-label="Agent activity">
      {/* Not "Agents": that's the rail's drawer of agent definitions. This is
          what they're doing. */}
      <DrawerHead title="Agent activity" onClose={onClose} />
      <p className="drawer-intro">
        Every agent in this conversation. Each question&apos;s own agent is at the
        top, with the delegates it started under it, drawn against the same
        clock so work that ran side by side lines up.
      </p>

      {total === 0 && (
        <p className="drawer-empty">
          Nothing has been delegated in this conversation yet. When Taurus hands
          work to a sub-agent, it shows up here while it runs.
        </p>
      )}

      {groups
        .filter((g) => g.calls.length > 0)
        .map((group, index, shown) => {
          const latest = index === shown.length - 1;
          const span = timeline(group.calls, agents, now);
          return (
            <section key={group.id} className="section agents-question">
              <span className="micro agents-asked" title={group.text}>
                {group.text || "(a question)"}
              </span>
              <div className="agents-root">
                <span className={`dot ${latest && busy ? "warn" : "ok"}`} aria-hidden="true" />
                <span className="agents-name">Conversation</span>
                <span className="agents-detail">
                  {[model, latest && busy ? "working" : "done", plural(group.calls.length, "delegate")]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
              </div>
              <ul className="agents-tree">
                {group.calls.map((call) => (
                  <AgentNode
                    key={call.id}
                    call={call}
                    run={agents[call.id]}
                    now={now}
                    span={span}
                    open={open === call.id}
                    onToggle={() => setOpen(open === call.id ? null : call.id)}
                    onOpenTranscript={onOpenTranscript}
                  />
                ))}
              </ul>
            </section>
          );
        })}
    </aside>
  );
}

/** One question and the delegations made while answering it. */
type Question = { id: string; text: string; calls: ToolEntry[] };

/** The transcript's delegation calls, under the question each answered. */
export function questions(entries: Entry[]): Question[] {
  const out: Question[] = [];
  let current: Question = { id: "start", text: "", calls: [] };
  for (const entry of entries) {
    if (entry.kind === "user") {
      out.push(current);
      current = { id: entry.id, text: firstLine(entry.text), calls: [] };
    } else if (entry.kind === "tool" && entry.name === SPAWN_TOOL) {
      current.calls.push(entry);
    }
  }
  out.push(current);
  return out;
}

function firstLine(text: string): string {
  const line = text.trim().split("\n")[0] ?? "";
  return line.length > 120 ? `${clip(line, 120)}…` : line;
}

function isRunning(call: ToolEntry, run: AgentRun | undefined): boolean {
  const state = stateOf(run, call.report);
  if (state !== "running" && state !== "queued") return false;
  // A call the transcript has settled is over, whatever this window saw.
  return call.status === "running" || !!call.detached || !!run?.detached;
}

/**
 * How a delegation stands. Its report when it has one; otherwise what this
 * window watched, unless the transcript has since settled the call, which
 * wins: a call that came back without this window seeing its delegate stop
 * is over all the same.
 */
export function standing(call: ToolEntry, run: AgentRun | undefined): AgentState {
  if (isRunning(call, run)) return stateOf(run);
  const state = stateOf(run, call.report);
  if (state === "running" || state === "queued") {
    return call.status === "error" ? "failed" : "done";
  }
  return state;
}

/** The window of time a question's delegates span, for their bars. */
function timeline(
  calls: ToolEntry[],
  agents: Readonly<Record<string, AgentRun>>,
  now: number,
): { from: number; to: number } | null {
  let from = Infinity;
  let to = -Infinity;
  for (const call of calls) {
    const run = agents[call.id];
    const start = run?.queuedAt ?? run?.startedAt;
    if (start === undefined) continue;
    from = Math.min(from, start);
    to = Math.max(to, run?.finishedAt ?? now);
  }
  return Number.isFinite(from) && to > from ? { from, to } : null;
}

const STATE_DOT: Record<AgentState, string> = {
  queued: "",
  running: "warn",
  done: "ok",
  blocked: "accent",
  failed: "error",
  stopped: "",
};

const STATE_WORD: Record<AgentState, string> = {
  queued: "waiting for a slot",
  running: "running",
  done: "done",
  blocked: "blocked",
  failed: "failed",
  stopped: "stopped",
};

function AgentNode({
  call,
  run,
  now,
  span,
  open,
  onToggle,
  onOpenTranscript,
}: {
  call: ToolEntry;
  run: AgentRun | undefined;
  now: number;
  span: { from: number; to: number } | null;
  open: boolean;
  onToggle: () => void;
  onOpenTranscript: (transcript: { session: string; agent: string }) => void;
}) {
  const node = useRef<HTMLLIElement>(null);
  useEffect(() => {
    if (open) node.current?.scrollIntoView?.({ block: "nearest" });
  }, [open]);

  const report = call.report ?? run?.report;
  const state = standing(call, run);
  const agent = run?.agent ?? call.transcript?.agent ?? agentFrom(call.preview);
  const started = run?.startedAt;
  const ended = run?.finishedAt;
  const elapsed = started ? (ended ?? now) - started : null;
  const doing = state === "running" ? currentTool(run) : undefined;
  const transcript = call.transcript ?? (run?.session && agent ? { session: run.session, agent } : undefined);

  // The numbers first and the model last. The model is usually the same on
  // every row, and the numbers are what somebody opened this to read: led by
  // the model, a narrow panel cut off the time, the calls and the tokens.
  const facts = [
    elapsed !== null ? duration(elapsed) : null,
    run && run.round > 0
      ? `round trip ${run.round}${run.maxRounds ? ` of ${run.maxRounds}` : ""}`
      : null,
    run?.tools.length ? plural(run.tools.length, "call") : null,
    run?.usage ? `${short(run.usage.input + run.usage.output)} tokens` : null,
    run?.detached ? "background" : null,
    run?.model,
  ].filter(Boolean);

  return (
    <li ref={node} className={`agents-node${open ? " open" : ""}`} data-state={state}>
      <button className="agents-head" aria-expanded={open} onClick={onToggle}>
        <span className={`dot ${STATE_DOT[state]}`} aria-hidden="true" />
        <span className="agents-name">{agent}</span>
        <span className="agents-state">
          {report ? reportLabel(report) : STATE_WORD[state]}
        </span>
        <span className="agents-detail">{facts.join(" · ")}</span>
      </button>
      {span && started !== undefined && (
        <Bar span={span} from={run?.queuedAt ?? started} started={started} to={ended ?? now} state={state} />
      )}
      {doing && <p className="agents-doing">{doing.preview}</p>}
      {!run && !open && (
        <p className="agents-doing">{call.preview}</p>
      )}

      {open && (
        <div className="agents-body">
          <p className="agents-task">{call.preview}</p>
          {report?.needs && (
            <p className="agents-needs">
              Needs {report.owner === "user" ? "you" : "the agent"} to: {report.needs}
            </p>
          )}
          {report?.summary && (
            <div className="agents-report">
              <span className="micro">Report</span>
              <Markdown text={report.summary} streaming={false} keyBy="content" />
            </div>
          )}
          {run?.retrying && <p className="agents-doing warn">{run.retrying}</p>}
          {run?.error && <p className="agents-doing error">{run.error}</p>}
          {run && run.tools.length > 0 && (
            <ol className="agents-calls" aria-label="Its tool calls">
              {run.tools.map((tool) => {
                const mark = tool.ok === undefined ? "running" : tool.ok ? "ok" : "failed";
                return (
                  <li key={tool.call} data-state={mark}>
                    <span className="agents-mark" aria-label={mark}>
                      {mark === "running" ? "…" : mark === "ok" ? "✓" : "✕"}
                    </span>
                    <span className="agents-call">{tool.preview}</span>
                    {tool.startedAt !== undefined && (
                      <span className="agents-took">
                        {duration((tool.endedAt ?? now) - tool.startedAt)}
                      </span>
                    )}
                  </li>
                );
              })}
            </ol>
          )}
          {run?.thinking && <Reasoning text={run.thinking} live={state === "running"} />}
          {run?.text && (
            <div className="agents-said">
              <span className="micro">What it&apos;s saying</span>
              <Markdown text={run.text} streaming={state === "running"} keyBy="content" />
            </div>
          )}
          {!run && (
            <p className="drawer-empty">
              {call.transcript
                ? "Reading what it did from its transcript…"
                : "This window didn't watch it, and no transcript is linked to it, so only its report is here."}
            </p>
          )}
          {run?.restored && (
            <p className="agents-context">
              Read back from its transcript. How long each call took and what it
              spent weren&apos;t recorded there.
            </p>
          )}
          {run?.context && run.context.window > 0 && (
            <p className="agents-context">
              Context {Math.round((run.context.used / run.context.window) * 100)}% of{" "}
              {short(run.context.window)}
            </p>
          )}
          {transcript && (
            <button className="run-row-delegate" onClick={() => onOpenTranscript(transcript)}>
              Read its whole transcript →
            </button>
          )}
        </div>
      )}
    </li>
  );
}

/** Its reasoning this round, folded: it's long, and it's how, not what. */
function Reasoning({ text, live }: { text: string; live: boolean }) {
  const [shown, setShown] = useState(false);
  return (
    <div className="agents-reasoning">
      <button className="quiet" aria-expanded={shown} onClick={() => setShown(!shown)}>
        {shown ? "Hide its reasoning" : live ? "Show its reasoning, as it thinks" : "Show its reasoning"}
      </button>
      {shown && <pre className="thinking-body">{text}</pre>}
    </div>
  );
}

/**
 * Where on the question's clock this delegate ran: a faint stretch while it
 * waited for a slot, then a solid one. Two delegates that ran side by side
 * have bars that overlap.
 */
function Bar({
  span,
  from,
  started,
  to,
  state,
}: {
  span: { from: number; to: number };
  from: number;
  started: number;
  to: number;
  state: AgentState;
}) {
  const width = span.to - span.from;
  const at = (t: number) => `${((t - span.from) / width) * 100}%`;
  return (
    <div className="agents-bar" aria-hidden="true">
      {started > from && (
        <span
          className="agents-bar-wait"
          style={{ left: at(from), width: `${((started - from) / width) * 100}%` }}
        />
      )}
      <span
        className="agents-bar-run"
        data-state={state}
        style={{ left: at(started), width: `${Math.max(0.5, ((to - started) / width) * 100)}%` }}
      />
    </div>
  );
}

/** "Delegate to explorer: …" → "explorer", for a call this window didn't watch. */
function agentFrom(preview: string): string {
  const match = /^Delegate to ([^:]+):/.exec(preview);
  return match?.[1] ?? "delegate";
}

function short(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(tokens >= 10_000 ? 0 : 1)}k`;
  return String(tokens);
}
