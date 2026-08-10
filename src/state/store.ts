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
  AppStatus,
  CreatedSession,
  Message,
  PermissionDecision,
  PermissionRequest,
  SkillProposal,
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
    }
  | { kind: "notice"; id: string; text: string; tone: "info" | "error" };

interface Store {
  status: AppStatus | null;
  session: CreatedSession | null;
  entries: Entry[];
  busy: boolean;
  /** Set while a turn is running so the composer can show Stop instead of Send. */
  permission: PermissionRequest | null;
  proposals: SkillProposal[];
  error: string | null;

  init: () => Promise<void>;
  startSession: (providerId: string, model: string) => Promise<void>;
  /** Reopens a saved conversation and redraws it. */
  resume: (sessionId: string) => Promise<void>;
  send: (text: string) => Promise<void>;
  stop: () => Promise<void>;
  answerPermission: (decision: PermissionDecision) => Promise<void>;
  resolveProposal: (
    id: string,
    approve: boolean,
    target?: "project" | "user",
  ) => Promise<void>;
  setWorkspace: (path: string) => Promise<void>;
  /** Re-reads config-derived state after something on disk changed. */
  refresh: () => Promise<void>;
  clear: () => void;
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
  entries: [],
  busy: false,
  permission: null,
  proposals: [],
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

    // Reopen this workspace's most recent conversation. Failing that — a first
    // run, a deleted transcript, a model that no longer exists — fall through
    // to a fresh session rather than leaving the app with none.
    try {
      const [recent] = await api.listSessions();
      if (recent) {
        await get().resume(recent.id);
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
    set({ session, entries: [], error: null });
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
      error: null,
      proposals: [],
    });
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

  setWorkspace: async (path) => {
    await api.setWorkspace(path);
    set({ status: await api.getStatus() });
  },

  refresh: async () => set({ status: await api.getStatus() }),

  clear: () => set({ entries: [] }),
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
            entries[index] = {
              ...(entries[index] as Extract<Entry, { kind: "tool" }>),
              status: block.is_error ? "error" : "ok",
              output: block.content,
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
        // Overwritten by the result below. A call left running is one whose
        // result never made it to disk — the turn the process died in.
        status: "running",
      });
    }
  }

  return entries;
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
      return [
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
        },
      ];

    case "tool_call_finished":
      return entries.map((e) =>
        e.kind === "tool" && e.id === event.id
          ? { ...e, status: event.ok ? "ok" : "error", output: event.output }
          : e,
      );

    case "compacted":
      return [
        ...entries,
        {
          kind: "notice",
          id: nextId(),
          tone: "info",
          text: `Summarized ${event.messages_removed} earlier messages to stay within the context window.`,
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
