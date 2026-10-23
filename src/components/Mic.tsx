import { useEffect, useRef, useState } from "react";

import * as api from "../lib/api";
import { APPLE } from "../lib/keys";
import { useSpeech } from "../state/speech";
import { useStore } from "../state/store";
import { clock, MAX_SECONDS, megabytes, useVoice } from "../state/voice";

/** How the dictation shortcut is written: the same chord on every platform. */
export const DICTATE_SHORTCUT = APPLE ? "⌘⇧D" : "Ctrl+Shift+D";

/**
 * Pressed for longer than this, the mic is push-to-talk: letting go finishes.
 * A shorter press is a click, and it stays on until the next one.
 */
const HOLD_MS = 400;

/**
 * The mic button in the composer's foot.
 *
 * Two ways to use it, from one control. Click it to start and click again to
 * finish, for a long thought. Or hold it while talking and let go, for a short
 * one. The hold is told apart from the click by how long the pointer stayed
 * down, so neither needs a setting.
 */
export function MicButton({ disabled }: { disabled: boolean }) {
  const phase = useVoice((s) => s.phase);
  const level = useVoice((s) => s.level);
  const press = useVoice((s) => s.press);
  const finish = useVoice((s) => s.finish);

  // Set on pointer-down, so the click that follows the release doesn't toggle
  // a second time. Keyboard activation has no pointer-down and goes through
  // `onClick` alone.
  const downAt = useRef<number | null>(null);

  const listening = phase.kind === "listening";
  const working = phase.kind === "opening" || phase.kind === "transcribing";

  const tip = listening
    ? "Click to finish · esc to discard"
    : phase.kind === "opening"
      ? "Opening the microphone…"
      : phase.kind === "transcribing"
        ? "Turning speech into text…"
        : `Dictate (${DICTATE_SHORTCUT}). Click to start and again to finish, or hold while you talk.`;

  return (
    <button
      type="button"
      className="composer-mic relative grid size-8 shrink-0 place-items-center rounded-full border border-line bg-transparent p-0 text-dim hover:not-disabled:text-ink data-listening:border-danger data-listening:text-danger data-working:text-faint"
      data-listening={listening || undefined}
      data-working={working || undefined}
      aria-label={listening ? "Finish dictating" : "Dictate"}
      aria-pressed={listening}
      disabled={disabled || working}
      data-tip={tip}
      onPointerDown={(e) => {
        if (e.button !== 0 || phase.kind !== "idle") return;
        downAt.current = Date.now();
        void press();
      }}
      onPointerUp={() => {
        const at = downAt.current;
        if (at === null) return;
        if (Date.now() - at >= HOLD_MS) void finish();
      }}
      onClick={() => {
        if (downAt.current !== null) {
          downAt.current = null;
          return;
        }
        void press();
      }}
    >
      {/* The input level, as a ring that swells with it. Driven by the
          numbers rather than animated, so it holds still when the room
          does — and in a screenshot. */}
      {listening && (
        <span
          aria-hidden="true"
          className="mic-level pointer-events-none absolute inset-0 rounded-full bg-danger opacity-20"
          style={{ transform: `scale(${1 + Math.min(level * 4, 1) * 0.45})` }}
        />
      )}
      <svg
        aria-hidden="true"
        viewBox="0 0 16 16"
        className="relative size-4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      >
        <rect x="5.5" y="1.75" width="5" height="8" rx="2.5" />
        <path d="M3.25 7.5a4.75 4.75 0 0 0 9.5 0M8 12.25v2" />
      </svg>
    </button>
  );
}

/**
 * Read-aloud, beside the mic: a switch while it's quiet, and Stop while it's
 * talking.
 *
 * One control for both, because they're never wanted at once. While a reply
 * is being read the thing anybody reaches for is a way to make it stop;
 * otherwise it's whether the next one should be read at all.
 */
export function SpeakerButton() {
  const speech = useStore((s) => s.status?.settings.speech ?? null);
  const speaking = useSpeech((s) => s.speaking);
  const stop = useSpeech((s) => s.stop);
  if (!speech) return null;
  const on = speech.read_aloud;

  const tip = speaking
    ? "Stop reading (esc)"
    : on
      ? "Replies are read aloud. Click to stop reading them."
      : "Read replies aloud as they arrive";

  return (
    <button
      type="button"
      className="composer-speaker grid size-8 shrink-0 place-items-center rounded-full border border-line bg-transparent p-0 text-dim hover:text-ink data-on:text-accent data-speaking:border-accent data-speaking:text-accent"
      data-on={on || undefined}
      data-speaking={speaking || undefined}
      aria-label={speaking ? "Stop reading" : "Read replies aloud"}
      aria-pressed={speaking ? undefined : on}
      data-tip={tip}
      onClick={() => {
        if (speaking) return stop();
        void api.saveSpeechSettings({ ...speech, read_aloud: !on }).catch((e) =>
          useSpeech.setState({ failed: String(e) }),
        );
      }}
    >
      <svg
        aria-hidden="true"
        viewBox="0 0 16 16"
        className="size-4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        {speaking ? (
          <rect x="4.5" y="4.5" width="7" height="7" rx="1.25" />
        ) : (
          <>
            <path d="M2.75 6.25h2.5L8.5 3.5v9L5.25 9.75h-2.5z" />
            {on ? (
              <path d="M10.75 6a2.75 2.75 0 0 1 0 4M12.5 4.25a5.25 5.25 0 0 1 0 7.5" />
            ) : (
              <path d="M11 6.25l3 3.5M14 6.25l-3 3.5" />
            )}
          </>
        )}
      </svg>
    </button>
  );
}

/**
 * What the mic has to say, above the box: the offer to download a model,
 * the download's progress, and what went wrong.
 *
 * Above rather than in a dialog, the way the queued message and the
 * interrupted turn are: it's about the next message, and the box is where
 * you're looking.
 */
export function VoiceStrip() {
  const phase = useVoice((s) => s.phase);
  const download = useVoice((s) => s.download);
  const notice = useVoice((s) => s.notice);
  const startDownload = useVoice((s) => s.startDownload);
  const stopDownload = useVoice((s) => s.stopDownload);
  const decline = useVoice((s) => s.decline);
  const dismiss = useVoice((s) => s.dismiss);
  const speechFailed = useSpeech((s) => s.failed);
  const dismissSpeech = useSpeech((s) => s.dismiss);

  if (phase.kind === "offer") {
    const { model } = phase;
    return (
      <div className="composer-queued held voice-offer" role="status">
        <span className="dataset-mark">◉</span>
        {/* Wrapped, unlike a queued message: this is the one consent voice
            asks for, and an ellipsis would cut off the half about privacy. */}
        <span className="composer-queued-text whitespace-normal">
          Dictation runs on this computer. It needs a one-time{" "}
          {megabytes(model.bytes)} download of the {model.id} speech model.
          What you say is never saved or sent anywhere.
        </span>
        <div className="spacer" />
        <button
          className="quiet shrink-0 whitespace-nowrap"
          onClick={() => void startDownload(model.id, true)}
        >
          Download
        </button>
        <button className="quiet shrink-0 whitespace-nowrap" onClick={decline}>
          Not now
        </button>
      </div>
    );
  }

  if (download) {
    const pct = download.total > 0 ? Math.round((download.done / download.total) * 100) : 0;
    return (
      <div className="composer-queued voice-download" role="status">
        <span className="dataset-mark">↓</span>
        <span className="composer-queued-text">
          Downloading the {download.model} speech model… {pct}% of{" "}
          {megabytes(download.total)}
        </span>
        <div className="plan-bar w-24 shrink-0" aria-hidden="true">
          <span className="plan-bar-fill" style={{ width: `${pct}%` }} />
        </div>
        <div className="spacer" />
        <button className="quiet" onClick={stopDownload}>
          Cancel
        </button>
      </div>
    );
  }

  if (notice) {
    return (
      <div
        className="composer-queued held voice-notice data-error:border-l-danger"
        data-error={notice.tone === "error" || undefined}
        role={notice.tone === "error" ? "alert" : "status"}
      >
        <span className="dataset-mark">{notice.tone === "error" ? "!" : "◉"}</span>
        <span className="composer-queued-text whitespace-normal">{notice.text}</span>
        <div className="spacer" />
        <button className="quiet" onClick={dismiss} aria-label="Dismiss">
          ✕
        </button>
      </div>
    );
  }

  // Why reading aloud stopped. Here because a reply that was being read and
  // went quiet leaves somebody looking at the box, not at Settings.
  if (speechFailed) {
    return (
      <div
        className="composer-queued held speech-notice border-l-danger"
        role="alert"
      >
        <span className="dataset-mark">!</span>
        <span className="composer-queued-text whitespace-normal">
          Couldn't read the reply aloud. {speechFailed}
        </span>
        <div className="spacer" />
        <button className="quiet" onClick={dismissSpeech} aria-label="Dismiss">
          ✕
        </button>
      </div>
    );
  }
  return null;
}

/**
 * The hint line's words while the mic is busy, or `null` when it isn't.
 *
 * Counts up while listening, and finishes the recording itself at the limit
 * the backend would otherwise cut it at, so the words aren't lost silently.
 */
export function useMicHint(): string | null {
  const phase = useVoice((s) => s.phase);
  const finish = useVoice((s) => s.finish);
  const [now, setNow] = useState(() => Date.now());

  const since = phase.kind === "listening" ? phase.since : null;
  useEffect(() => {
    if (since === null) return;
    const tick = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(tick);
  }, [since]);

  const elapsed = since === null ? 0 : (now - since) / 1000;
  useEffect(() => {
    if (since !== null && elapsed >= MAX_SECONDS) void finish();
  }, [since, elapsed, finish]);

  switch (phase.kind) {
    case "listening":
      return `● ${clock(elapsed)} listening · click the mic or ${DICTATE_SHORTCUT} to finish · esc discards`;
    case "opening":
      return "opening the microphone…";
    case "transcribing":
      return "turning speech into text…";
    default:
      return null;
  }
}
