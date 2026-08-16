/**
 * Transcript state.
 *
 * The store's job is turning the ordered `UiEvent` stream into a list of
 * renderable entries. Streaming text appends to the open assistant entry;
 * a tool call opens its own entry and is completed in place when its result
 * arrives, so the UI never has to correlate anything itself.
 */
import { create } from "zustand";

import * as api from "../lib/api";
import type {
  Answer,
  AppStatus,
  CreatedSession,
  Message,
  PermissionDecision,
  PermissionRequest,
  SessionMeta,
  AgentProposal,
  SkillProposal,
  TranscriptView,
  UiEvent,
} from "../lib/api";

export type Entry =
  | { kind: "user"; id: string; text: string }
  | { kind: "assistant"; id: string; text: string; thinking: string; open: boolean }
  | {
      kind: "tool";
      id: string;
      name: string;
      preview: string;
      status: "running" | "ok" | "error";
      output?: string;
      /**
       * What a still-running tool has reported doing.
       *
       * Two kinds of thing arrive here. Delegation reports labels — one line
       * per step it takes. A command reports its own output, in batches, as it
       * is produced. Both are progress; they differ only in how they are drawn.
       * For every other tool the array stays empty and nothing is drawn.
       */
      steps: string[];
      /**
       * Drawn in place of the call's row, for the three tools whose output is
       * something to look at. Set the moment the call is announced, so a
       * question card is on screen while the call it belongs to is waiting.
       *
       * Dropped again if the call fails, in both the live and the resumed
       * path: a table the harness refused to accept must not be left on
       * screen looking like an answer.
       */
      view?: TranscriptView;
      /**
       * Wall-clock bounds of the call, in epoch milliseconds, so a run of
       * steps can report how long it took. Absent on a resumed conversation:
       * the transcript on disk records what happened, not when, and a made-up
       * duration is worse than none.
       */
      startedAt?: number;
      endedAt?: number;
    }
  | {
      kind: "notice";
      id: string;
      text: string;
      tone: "info" | "error";
      /**
       * Renders as a labelled hairline across the transcript instead of a
       * block of prose. For the things that happened *to* the conversation
       * rather than in it — compaction being the only one so far.
       */
      rule?: { label: string; note: string };
    };

interface Store {
  status: AppStatus | null;
  session: CreatedSession | null;
  /** This workspace's saved conversations, newest first. Drives the rail. */
  sessions: SessionMeta[];
  entries: Entry[];
  /**
   * Workspace-relative paths this conversation has changed. Counted in the
   * header, listed turn by turn in the Changes drawer.
   */
  changed: string[];
  busy: boolean;
  /** Set while a turn is running so the composer can show Stop instead of Send. */
  permission: PermissionRequest | null;
  proposals: SkillProposal[];
  agentProposals: AgentProposal[];
  error: string | null;

  init: () => Promise<void>;
  startSession: (providerId: string, model: string) => Promise<void>;
  /** Reopens a saved conversation and redraws it. */
  resume: (sessionId: string) => Promise<void>;
  /**
   * Erases a saved conversation. Deleting the open one starts a replacement on
   * the same provider and model, so the app is never left without a session.
   */
  remove: (sessionId: string) => Promise<void>;
  send: (text: string) => Promise<void>;
  stop: () => Promise<void>;
  answerPermission: (decision: PermissionDecision) => Promise<void>;
  /**
   * Releases the tool call parked behind a question card. One answer per
   * question, in order; an empty one is a skip, which every question allows.
   */
  answerQuestions: (id: string, answers: Answer[]) => Promise<void>;
  resolveProposal: (
    id: string,
    approve: boolean,
    target?: "project" | "user",
  ) => Promise<void>;
  resolveAgentProposal: (
    id: string,
    approve: boolean,
    target?: "project" | "user",
  ) => Promise<void>;
  setWorkspace: (path: string) => Promise<void>;
  /** Re-reads config-derived state after something on disk changed. */
  refresh: () => Promise<void>;
  /** Re-reads the conversation list and this conversation's changed files. */
  reload: () => Promise<void>;
  dismissError: () => void;
}

let counter = 0;
const nextId = () => `e${++counter}`;

/**
 * Guards `init` against React StrictMode, which runs effects twice in dev.
 * Without it, startup creates two sessions and registers two sets of event
 * listeners, so every permission prompt would arrive duplicated.
 */
let initialized = false;

export const useStore = create<Store>((set, get) => ({
  status: null,
  session: null,
  sessions: [],
  entries: [],
  changed: [],
  busy: false,
  permission: null,
  proposals: [],
  agentProposals: [],
  error: null,

  init: async () => {
    if (initialized) return;
    initialized = true;

    const status = await api.getStatus();
    set({ status });

    api.onPermissionRequest((permission) => set({ permission }));
    api.onSkillProposal((proposal) =>
      set((s) => ({ proposals: [...s.proposals, proposal] })),
    );
    api.onAgentProposal((proposal) =>
      set((s) => ({ agentProposals: [...s.agentProposals, proposal] })),
    );

    // Reopen this workspace's most recent conversation. Failing that — a first
    // run, a deleted transcript, a model that no longer exists — fall through
    // to a fresh session rather than leaving the app with none.
    try {
      const sessions = await api.listSessions();
      set({ sessions });
      if (sessions[0]) {
        await get().resume(sessions[0].id);
        return;
      }
    } catch (e) {
      console.warn("could not restore the last session", e);
    }

    // Restore the previous provider/model when both are still available.
    const { last_provider, last_model } = status.settings;
    const provider =
      status.providers.find((p) => p.id === last_provider) ?? status.providers[0];
    if (!provider) return;
    try {
      const models = await api.listModels(provider.id);
      const model =
        models.find((m) => m.id === last_model)?.id ?? models[0]?.id;
      if (model) await get().startSession(provider.id, model);
    } catch (e) {
      // A provider that is not running is expected on first launch; the user
      // picks one from the header.
      set({ error: String(e) });
    }
  },

  startSession: async (providerId, model) => {
    const session = await api.createSession(providerId, model);
    set({
      session,
      entries: [],
      changed: [],
      error: null,
      proposals: [],
      agentProposals: [],
    });
    void get().reload();
    if (!session.native_tools) {
      set((s) => ({
        entries: [
          ...s.entries,
          {
            kind: "notice",
            id: nextId(),
            tone: "info",
            text: `${model} has no built-in tool calling. Taurus will use prompted tool calls instead — this works, but expect the occasional retry.`,
          },
        ],
      }));
    }
  },

  resume: async (sessionId) => {
    const { messages, ...session } = await api.resumeSession(sessionId);
    set({
      session,
      entries: entriesFromMessages(messages),
      changed: [],
      error: null,
      proposals: [],
      agentProposals: [],
    });
    void get().reload();
  },

  remove: async (sessionId) => {
    const open = get().session;
    try {
      await api.deleteSession(sessionId);
    } catch (e) {
      // The backend refuses to delete a conversation mid-turn, which is the one
      // failure a user can actually provoke. It belongs in the banner rather
      // than thrown at a click handler in the rail.
      return set({ error: String(e) });
    }

    if (open?.id !== sessionId) return get().reload();

    // The conversation on screen is the one that just went. Replacing it beats
    // leaving the transcript showing a conversation that no longer exists, and
    // its provider and model were both working a moment ago.
    set({
      session: null,
      entries: [],
      changed: [],
      proposals: [],
      agentProposals: [],
    });
    try {
      await get().startSession(open.provider_id, open.model);
    } catch (e) {
      // A provider that has since gone away. The rail, the first-run screen and
      // Settings all work with no session, so this is a message, not a dead end.
      set({ error: String(e) });
    }
    await get().reload();
  },

  send: async (text) => {
    const { session } = get();
    if (!session || !text.trim()) return;

    set((s) => ({
      busy: true,
      error: null,
      entries: [...s.entries, { kind: "user", id: nextId(), text }],
    }));

    try {
      await api.sendMessage(session.id, text, (event) =>
        set((s) => ({ entries: reduce(s.entries, event) })),
      );
    } catch (e) {
      set((s) => ({
        entries: [
          ...s.entries,
          { kind: "notice", id: nextId(), tone: "error", text: String(e) },
        ],
      }));
    } finally {
      set((s) => ({
        busy: false,
        // Close the open assistant entry so the next turn starts a new bubble.
        entries: s.entries.map((e) =>
          e.kind === "assistant" ? { ...e, open: false } : e,
        ),
      }));
      // The turn may have retitled the conversation and almost certainly
      // changed the file list, both of which are on screen.
      void get().reload();
    }
  },

  stop: async () => {
    const { session } = get();
    if (session) await api.cancelSession(session.id);
  },

  answerPermission: async (decision) => {
    const { permission } = get();
    if (!permission) return;
    set({ permission: null });
    await api.respondPermission(permission.id, decision);
  },

  answerQuestions: async (id, answers) => {
    // No optimistic update: the card reads its own state from the call it
    // belongs to, and that call turns from running to done when the harness
    // releases it. Marking it answered here would show it as settled a beat
    // before the turn had actually resumed.
    await api.answerQuestions(id, answers);
  },

  resolveProposal: async (id, approve, target = "project") => {
    await api.respondSkillProposal(id, approve, approve ? target : undefined);
    set((s) => ({
      proposals: s.proposals.filter((p) => p.id !== id),
      entries: [
        ...s.entries,
        {
          kind: "notice",
          id: nextId(),
          tone: "info",
          text: approve ? `Skill saved.` : `Skill discarded.`,
        },
      ],
    }));
    if (approve) await get().refresh();
  },

  resolveAgentProposal: async (id, approve, target = "project") => {
    await api.respondAgentProposal(id, approve, approve ? target : undefined);
    set((s) => ({
      agentProposals: s.agentProposals.filter((p) => p.id !== id),
      entries: [
        ...s.entries,
        {
          kind: "notice",
          id: nextId(),
          tone: "info",
          text: approve ? `Sub-agent saved.` : `Sub-agent discarded.`,
        },
      ],
    }));
    if (approve) await get().refresh();
  },

  setWorkspace: async (path) => {
    await api.setWorkspace(path);
    set({ status: await api.getStatus() });
    await get().reload();
  },

  refresh: async () => set({ status: await api.getStatus() }),

  reload: async () => {
    const { session } = get();
    // Neither list is load-bearing: a rail with a stale entry beats an error
    // banner over a working conversation, so both failures are swallowed.
    try {
      set({ sessions: await api.listSessions() });
    } catch (e) {
      console.warn("could not list conversations", e);
    }
    if (!session) return set({ changed: [] });
    try {
      const turns = await api.listCheckpoints(session.id);
      set({ changed: [...new Set(turns.flatMap((t) => t.files))] });
    } catch (e) {
      console.warn("could not list changed files", e);
    }
  },

  dismissError: () => set({ error: null }),
}));

/**
 * Rebuilds the transcript view from a saved conversation.
 *
 * The saved form is the model's, not the view's: a tool call and the result
 * that answers it are two blocks in two different messages, and the view shows
 * them as one entry. So calls are opened as they are met and completed when
 * their result turns up, which is the same shape the live event reducer
 * produces — a resumed conversation has to be indistinguishable from one that
 * was streamed.
 */
export function entriesFromMessages(messages: Message[]): Entry[] {
  const entries: Entry[] = [];

  for (const message of messages) {
    if (message.role === "user") {
      for (const block of message.content) {
        if (block.type === "text") {
          entries.push({ kind: "user", id: nextId(), text: block.text });
        } else if (block.type === "tool_result") {
          const index = entries.findIndex(
            (e) => e.kind === "tool" && e.id === block.tool_use_id,
          );
          if (index >= 0) {
            const call = entries[index] as Extract<Entry, { kind: "tool" }>;
            entries[index] = {
              ...call,
              status: block.is_error ? "error" : "ok",
              output: block.content,
              // The same rule the live reducer applies: a call the harness
              // refused drew nothing, so a reopened conversation must not
              // show it having drawn something.
              view: block.is_error ? undefined : call.view,
            };
          }
        }
      }
      continue;
    }

    // An assistant turn is one bubble plus one entry per tool it called, in
    // that order — text is always what precedes the call that follows it.
    const text = message.content
      .filter((b) => b.type === "text")
      .map((b) => (b.type === "text" ? b.text : ""))
      .join("");
    const thinking = message.content
      .filter((b) => b.type === "thinking")
      .map((b) => (b.type === "thinking" ? b.text : ""))
      .join("");

    if (text || thinking) {
      entries.push({
        kind: "assistant",
        id: nextId(),
        text,
        thinking,
        open: false,
      });
    }

    for (const block of message.content) {
      if (block.type !== "tool_use") continue;
      entries.push({
        kind: "tool",
        id: block.id,
        name: block.name,
        // The live preview is composed by the tool itself and is not part of
        // the transcript, so the arguments stand in for it.
        preview: preview(block.input),
        // Nothing to replay: progress is live-only, and a transcript records
        // what a call did, not what it said while doing it.
        steps: [],
        view: viewFromCall(block.id, block.name, block.input),
        // Overwritten by the result below. A call left running is one whose
        // result never made it to disk — the turn the process died in.
        status: "running",
      });
    }
  }

  // Applied once at the end rather than per call: the rule is about the whole
  // conversation, and a resumed transcript has every update in it at once.
  return supersedePlans(entries);
}

/**
 * The view a saved call drew, rebuilt from the call itself.
 *
 * A transcript on disk records the model's messages and nothing about how they
 * were drawn, so this is only possible because the three drawing tools take
 * their view payload *as* their input, unchanged — see `taurus_tools::view`,
 * where that identity is the stated reason for the shape. Reopening a
 * conversation therefore redraws a table rather than showing a row saying one
 * was drawn once.
 *
 * The payload is checked rather than trusted. It was written by whichever build
 * of Taurus was running at the time, and a card that throws mid-render takes the
 * whole transcript with it, including the parts that were fine.
 */
export function viewFromCall(
  id: string,
  name: string,
  input: unknown,
): TranscriptView | undefined {
  if (typeof input !== "object" || input === null) return undefined;
  const payload = input as Record<string, unknown>;

  switch (name) {
    case "show_table":
      return typeof payload.title === "string" &&
        Array.isArray(payload.columns) &&
        Array.isArray(payload.rows) &&
        payload.rows.every(Array.isArray)
        ? {
            type: "table",
            title: payload.title,
            caption: asCaption(payload.caption),
            columns: payload.columns,
            rows: payload.rows,
          }
        : undefined;

    case "show_chart":
      return typeof payload.title === "string" &&
        Array.isArray(payload.labels) &&
        Array.isArray(payload.series)
        ? {
            type: "chart",
            title: payload.title,
            caption: asCaption(payload.caption),
            labels: payload.labels,
            series: payload.series,
          }
        : undefined;

    case "ask_user":
      // Keyed to the call, exactly as the live event was — though nothing is
      // waiting on it any more, and the card knows to draw itself read-only
      // from the call's own finished status.
      return Array.isArray(payload.questions)
        ? { type: "questions", id, questions: payload.questions }
        : undefined;

    case "update_plan":
      // Only the last one drawn — see `supersedePlans`, which the callers of
      // this apply once they have the whole conversation.
      return Array.isArray(payload.steps)
        ? { type: "plan", steps: payload.steps }
        : undefined;

    default:
      return undefined;
  }
}

/** `caption` is optional to the model and nullable across the boundary. */
function asCaption(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

/**
 * Leaves only the newest plan card drawn.
 *
 * Every other view is a thing that happened once — a table of the numbers as
 * they were, a question that was asked and answered — so two of them are two
 * facts and both belong on screen. A plan is not that. It is one evolving
 * object, and the model rewrites the whole list every time a step starts or
 * finishes, so a six-step task ends with seven `update_plan` calls. Drawn as
 * seven cards, the checklist that exists to say *where you are* would instead
 * be a history of everywhere you have been, with the current state at the
 * bottom of it.
 *
 * The superseded calls keep their row — they happened, and the run header still
 * counts them — they just stop drawing. Nothing is removed from the transcript.
 */
function supersedePlans(entries: Entry[]): Entry[] {
  // Walked backwards rather than with `findLastIndex`, which needs a newer
  // lib target than this project sets — not worth moving for one call.
  let last = -1;
  for (let i = entries.length - 1; i >= 0; i--) {
    const entry = entries[i];
    if (entry.kind === "tool" && entry.view?.type === "plan") {
      last = i;
      break;
    }
  }
  if (last < 0) return entries;
  return entries.map((e, i) =>
    i !== last && e.kind === "tool" && e.view?.type === "plan"
      ? { ...e, view: undefined }
      : e,
  );
}

const PREVIEW_MAX_CHARS = 120;

function preview(input: unknown): string {
  let text: string;
  try {
    text = JSON.stringify(input) ?? "";
  } catch {
    return "";
  }
  return text.length > PREVIEW_MAX_CHARS
    ? `${text.slice(0, PREVIEW_MAX_CHARS - 1)}…`
    : text;
}

/**
 * How many progress batches one call keeps.
 *
 * A build can emit tens of thousands of lines, and every one of them would
 * otherwise be held in memory and re-rendered on the next. A terminal keeps a
 * scrollback rather than the whole history, for the same reason.
 */
const MAX_SCROLLBACK = 200;

/** Keeps the tail, which for a running command is the part being watched. */
export function trimScrollback(steps: string[]): string[] {
  return steps.length > MAX_SCROLLBACK ? steps.slice(-MAX_SCROLLBACK) : steps;
}

/** Folds one event into the transcript. */
export function reduce(entries: Entry[], event: UiEvent): Entry[] {
  switch (event.type) {
    case "text_delta":
      return appendAssistant(entries, (e) => ({
        ...e,
        text: e.text + event.text,
      }));

    case "thinking_delta":
      return appendAssistant(entries, (e) => ({
        ...e,
        thinking: e.thinking + event.text,
      }));

    case "tool_call_started":
      return supersedePlans([
        // Any streaming text before a tool call is finished text.
        ...entries.map((e) =>
          e.kind === "assistant" ? { ...e, open: false } : e,
        ),
        {
          kind: "tool",
          id: event.id,
          name: event.name,
          preview: event.preview,
          status: "running",
          steps: [],
          view: event.view,
          startedAt: Date.now(),
        },
      ]);

    case "tool_progress":
      return entries.map((e) =>
        e.kind === "tool" && e.id === event.id
          ? { ...e, steps: trimScrollback([...e.steps, event.label]) }
          : e,
      );

    case "tool_call_finished":
      return entries.map((e) =>
        e.kind === "tool" && e.id === event.id
          ? {
              ...e,
              status: event.ok ? "ok" : "error",
              output: event.output,
              // A refused call leaves nothing behind. The view went out before
              // the call ran, so a chart whose series did not line up is on
              // screen by the time the harness says so — and a wrong chart
              // beside the word "failed" is still a wrong chart.
              view: event.ok ? e.view : undefined,
              endedAt: Date.now(),
            }
          : e,
      );

    case "context_trimmed":
      return [
        ...entries,
        {
          kind: "notice",
          id: nextId(),
          tone: "info",
          text: `Shortened ${event.results} older tool results, recovering about ${event.tokens_saved.toLocaleString()} tokens.`,
          rule: {
            label: "Context trimmed",
            note: `~${event.tokens_saved.toLocaleString()} tokens`,
          },
        },
      ];

    case "compacted":
      return [
        ...entries,
        {
          kind: "notice",
          id: nextId(),
          tone: "info",
          text: `Summarized ${event.messages_removed} earlier messages to stay within the context window.`,
          rule: {
            label: "Context compacted",
            note: `${event.messages_removed} messages`,
          },
        },
      ];

    // Informational, not an error: nothing has gone wrong yet. If the retries
    // run out, the failure arrives on its own as `error`.
    case "retrying":
      return [
        ...entries,
        {
          kind: "notice",
          id: nextId(),
          tone: "info",
          text: event.reason,
          rule: {
            label: "Retrying",
            note: `attempt ${event.attempt} of ${event.of}`,
          },
        },
      ];

    case "error":
      return [
        ...entries,
        { kind: "notice", id: nextId(), tone: "error", text: event.message },
      ];

    // Iteration boundaries and the final usage report are not shown per-entry;
    // the header's token counter covers the latter.
    case "iteration_started":
    case "turn_finished":
      return entries;
  }
}

/** Appends to the open assistant entry, opening one if needed. */
function appendAssistant(
  entries: Entry[],
  update: (entry: Extract<Entry, { kind: "assistant" }>) => Entry,
): Entry[] {
  const last = entries[entries.length - 1];
  if (last?.kind === "assistant" && last.open) {
    return [...entries.slice(0, -1), update(last)];
  }
  return [
    ...entries,
    update({ kind: "assistant", id: nextId(), text: "", thinking: "", open: true }),
  ];
}
