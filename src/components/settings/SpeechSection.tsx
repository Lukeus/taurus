// Settings › Voice, the second half: reading replies aloud, and in whose voice.

import { useCallback, useEffect, useState } from "react";

import * as api from "../../lib/api";
import type { SpeechEngine, SpeechSettings, SpeechStatus, VoiceInfo } from "../../lib/api";
import { APPLE } from "../../lib/keys";
import { useSpeech } from "../../state/speech";
import { Problem } from "../Problem";
import { ApiKeyField } from "./ApiKeyField";
import { Field, blank } from "./Field";

/** Models ElevenLabs offers for speech, fastest first. Any id is accepted. */
const ELEVENLABS_MODELS = [
  "eleven_flash_v2_5",
  "eleven_turbo_v2_5",
  "eleven_multilingual_v2",
  "eleven_v3",
];

/** What a test reading says. */
const SAMPLE = "This is how replies will sound when Taurus reads them aloud.";

/** The test reading's key, so the button can say Stop while it plays. */
const SAMPLE_KEY = "settings-sample";

/**
 * The system's voices grouped by language, the reader's own first.
 *
 * macOS lists nearly two hundred, most of them in languages the reader isn't
 * using, so an ungrouped list puts the one somebody wants a long scroll away.
 */
export function groupVoices(
  voices: VoiceInfo[],
  language: string,
): [string, VoiceInfo[]][] {
  const groups = new Map<string, VoiceInfo[]>();
  for (const voice of voices) {
    const key = voice.detail || "Other";
    groups.set(key, [...(groups.get(key) ?? []), voice]);
  }
  // `en-US`, `en_US` and `en-us` are one language as far as a reader is
  // concerned.
  const base = (tag: string) => tag.toLowerCase().split(/[-_]/)[0];
  const mine = base(language);
  const rank = (tag: string) =>
    tag.toLowerCase().replace("_", "-") === language.toLowerCase()
      ? 0
      : base(tag) === mine
        ? 1
        : 2;
  return [...groups.entries()]
    .map(([tag, list]): [string, VoiceInfo[]] => [
      tag,
      list.slice().sort((a, b) => a.name.localeCompare(b.name)),
    ])
    .sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b));
}

/** What the built-in choice is called here. */
function systemName(): string {
  if (APPLE) return "Built in (macOS voices)";
  if (typeof navigator !== "undefined" && /windows/i.test(navigator.userAgent)) {
    return "Built in (Windows voices)";
  }
  return "Built in (espeak-ng)";
}

export function SpeechSection({ keychain }: { keychain: boolean }) {
  const [status, setStatus] = useState<SpeechStatus | null>(null);
  /** What's on screen, which can be ahead of what's saved while typing. */
  const [draft, setDraft] = useState<SpeechSettings | null>(null);
  const [voices, setVoices] = useState<VoiceInfo[] | null>(null);
  const [voicesError, setVoicesError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const reading = useSpeech((s) => s.speaking && s.reading === SAMPLE_KEY);
  const say = useSpeech((s) => s.say);
  const stop = useSpeech((s) => s.stop);
  const failed = useSpeech((s) => s.failed);

  const load = useCallback(async () => {
    try {
      const fresh = await api.speechStatus();
      setStatus(fresh);
      setError(null);
      return fresh;
    } catch (e) {
      setError(String(e));
      return null;
    }
  }, []);

  useEffect(() => {
    void load().then((fresh) => fresh && setDraft(fresh.settings));
  }, [load]);

  const engine = draft?.engine;
  const keys = new Map(status?.key_statuses ?? []);
  const keyReady = (e: SpeechEngine) => {
    const k = keys.get(e);
    return k !== undefined && k.kind !== "missing";
  };
  const elevenReady = keyReady("elevenlabs");

  // The voices for the engine on screen. ElevenLabs's need a key, and are
  // asked for again when one is stored.
  useEffect(() => {
    // Cleared on every engine change, so one engine's failure isn't shown
    // under another's fields.
    setVoicesError(null);
    if (!engine || engine === "openai") {
      setVoices(engine === "openai" ? [] : null);
      return;
    }
    if (engine === "elevenlabs" && !elevenReady) {
      setVoices(null);
      return;
    }
    let live = true;
    setVoices(null);
    setVoicesError(null);
    api
      .speechVoices(engine)
      .then((list) => live && setVoices(list))
      .catch((e) => live && setVoicesError(String(e)));
    return () => {
      live = false;
    };
  }, [engine, elevenReady]);

  const save = async (next: SpeechSettings) => {
    setDraft(next);
    setBusy(true);
    setError(null);
    try {
      await api.saveSpeechSettings(next);
      const fresh = await load();
      // Kept if typed into since: the reload answers what was sent, and
      // anything newer than that is newer than the answer.
      if (fresh) {
        setDraft((now) =>
          JSON.stringify(now) === JSON.stringify(next) ? fresh.settings : now,
        );
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // ElevenLabs has no default voice, so the first on the account is picked
  // the moment there's a list to pick from. Reading aloud with nothing chosen
  // would only fail.
  useEffect(() => {
    if (draft?.engine !== "elevenlabs" || draft.elevenlabs.voice_id || !voices?.length) return;
    void save({ ...draft, elevenlabs: { ...draft.elevenlabs, voice_id: voices[0].id } });
    // `save` is stable enough: it only closes over setters and `load`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft?.engine, draft?.elevenlabs.voice_id, voices]);

  if (!draft || !status) {
    return (
      <section className="section">
        <span className="micro">Reading aloud</span>
        {error ? <Problem>{error}</Problem> : <p className="hint">Loading…</p>}
      </section>
    );
  }

  const patch = (change: Partial<SpeechSettings>) => setDraft({ ...draft, ...change });

  return (
    <section className="section speech-settings">
      <span className="micro">Reading aloud</span>
      <p className="hint m-0">
        Taurus reads the prose of a reply and skips code, diffs, tables and tool
        output. The speaker beside the mic turns it on and off, and Escape stops
        it.
      </p>

      <label className="flex items-center gap-2 text-13">
        <input
          type="checkbox"
          checked={draft.read_aloud}
          disabled={busy}
          onChange={(e) => void save({ ...draft, read_aloud: e.target.checked })}
        />
        Read replies aloud as they arrive
      </label>

      {/* Not "Voice": each service below has a Voice field of its own, and
          two fields with one name read as one question asked twice. */}
      <Field
        label="Spoken by"
        hint={
          draft.engine === "system"
            ? "Your computer's own voices. Nothing is sent anywhere."
            : "Each sentence of a reply is sent to this service to be spoken. Nothing else from the conversation is."
        }
      >
        <select
          value={draft.engine}
          disabled={busy}
          onChange={(e) => void save({ ...draft, engine: e.target.value as SpeechEngine })}
        >
          <option value="system">{systemName()}</option>
          <option value="elevenlabs">ElevenLabs</option>
          <option value="openai">OpenAI or a compatible server</option>
        </select>
      </Field>

      {draft.engine === "system" && (
        <Field
          label="System voice"
          hint={
            APPLE
              ? "Download Enhanced and Premium voices in System Settings › Accessibility › Spoken Content."
              : undefined
          }
        >
          <select
            value={draft.system.voice ?? ""}
            disabled={busy || voices === null}
            onChange={(e) =>
              void save({ ...draft, system: { voice: e.target.value || undefined } })
            }
          >
            <option value="">System default</option>
            {groupVoices(voices ?? [], navigator.language).map(([tag, list]) => (
              <optgroup key={tag} label={tag}>
                {list.map((v) => (
                  <option key={v.id} value={v.id}>
                    {v.name}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
        </Field>
      )}
      {/* Why the list is empty, which on Linux without espeak-ng is the
          backend saying what to install. It used to show only for
          ElevenLabs, leaving this select disabled with no reason. */}
      {draft.engine === "system" && voicesError && <Problem>{voicesError}</Problem>}

      {draft.engine === "elevenlabs" && (
        <div className="card-list">
          <div className="card">
            <div className="card-body">
              <ApiKeyField
                status={keys.get("elevenlabs")}
                available={keychain}
                onStore={(key) => api.setSpeechKey("elevenlabs", key)}
                onClear={() => api.clearSpeechKey("elevenlabs")}
                onChanged={() => void load()}
                unsavedHint="Choose ElevenLabs above before storing a key for it."
              />
              <Field
                label="Voice"
                hint={
                  elevenReady
                    ? "The voices on your account, including the ones you've added in ElevenLabs."
                    : "Store a key to list the voices on your account."
                }
              >
                <select
                  value={draft.elevenlabs.voice_id ?? ""}
                  disabled={busy || !voices?.length}
                  onChange={(e) =>
                    void save({
                      ...draft,
                      elevenlabs: { ...draft.elevenlabs, voice_id: e.target.value || undefined },
                    })
                  }
                >
                  {!draft.elevenlabs.voice_id && <option value="">None chosen</option>}
                  {/* Kept when the list hasn't come back, so the select
                      doesn't read as if the choice was lost. */}
                  {draft.elevenlabs.voice_id &&
                    !voices?.some((v) => v.id === draft.elevenlabs.voice_id) && (
                      <option value={draft.elevenlabs.voice_id}>
                        {draft.elevenlabs.voice_id}
                      </option>
                    )}
                  {(voices ?? []).map((v) => (
                    <option key={v.id} value={v.id}>
                      {v.name}
                      {v.detail && ` · ${v.detail}`}
                    </option>
                  ))}
                </select>
              </Field>
              {voicesError && <Problem>{voicesError}</Problem>}
              <Field label="Model" hint="Flash is the quickest to start speaking; Multilingual v2 sounds best.">
                <input
                  list="elevenlabs-models"
                  value={draft.elevenlabs.model}
                  disabled={busy}
                  spellCheck={false}
                  onChange={(e) =>
                    patch({ elevenlabs: { ...draft.elevenlabs, model: e.target.value } })
                  }
                  onBlur={() => void save(draft)}
                />
                <datalist id="elevenlabs-models">
                  {ELEVENLABS_MODELS.map((m) => (
                    <option key={m} value={m} />
                  ))}
                </datalist>
              </Field>
              <Field
                label="API key variable"
                hint="Optional. An environment variable that overrides the stored key."
              >
                <input
                  value={draft.elevenlabs.api_key_env ?? ""}
                  disabled={busy}
                  spellCheck={false}
                  placeholder="ELEVENLABS_API_KEY"
                  onChange={(e) =>
                    patch({
                      elevenlabs: {
                        ...draft.elevenlabs,
                        api_key_env: blank(e.target.value) ?? undefined,
                      },
                    })
                  }
                  onBlur={() => void save(draft)}
                />
              </Field>
            </div>
          </div>
        </div>
      )}

      {draft.engine === "openai" && (
        <div className="card-list">
          <div className="card">
            <div className="card-body">
              <Field
                label="Address"
                hint="Up to and including /v1. An Azure deployment or APIM gateway works too, with its ?api-version= on the end, and so does a local server such as Kokoro-FastAPI."
              >
                <input
                  value={draft.openai.base_url}
                  disabled={busy}
                  spellCheck={false}
                  placeholder="https://api.openai.com/v1"
                  onChange={(e) => patch({ openai: { ...draft.openai, base_url: e.target.value } })}
                  onBlur={() => void save(draft)}
                />
              </Field>
              <Field label="Model">
                <input
                  value={draft.openai.model}
                  disabled={busy}
                  spellCheck={false}
                  placeholder="gpt-4o-mini-tts"
                  onChange={(e) => patch({ openai: { ...draft.openai, model: e.target.value } })}
                  onBlur={() => void save(draft)}
                />
              </Field>
              <Field label="Voice" hint="OpenAI's are listed. Another server's are whatever it calls them.">
                <input
                  list="openai-voices"
                  value={draft.openai.voice}
                  disabled={busy}
                  spellCheck={false}
                  onChange={(e) => patch({ openai: { ...draft.openai, voice: e.target.value } })}
                  onBlur={() => void save(draft)}
                />
                <datalist id="openai-voices">
                  {(voices ?? []).map((v) => (
                    <option key={v.id} value={v.id} />
                  ))}
                </datalist>
              </Field>
              <ApiKeyField
                status={keys.get("openai")}
                available={keychain}
                onStore={(key) => api.setSpeechKey("openai", key)}
                onClear={() => api.clearSpeechKey("openai")}
                onChanged={() => void load()}
                unsavedHint="Choose this service above before storing a key for it."
              />
              <Field
                label="API key variable"
                hint="Optional, and not OPENAI_API_KEY unless you mean it: whatever this names is sent to the address above."
              >
                <input
                  value={draft.openai.api_key_env ?? ""}
                  disabled={busy}
                  spellCheck={false}
                  placeholder="none"
                  onChange={(e) =>
                    patch({
                      openai: { ...draft.openai, api_key_env: blank(e.target.value) ?? undefined },
                    })
                  }
                  onBlur={() => void save(draft)}
                />
              </Field>
              <Field
                label="API key header"
                hint="Empty sends the key as a bearer token. An Azure APIM gateway wants Ocp-Apim-Subscription-Key, Azure OpenAI wants api-key."
              >
                <input
                  value={draft.openai.api_key_header ?? ""}
                  disabled={busy}
                  spellCheck={false}
                  placeholder="Authorization: Bearer"
                  onChange={(e) =>
                    patch({
                      openai: { ...draft.openai, api_key_header: blank(e.target.value) ?? undefined },
                    })
                  }
                  onBlur={() => void save(draft)}
                />
              </Field>
            </div>
          </div>
        </div>
      )}

      <div className="flex items-center gap-2">
        <button
          disabled={busy}
          onClick={() => (reading ? stop() : say(SAMPLE, SAMPLE_KEY))}
        >
          {reading ? "Stop" : "Try it"}
        </button>
      </div>

      {failed && <Problem>{failed}</Problem>}
      {error && <Problem>{error}</Problem>}
    </section>
  );
}
