import type { DelegateReport, DelegateStep, UiEvent } from "./api";

/**
 * What the agents panel knows about one delegation, keyed by the parent's
 * call id.
 *
 * Built from `delegate_activity` and the three events that tie a card to its
 * child. The card in the transcript stays one call and its report; this is
 * the delegate's run as it happens, for the panel that shows every agent in a
 * conversation and for the card's one-line summary of it.
 */
export type AgentRun = {
  /** The parent's call id: which card this is. */
  id: string;
  agent?: string;
  model?: string;
  /** The child's own session id, once its transcript is being written. */
  session?: string;
  /** Unix ms. Set when every slot was taken and it had to wait. */
  queuedAt?: number;
  startedAt?: number;
  finishedAt?: number;
  maxRounds?: number;
  /** The round it's on. Zero before the first. */
  round: number;
  tools: AgentTool[];
  /** Everything it has written, newest last, capped at the tail. */
  text: string;
  /** Its reasoning in the round it's on. */
  thinking: string;
  context?: { used: number; window: number };
  usage?: { input: number; output: number };
  /** Why a request is being tried again, while it is. */
  retrying?: string;
  error?: string;
  /** Its call returned and it went on in the background. */
  detached: boolean;
  report?: DelegateReport;
  /**
   * Rebuilt from the delegate's transcript on a reopened conversation, not
   * watched. Its calls and prose are there; the times of each call and what
   * it spent aren't, because a transcript doesn't record them.
   */
  restored?: boolean;
};

export type AgentTool = {
  /** The delegate's own call id. */
  call: string;
  name: string;
  preview: string;
  /** Absent on a run rebuilt from a transcript, which records no times. */
  startedAt?: number;
  endedAt?: number;
  /** Absent while it runs. */
  ok?: boolean;
};

export type Agents = Readonly<Record<string, AgentRun>>;

/** How much of a delegate's prose is kept: the panel shows the end of it. */
export const MAX_AGENT_TEXT = 24 * 1024;
/** How many of its tool calls are kept, newest last. */
export const MAX_AGENT_TOOLS = 200;

export const NO_AGENTS: Agents = Object.freeze({});

function blank(id: string): AgentRun {
  return { id, round: 0, tools: [], text: "", thinking: "", detached: false };
}

function tail(text: string, max: number): string {
  return text.length > max ? text.slice(text.length - max) : text;
}

/**
 * The agents after one event. Hands back the same object for an event that
 * isn't about a delegation, which is nearly all of them: the panel redraws on
 * identity.
 */
export function mergeAgents(agents: Agents, event: UiEvent): Agents {
  switch (event.type) {
    case "delegate_activity":
      return put(agents, event.id, (run) => apply(run, event.step));
    case "tool_transcript":
      return put(agents, event.id, (run) => ({
        ...run,
        session: event.session,
        agent: run.agent ?? event.agent,
      }));
    case "tool_detached":
      return put(agents, event.id, (run) => ({ ...run, detached: true }));
    case "delegate_report":
      return put(agents, event.id, (run) => ({ ...run, report: event.report }));
    default:
      return agents;
  }
}

function put(agents: Agents, id: string, change: (run: AgentRun) => AgentRun): Agents {
  return { ...agents, [id]: change(agents[id] ?? blank(id)) };
}

function apply(run: AgentRun, step: DelegateStep): AgentRun {
  switch (step.kind) {
    case "queued":
      return { ...run, queuedAt: step.at };
    case "started":
      return {
        ...run,
        agent: step.agent,
        model: step.model,
        maxRounds: step.max_rounds,
        startedAt: step.at,
      };
    case "round":
      return {
        ...run,
        round: step.n,
        thinking: "",
        retrying: undefined,
        // A paragraph break between what it said in one round and the next,
        // so a run of rounds doesn't read as one sentence.
        text: run.text && !run.text.endsWith("\n\n") ? `${run.text}\n\n` : run.text,
      };
    case "text":
      return { ...run, text: tail(run.text + step.text, MAX_AGENT_TEXT) };
    case "thinking":
      return { ...run, thinking: tail(run.thinking + step.text, MAX_AGENT_TEXT) };
    case "tool_started": {
      const tools = [
        ...run.tools,
        { call: step.call, name: step.name, preview: step.preview, startedAt: step.at },
      ];
      return {
        ...run,
        tools: tools.length > MAX_AGENT_TOOLS ? tools.slice(-MAX_AGENT_TOOLS) : tools,
      };
    }
    case "tool_finished":
      return {
        ...run,
        tools: run.tools.map((tool) =>
          tool.call === step.call ? { ...tool, ok: step.ok, endedAt: step.at } : tool,
        ),
      };
    case "context":
      return { ...run, context: { used: step.used, window: step.window } };
    case "retrying":
      return { ...run, retrying: `Attempt ${step.attempt} of ${step.of}: ${step.reason}` };
    case "error":
      return { ...run, error: step.message };
    case "finished":
      return {
        ...run,
        finishedAt: step.at,
        retrying: undefined,
        usage: { input: step.input_tokens, output: step.output_tokens },
      };
  }
}

/** How a delegation stands, for the dot and the word beside it. */
export type AgentState = "queued" | "running" | "done" | "blocked" | "failed" | "stopped";

export function stateOf(run: AgentRun | undefined, report?: DelegateReport): AgentState {
  const settled = report ?? run?.report;
  if (settled) {
    switch (settled.disposition) {
      case "done":
        return "done";
      case "blocked":
        return "blocked";
      case "cancelled":
        return "stopped";
      default:
        return "failed";
    }
  }
  if (run?.finishedAt) return run.error ? "failed" : "done";
  if (run?.queuedAt && !run.startedAt) return "queued";
  return "running";
}

/** The tool call it's in the middle of, if any. */
export function currentTool(run: AgentRun | undefined): AgentTool | undefined {
  if (!run) return undefined;
  for (let i = run.tools.length - 1; i >= 0; i--) {
    if (run.tools[i].ok === undefined) return run.tools[i];
  }
  return undefined;
}
