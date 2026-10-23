/**
 * The composer's mic, as one state machine.
 *
 * Its own store rather than part of the transcript's, because none of it
 * belongs to a conversation: there is one microphone, one model download and
 * one Settings tab, and the composer that asked is keyed per conversation and
 * comes and goes. What it heard is handed over as `heard`, and whichever
 * composer is on screen takes it.
 *
 * The phases, in the order a first press walks them:
 *
 * - **offer** — the model isn't downloaded, so the press asks first. Nothing
 *   is fetched without a yes, because it's 60 MB from a server somebody
 *   didn't pick.
 * - **opening** — the microphone is being opened. Usually instant; on macOS
 *   the first time it's the permission dialog.
 * - **listening** — recording. Ends with a second press, a release after a
 *   hold, ⌘⇧D, or Escape to throw it away.
 * - **transcribing** — whisper.cpp is running on what was said.
 */
import { create } from "zustand";

import * as api from "../lib/api";
import type { VoiceModel, VoiceStatus } from "../lib/api";

export type Phase =
  | { kind: "idle" }
  | { kind: "offer"; model: VoiceModel }
  | { kind: "opening" }
  | { kind: "listening"; since: number }
  | { kind: "transcribing" };

export type Download = {
  model: string;
  done: number;
  total: number;
  /** Whether the composer started it, which is who to tell when it's done. */
  fromMic: boolean;
};

export type Notice = { text: string; tone: "error" | "info" };

/** The longest dictation, matching `taurus_voice::capture::MAX_SECONDS`. */
export const MAX_SECONDS = 300;

/** What a cancel comes back as from the backend. Not worth a notice. */
const CANCELED = "Canceled.";

type Voice = {
  phase: Phase;
  /** The input level while listening, 0–1. */
  level: number;
  status: VoiceStatus | null;
  download: Download | null;
  notice: Notice | null;
  /**
   * The last thing heard, for the composer to insert. An object so that
   * saying the same words twice is two events.
   */
  heard: { text: string } | null;

  refresh: () => Promise<VoiceStatus | null>;
  /** The mic button, a tap of ⌘⇧D: starts when idle, finishes when listening. */
  press: () => Promise<void>;
  finish: () => Promise<void>;
  /** Escape: closes the microphone and drops what it heard. */
  cancel: () => void;
  startDownload: (model: string, fromMic: boolean) => Promise<void>;
  stopDownload: () => void;
  /** "Not now" on the offer. */
  decline: () => void;
  dismiss: () => void;
};

/**
 * Counts presses, so a start that resolves after Escape was pressed knows it
 * has been overtaken. Opening can take as long as a permission dialog stays
 * up, and the mic must not turn itself on when somebody answers it after
 * already giving up.
 */
let attempt = 0;

export const useVoice = create<Voice>((set, get) => ({
  phase: { kind: "idle" },
  level: 0,
  status: null,
  download: null,
  notice: null,
  heard: null,

  refresh: async () => {
    try {
      const status = await api.voiceStatus();
      set({ status });
      return status;
    } catch (e) {
      set({ notice: { text: String(e), tone: "error" } });
      return null;
    }
  },

  press: async () => {
    const { phase } = get();
    if (phase.kind === "listening") return get().finish();
    if (phase.kind !== "idle") return;

    const mine = ++attempt;
    set({ notice: null });
    const status = await get().refresh();
    if (!status || mine !== attempt) return;
    const chosen = status.models.find((m) => m.id === status.model);
    if (!chosen) return;
    if (!chosen.installed) {
      // A download already running is the answer to this press, not a
      // second question.
      if (!get().download) set({ phase: { kind: "offer", model: chosen } });
      return;
    }

    set({ phase: { kind: "opening" }, level: 0 });
    try {
      await api.startListening((level) => {
        if (mine === attempt) set({ level });
      });
    } catch (e) {
      if (mine === attempt) {
        set({ phase: { kind: "idle" }, notice: { text: String(e), tone: "error" } });
      }
      return;
    }
    if (mine !== attempt) {
      // Escape arrived while it was opening.
      void api.cancelListening();
      return;
    }
    set({ phase: { kind: "listening", since: Date.now() } });
  },

  finish: async () => {
    if (get().phase.kind !== "listening") return;
    const mine = attempt;
    set({ phase: { kind: "transcribing" }, level: 0 });
    try {
      const text = await api.stopListening();
      if (mine !== attempt) return;
      set({ phase: { kind: "idle" }, heard: { text } });
    } catch (e) {
      if (mine !== attempt) return;
      const text = String(e);
      set({
        phase: { kind: "idle" },
        notice: text === CANCELED ? null : { text, tone: "error" },
      });
    }
  },

  cancel: () => {
    const { phase } = get();
    if (phase.kind === "idle" || phase.kind === "offer") return;
    attempt++;
    set({ phase: { kind: "idle" }, level: 0 });
    void api.cancelListening();
  },

  startDownload: async (model, fromMic) => {
    set({
      phase: { kind: "idle" },
      notice: null,
      download: { model, done: 0, total: 0, fromMic },
    });
    try {
      await api.downloadVoiceModel(model, ({ done, total }) => {
        const held = get().download;
        if (held?.model === model) set({ download: { ...held, done, total } });
      });
      // Still the download on screen: a second one replaces the first, and
      // the first resolves canceled rather than here.
      if (get().download?.model !== model) return;
      set({
        download: null,
        notice: fromMic
          ? { text: "The speech model is ready. Press the mic and talk.", tone: "info" }
          : null,
      });
    } catch (e) {
      if (get().download?.model !== model) return;
      const text = String(e);
      set({
        download: null,
        notice: text === CANCELED ? null : { text, tone: "error" },
      });
    }
    await get().refresh();
  },

  stopDownload: () => {
    void api.stopVoiceDownload();
  },

  decline: () => {
    if (get().phase.kind === "offer") set({ phase: { kind: "idle" } });
  },

  dismiss: () => set({ notice: null }),
}));

/** `bytes` as the prompt says it: "60 MB". */
export function megabytes(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

/** Seconds as a recording timer: `0:07`, `1:42`. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
