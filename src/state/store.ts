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
  Attachment,
  FlowEdge,
  FlowNode,
  FlowStage,
  SequenceMessage,
  SkillProposal,
  Step,
  TranscriptView,
  UiEvent,
} from "../lib/api";

export type Entry =
  | {
      kind: "user";
      id: string;
      text: string;
      /**
       * Images sent with this message, base64, in the order they were attached.
       *
       * Held for the live view only. A resumed conversation rebuilds them from
       * the transcript's own `image` blocks, which is the same data by a
       * different route — see `entriesFromMessages`.
       */
      images?: Attachment[];
    }
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
       * The delegate's own conversation, for the calls that had one.
       *
       * Set while the call is still running, which is the point: a delegation
       * that hung, or was stopped, is the one somebody wants to read, and a
       * reference that only arrived with the result could not offer either.
       * Absent on a resumed conversation — the parent's transcript records the
       * delegation, not where its child was written.
       */
      transcript?: { session: string; agent: string };
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
  send: (text: string, images?: Attachment[]) => Promise<void>;
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
  /**
   * Moves the whole app to another folder.
   *
   * Not a setting applied to the conversation on screen: a session's transcript
   * is written under the workspace it was started in and its checkpoints are
   * keyed by it, so the conversation is closed and this workspace's own is
   * opened in its place. See `adoptWorkspace`.
   */
  setWorkspace: (path: string) => Promise<void>;
  /**
   * Opens whatever this workspace has — its most recent conversation, or a
   * fresh one on the provider and model it was last worked in.
   *
   * Shared by startup and the workspace switch, which have to agree: a folder
   * opened from the picker must land in the same state as one the app was
   * launched into.
   */
  adoptWorkspace: () => Promise<void>;
  /** Re-reads config-derived state after something on disk changed. */
  refresh: () => Promise<void>;
  /** Re-reads the conversation list and this conversation's changed files. */
  reload: () => Promise<void>;
  dismissError: () => void;
}

let counter = 0;
const nextId = () => `e${++counter}`;

/**
 * Lets go of a conversation the view has moved on from.
 *
 * The conversation is not deleted: it is on disk and reopens from the rail.
 * This releases the backend's live copy, which is otherwise held — every
 * message, and every image ever attached to one — for the life of the process,
 * with nothing to reach it by once the UI has stopped pointing at it.
 *
 * `replacement` is the session taking its place, so re-opening the conversation
 * that is already open does not close the thing it just returned.
 *
 * Never throws. Failing to let go of something is not worth an error banner
 * over the conversation that succeeded.
 */
async function release(session: CreatedSession | null, replacement?: string) {
  if (!session || session.id === replacement) return;
  try {
    await api.closeSession(session.id);
  } catch (e) {
    console.warn("could not close the previous conversation", e);
  }
}

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

    await get().adoptWorkspace();
  },

  adoptWorkspace: async () => {
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
      console.warn("could not open this workspace's last conversation", e);
    }

    // The provider and model this workspace was last worked in, when both are
    // still available. Read from the store rather than captured, because a
    // workspace switch has just replaced them: settings are resolved per
    // workspace, so the folder being opened may name a different pair.
    const status = get().status;
    if (!status) return;
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
    const previous = get().session;
    const session = await api.createSession(providerId, model);
    // Released only once the replacement is in hand, so a provider that has
    // gone away leaves the conversation on screen rather than closing it in
    // exchange for nothing.
    await release(previous, session.id);
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
    const previous = get().session;
    const { messages, ...session } = await api.resumeSession(sessionId);
    // As in `startSession`: a resume that fails must leave the conversation on
    // screen exactly as it was.
    await release(previous, session.id);
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

  send: async (text, images = []) => {
    const { session } = get();
    // Text is still required with an image attached. "What is wrong with this?"
    // is a question; a bare screenshot is a guess about what was wanted.
    if (!session || !text.trim()) return;

    set((s) => ({
      busy: true,
      error: null,
      entries: [...s.entries, { kind: "user", id: nextId(), text, images }],
    }));

    try {
      await api.sendMessage(
        session.id,
        text,
        (event) => set((s) => ({ entries: reduce(s.entries, event) })),
        images,
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
    // Refused rather than queued. The switch reconnects every MCP server, so
    // the tools a running turn is holding would start failing mid-call — and
    // the turn would go on editing the folder being left while the app claimed
    // to be in the new one.
    if (get().busy) {
      return set({
        error:
          "Taurus is in the middle of a turn. Stop it before switching workspace.",
      });
    }

    // Everything on screen belongs to the folder being left, and none of it
    // survives the move: the transcript is written under the old workspace's
    // directory, the changed-file list is read from checkpoints keyed by it,
    // and a permission prompt or a proposal is about work in it. Cleared
    // before the switch rather than after, so nothing that follows can address
    // a conversation the backend has stopped answering for.
    const previous = get().session;
    set({
      session: null,
      sessions: [],
      entries: [],
      changed: [],
      permission: null,
      proposals: [],
      agentProposals: [],
      error: null,
    });
    await release(previous);

    await api.setWorkspace(path);
    set({ status: await api.getStatus() });
    await get().adoptWorkspace();
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
      // Images precede the text they belong to, so they are collected first and
      // attached to the bubble that follows. A resumed conversation therefore
      // shows the screenshot that was asked about, not a message referring to
      // one that is no longer on screen.
      const attached: Attachment[] = message.content
        .filter((b) => b.type === "image")
        .map((b) =>
          b.type === "image" ? { mime_type: b.mime_type, data: b.data } : null!,
        );

      for (const block of message.content) {
        if (block.type === "text") {
          entries.push({
            kind: "user",
            id: nextId(),
            text: block.text,
            // On the first text block only. A user message has one, but a
            // hand-written transcript could have two, and repeating the images
            // under each would double them.
            images: attached.length > 0 ? attached.splice(0) : undefined,
          });
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

    case "show_sequence":
      // Arrows naming a lane that was never declared are dropped rather than
      // failing the diagram, which is the one place this is looser than the
      // Rust check that refuses them outright. By the time a transcript is
      // being replayed the call has already succeeded, so a payload that fails
      // here came from a build whose rules differed — and a diagram missing one
      // arrow is worth more than a blank where the answer was.
      return Array.isArray(payload.participants) &&
        payload.participants.every((p: unknown) => typeof p === "string") &&
        payload.participants.length > 0 &&
        Array.isArray(payload.messages)
        ? {
            type: "sequence",
            title: typeof payload.title === "string" ? payload.title : "",
            caption: asCaption(payload.caption),
            participants: payload.participants,
            messages: payload.messages
              .map(asMessage)
              .filter(isMessage)
              .filter(
                (m: SequenceMessage) =>
                  (payload.participants as string[]).includes(m.from) &&
                  (payload.participants as string[]).includes(m.to),
              ),
          }
        : undefined;

    case "show_flow": {
      if (!Array.isArray(payload.stages) || !Array.isArray(payload.edges)) {
        return undefined;
      }
      const stages = payload.stages.map(asStage).filter(isStage);
      if (stages.length !== payload.stages.length || stages.length === 0) {
        return undefined;
      }
      // Same rule the sequence diagram replays under: an edge naming a node
      // that is not there is dropped rather than failing the whole card. The
      // Rust check refuses these on the way in, so one here came from a build
      // whose rules differed.
      const labels = new Set(
        stages.flatMap((stage) => stage.nodes.map((node) => node.label)),
      );
      return {
        type: "flow",
        title: typeof payload.title === "string" ? payload.title : "",
        caption: asCaption(payload.caption),
        stages,
        edges: payload.edges
          .map(asEdge)
          .filter(isEdge)
          .filter((e: FlowEdge) => labels.has(e.from) && labels.has(e.to)),
      };
    }

    case "ask_user":
      // Keyed to the call, exactly as the live event was — though nothing is
      // waiting on it any more, and the card knows to draw itself read-only
      // from the call's own finished status.
      return Array.isArray(payload.questions)
        ? { type: "questions", id, questions: payload.questions }
        : undefined;

    case "update_plan": {
      // Only the last one drawn — see `supersedePlans`, which the callers of
      // this apply once they have the whole conversation.
      if (!Array.isArray(payload.steps)) return undefined;
      const steps = payload.steps.map(asStep).filter(isStep);
      return steps.length === payload.steps.length
        ? { type: "plan", steps }
        : undefined;
    }

    default:
      return undefined;
  }
}

/** `caption` is optional to the model and nullable across the boundary. */
function asCaption(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

/**
 * One saved arrow in the shape the card reads, or `undefined` if it is not one.
 *
 * `kind` defaults the way Rust defaults it — an omitted kind is a call — so a
 * transcript written by a model that never set the field replays as the
 * diagram it drew rather than as one with no arrows in it.
 */
function asMessage(value: unknown): SequenceMessage | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const message = value as Record<string, unknown>;
  if (
    typeof message.from !== "string" ||
    typeof message.to !== "string" ||
    typeof message.text !== "string"
  ) {
    return undefined;
  }
  return {
    from: message.from,
    to: message.to,
    text: message.text,
    kind: message.kind === "return" ? "return" : "call",
  };
}

function isMessage(message: SequenceMessage | undefined): message is SequenceMessage {
  return message !== undefined;
}

/** One saved stage of a flow diagram, or `undefined` if it is not one. */
function asStage(value: unknown): FlowStage | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const stage = value as Record<string, unknown>;
  if (!Array.isArray(stage.nodes)) return undefined;
  const nodes = stage.nodes.map(asNode).filter(isNode);
  // A stage that lost a node would draw a column with a gap where a box was,
  // and every arrow into it would be missing too — so the whole card goes back
  // rather than a diagram that is quietly wrong about the shape.
  if (nodes.length !== stage.nodes.length || nodes.length === 0) return undefined;
  return {
    name: typeof stage.name === "string" ? stage.name : null,
    nodes,
  };
}

function isStage(stage: FlowStage | undefined): stage is FlowStage {
  return stage !== undefined;
}

function asNode(value: unknown): FlowNode | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const node = value as Record<string, unknown>;
  if (typeof node.label !== "string" || !node.label.trim()) return undefined;
  return {
    label: node.label,
    note: typeof node.note === "string" ? node.note : null,
  };
}

function isNode(node: FlowNode | undefined): node is FlowNode {
  return node !== undefined;
}

function asEdge(value: unknown): FlowEdge | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const edge = value as Record<string, unknown>;
  if (typeof edge.from !== "string" || typeof edge.to !== "string") return undefined;
  return {
    from: edge.from,
    to: edge.to,
    label: typeof edge.label === "string" ? edge.label : null,
  };
}

function isEdge(edge: FlowEdge | undefined): edge is FlowEdge {
  return edge !== undefined;
}

/**
 * The field names a model reaches for when it means `text`, `state`, and
 * `active_form`, in the same order Rust tries them.
 *
 * This is the half of `Step::from_json` that replay needs, and it has to exist
 * because of *where* the two paths get their steps. A live call is drawn from
 * the view the tool built, which is a `Step` — normalized, because Rust already
 * read it. A reopened conversation is drawn from the raw arguments the model
 * sent, which is whatever it typed: `{"content": "...", "status": "active"}` is
 * a plan Rust accepts and this file, reading `.text` and `.state`, would render
 * as three blank rows and a progress bar stuck at zero.
 *
 * Kept deliberately as a copy rather than generated, since the cost of the two
 * drifting is a plan card that looks broken rather than one that fails — see
 * `taurus_tools::view`, which is the list to keep this in step with.
 */
const TEXT_KEYS = [
  "text",
  "step",
  "task",
  "title",
  "name",
  "description",
  "content",
];
const STATE_KEYS = ["state", "status"];
const ACTIVE_FORM_KEYS = ["active_form", "activeForm", "active_text"];

/** The aliases each state answers to, flattened into what it means. */
const STATE_ALIASES: Record<string, Step["state"]> = {
  todo: "todo",
  pending: "todo",
  not_started: "todo",
  open: "todo",
  waiting: "todo",
  active: "active",
  in_progress: "active",
  "in-progress": "active",
  doing: "active",
  current: "active",
  running: "active",
  done: "done",
  completed: "done",
  complete: "done",
  finished: "done",
};

/**
 * One saved step in the shape the panel reads, or `undefined` if it is not one.
 *
 * A step with no usable text is the only thing rejected. An unreadable *state*
 * is not: it can only have come from a build whose alias list differed from
 * this one, and reading it as `todo` — the same default an absent state gets —
 * costs one wrong word on one row, where blanking the card costs the whole
 * plan.
 */
function asStep(value: unknown): Step | undefined {
  if (typeof value === "string") {
    return value.trim() ? { text: value, state: "todo" } : undefined;
  }
  if (typeof value !== "object" || value === null) return undefined;
  const step = value as Record<string, unknown>;

  const text = TEXT_KEYS.map((key) => step[key]).find(
    (v) => typeof v === "string" && v.trim(),
  );
  if (typeof text !== "string") return undefined;

  const state = STATE_KEYS.map((key) => step[key]).find(
    (v) => typeof v === "string",
  );
  const activeForm = ACTIVE_FORM_KEYS.map((key) => step[key]).find(
    (v) => typeof v === "string" && v.trim(),
  );

  return {
    text,
    state: (typeof state === "string" && STATE_ALIASES[state]) || "todo",
    ...(typeof activeForm === "string" ? { active_form: activeForm } : {}),
  };
}

function isStep(step: Step | undefined): step is Step {
  return step !== undefined;
}

export type PlanView = Extract<TranscriptView, { type: "plan" }>;

/**
 * The checklist to pin above the composer, or `null` when there is none.
 *
 * Derived rather than held, so it is right by construction on every route into
 * the transcript — a live turn, a resumed conversation, a rewind — instead of
 * being a second copy of the plan that each of those has to remember to update.
 *
 * A finished plan stays up until the user asks for something else. That is the
 * one rule here worth stating: it is what makes "done" a thing you get to see,
 * while stopping last hour's completed checklist from sitting over an unrelated
 * question. An unfinished one keeps showing regardless — work left undone is
 * exactly what a pinned panel is for.
 */
export function pinnedPlan(entries: Entry[]): PlanView | null {
  let plan: PlanView | null = null;
  let at = -1;
  for (let i = entries.length - 1; i >= 0; i--) {
    const entry = entries[i];
    if (entry.kind === "tool" && entry.view?.type === "plan") {
      plan = entry.view;
      at = i;
      break;
    }
  }
  // An empty plan is legal on the wire and says nothing worth a panel — and
  // every proportion drawn from it would be a division by zero.
  if (!plan || plan.steps.length === 0) return null;

  if (!plan.steps.every((step) => step.state === "done")) return plan;
  return entries.some((e, i) => i > at && e.kind === "user") ? null : plan;
}

/**
 * Leaves only the newest plan's steps in the transcript.
 *
 * Every other view is a thing that happened once — a table of the numbers as
 * they were, a question that was asked and answered — so two of them are two
 * facts and both belong on screen. A plan is not that. It is one evolving
 * object, and the model rewrites the whole list every time a step starts or
 * finishes, so a six-step task ends with seven `update_plan` calls.
 *
 * Nothing draws them in the transcript any more — the newest is pinned above
 * the composer by `pinnedPlan`, which reads the view left here. Dropping the
 * superseded ones is still what makes that read a single unambiguous answer.
 * Their rows stay either way: they happened, and the run header still counts
 * them.
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

    case "tool_transcript":
      return entries.map((e) =>
        e.kind === "tool" && e.id === event.id
          ? { ...e, transcript: { session: event.session, agent: event.agent } }
          : e,
      );

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
