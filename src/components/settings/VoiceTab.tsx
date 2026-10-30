import { useEffect, useState } from "react";

import * as api from "../../lib/api";
import { useArmed } from "../../lib/armed";
import { megabytes, useVoice } from "../../state/voice";
import { DICTATE_SHORTCUT } from "../Mic";
import { Problem } from "../Problem";
import { SpeechSection } from "./SpeechSection";

/**
 * The Voice tab: which speech model the mic uses, the files on disk, and
 * reading replies aloud.
 *
 * Downloads go through the same store the composer's prompt does, so one
 * started here shows its progress there too, and the mic knows the moment the
 * file lands.
 */
export function VoiceTab() {
  const status = useVoice((s) => s.status);
  const download = useVoice((s) => s.download);
  const notice = useVoice((s) => s.notice);
  const refresh = useVoice((s) => s.refresh);
  const startDownload = useVoice((s) => s.startDownload);
  const stopDownload = useVoice((s) => s.stopDownload);
  const { armed, arm, disarm } = useArmed<string>();
  const [error, setError] = useState<string | null>(null);
  const [keychain, setKeychain] = useState(false);

  useEffect(() => {
    void refresh();
    api.keychainAvailable().then(setKeychain).catch(() => setKeychain(false));
  }, [refresh]);

  const run = (action: () => Promise<unknown>) => {
    setError(null);
    action()
      .then(() => refresh())
      .catch((e) => setError(String(e)));
  };

  return (
    <>
      <p className="drawer-intro">
        Press the mic in the composer, or {DICTATE_SHORTCUT}, and talk. Speech
        is turned into text on this computer by whisper.cpp; the audio is never
        saved or sent anywhere. The text lands in the box for you to check
        before sending.
      </p>

      <section className="section">
        <span className="micro">Speech model</span>
        {status === null ? (
          <p className="hint">Reading what's downloaded…</p>
        ) : (
          <ul className="card-list">
            {status.models.map((model) => {
              const chosen = model.id === status.model;
              const fetching = download?.model === model.id;
              const pct =
                fetching && download.total > 0
                  ? Math.round((download.done / download.total) * 100)
                  : 0;
              return (
                <li key={model.id} className="card voice-model" data-current={chosen || undefined}>
                  <div className="card-body">
                    <div className="card-row">
                      <label className="flex min-w-0 items-center gap-2">
                        <input
                          type="radio"
                          name="voice-model"
                          checked={chosen}
                          onChange={() => run(() => api.setVoiceModel(model.id))}
                        />
                        <span className="card-title font-mono text-12-5">{model.id}</span>
                      </label>
                      <span className="tag">{megabytes(model.bytes)}</span>
                      {model.installed && <span className="tag project">downloaded</span>}
                      <div className="spacer" />
                      {fetching ? (
                        <button onClick={stopDownload}>Cancel</button>
                      ) : model.installed ? (
                        armed === model.id ? (
                          <>
                            <button
                              className="danger"
                              onClick={() => {
                                disarm();
                                run(() => api.deleteVoiceModel(model.id));
                              }}
                            >
                              Delete {megabytes(model.bytes)}
                            </button>
                            <button onClick={disarm}>Keep it</button>
                          </>
                        ) : (
                          <button onClick={() => arm(model.id)}>Delete</button>
                        )
                      ) : (
                        <button
                          disabled={download !== null}
                          data-tip={download ? "One download at a time" : undefined}
                          onClick={() => void startDownload(model.id, false)}
                        >
                          Download
                        </button>
                      )}
                    </div>
                    <p className="hint m-0">{model.summary}</p>
                    {fetching && (
                      <div className="plan-bar w-full" aria-hidden="true">
                        <span className="plan-bar-fill" style={{ width: `${pct}%` }} />
                      </div>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
        <p className="hint">
          A model you pick but haven't downloaded is offered for download the
          next time you press the mic.
        </p>
      </section>

      {status && (
        <section className="section">
          <span className="micro">On this machine</span>
          <dl className="voice-files m-0 grid grid-cols-[88px_1fr] gap-x-3 gap-y-1.5 text-12-5 [&_dt]:text-faint [&_dd]:m-0 [&_dd]:wrap-anywhere [&_dd]:font-mono [&_dd]:text-12 [&_dd]:text-dim">
            <dt>Microphone</dt>
            <dd>{status.microphone ?? "none found"}</dd>
            <dt>Models</dt>
            <dd>{status.folder}</dd>
          </dl>
        </section>
      )}

      {notice?.tone === "error" && <Problem>{notice.text}</Problem>}
      {error && <Problem>{error}</Problem>}

      <SpeechSection keychain={keychain} />
    </>
  );
}
