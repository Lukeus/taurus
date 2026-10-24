/**
 * Reading replies aloud, from the window's side.
 *
 * The backend owns the voice, the filter that decides what's worth saying,
 * and the speakers. This decides what to hand it: the turn this window sent,
 * piece by piece as it streams, when read-aloud is on, or one reply whole
 * when its Read aloud button is pressed.
 *
 * Its own store, like the mic's, because none of it belongs to a
 * conversation: there's one pair of speakers.
 */
import { create } from "zustand";

import * as api from "../lib/api";
import type { UiEvent } from "../lib/api";

type Speech = {
  /** Whether anything is being said, or is queued to be. */
  speaking: boolean;
  /**
   * What the Read aloud button that started this reading belongs to, so that
   * button can say Stop. `null` for a reply read as it streamed.
   */
  reading: string | null;
  /** Why the last reading stopped, when it stopped by failing. */
  failed: string | null;

  /** Reads `text` from the start, instead of anything being read. */
  say: (text: string, key: string) => void;
  stop: () => void;
  dismiss: () => void;
};

/**
 * Feed calls, one after another. Two `speech_feed` invokes in flight at once
 * run on whichever backend worker is free, and could reach the filter in the
 * wrong order.
 */
let chain: Promise<void> = Promise.resolve();

/** Bumped by every stop, so a call still queued from before it is dropped. */
let epoch = 0;

/** Whether anything was fed since the last flush, which is what makes one worth sending. */
let fed = false;

/**
 * Set when reading failed, until the next turn. Everything after a refusal
 * would be refused for the same reason, and each would say so again.
 */
let muted = false;

function queue(call: () => Promise<void>) {
  const mine = epoch;
  chain = chain
    .then(() => (mine === epoch ? call() : undefined))
    .catch((e) => {
      if (mine !== epoch) return;
      // Once per turn, not once per piece. See `muted`.
      epoch++;
      muted = true;
      useSpeech.setState({ failed: String(e), speaking: false, reading: null });
    });
}

export const useSpeech = create<Speech>((set) => ({
  speaking: false,
  reading: null,
  failed: null,

  say: (text, key) => {
    epoch++;
    fed = false;
    muted = false;
    set({ failed: null, reading: key, speaking: true });
    queue(() => api.speechSay(text));
  },

  stop: () => {
    epoch++;
    fed = false;
    set({ reading: null, speaking: false });
    api.speechStop().catch((e) => console.warn("could not stop reading aloud", e));
  },

  dismiss: () => set({ failed: null }),
}));

/**
 * Wires the backend's reports in. Called once, at startup, with the other
 * listeners.
 */
export function listenForSpeech() {
  return api.onSpeech(({ speaking, failed }) => {
    // The speaker dropped everything when it failed. What the turn streams
    // after that would fail the same way.
    if (failed) muted = true;
    useSpeech.setState((s) => ({
      speaking,
      reading: speaking ? s.reading : null,
      failed: failed ?? s.failed,
    }));
  });
}

/**
 * A turn is about to start: whatever was being read is over, whether it was
 * the last reply or a button press.
 */
export function startTurn() {
  if (useSpeech.getState().speaking) useSpeech.getState().stop();
  epoch++;
  fed = false;
  muted = false;
  useSpeech.setState({ failed: null });
}

/**
 * The window is moving to another conversation. A reply that streamed faster
 * than it could be read leaves a backlog, and reading it on over a different
 * conversation would be reading the wrong one.
 */
export function leave() {
  if (useSpeech.getState().speaking) useSpeech.getState().stop();
}

/**
 * One batch of a streaming turn's events, as reading: the prose goes in, and
 * a tool call or the end of the turn says what was held back.
 */
export function hear(events: UiEvent[]) {
  if (muted) return;
  let text = "";
  const send = () => {
    if (!text) return;
    const piece = text;
    text = "";
    fed = true;
    queue(() => api.speechFeed(piece));
  };
  for (const event of events) {
    if (event.type === "text_delta") {
      text += event.text;
    } else if (event.type === "tool_call_started" || event.type === "turn_finished") {
      send();
      if (fed) {
        fed = false;
        queue(() => api.speechFlush());
      }
    }
  }
  send();
}

/** The turn is over, however it ended. Says anything still held back. */
export function endTurn() {
  if (!fed || muted) return;
  fed = false;
  queue(() => api.speechFlush());
}

/** All the prose a turn's replies wrote, for reading the turn again. */
export function turnText(texts: string[]): string {
  return texts.filter((t) => t.trim()).join("\n\n");
}
