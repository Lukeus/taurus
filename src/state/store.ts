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
  send: (text: string) => Promise<void>;
  stop: () => Promise<void>;
  answerPermission: (
    decision: "allow_once" | "allow_always" | "deny",
  ) => Promise<void>;
  resolveProposal: (
    id: string,
    approve: boolean,
    target?: "project" | "user",
  ) => Promise<void>;
  setWorkspace: (path: string) => Promise<void>;
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
    if (approve) set({ status: await api.getStatus() });
  },

  setWorkspace: async (path) => {
    await api.setWorkspace(path);
    set({ status: await api.getStatus() });
  },

  clear: () => set({ entries: [] }),
  dismissError: () => set({ error: null }),
}));

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
