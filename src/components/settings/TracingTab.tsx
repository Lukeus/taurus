// The Tracing tab: where traces are sent, and whether they carry the messages.

import { useEffect, useState } from "react";

import * as api from "../../lib/api";
import type { TraceExport } from "../../lib/api";
import { useStore } from "../../state/store";
import { Problem } from "../Problem";
import { Field, FromProject } from "./Field";

/**
 * Sending traces to a collector.
 *
 * Both settings were already read — the endpoint by the exporter at startup,
 * the opt-in by every turn — and the Traces panel told people to edit
 * settings.json to reach them.
 *
 * The endpoint can't be changed under a running app: the exporter is part of
 * the one log subscriber, installed before the window exists. So the field
 * says what this run is actually doing beside what's saved, and when the two
 * differ, that a restart is what joins them.
 */
export function TracingTab() {
  const status = useStore((s) => s.status);
  const refresh = useStore((s) => s.refresh);
  const saved = status?.settings.otlp_endpoint ?? "";
  const [draft, setDraft] = useState(saved);
  const [running, setRunning] = useState<TraceExport | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => setDraft(saved), [saved]);
  useEffect(() => {
    api
      .traceExport()
      .then(setRunning)
      .catch((e) => setError(`Couldn't read what this run is exporting: ${e}`));
  }, []);

  const commit = async () => {
    if (draft.trim() === saved) {
      setDraft(saved);
      return;
    }
    setError(null);
    try {
      await api.setOtlpEndpoint(draft);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <>
      <p className="drawer-intro">
        Sends each turn's spans to an OpenTelemetry collector over OTLP/HTTP:
        which model, how many tokens, how long, which tools ran and what
        failed. Langfuse, Phoenix, Honeycomb and a local collector all take
        it. The Traces panel shows the same spans for this run without any of
        this.
      </p>

      <Field
        label="Endpoint"
        hint="Empty sends nothing anywhere. Read when Taurus starts, so a change here applies from the next launch."
      >
        <input
          className="font-mono"
          aria-label="OTLP endpoint"
          placeholder="http://localhost:4318"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => void commit()}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
          }}
        />
      </Field>

      {running && <p className="trace-running hint m-0">{describe(running, saved)}</p>}

      <label className="settings-check">
        <input
          type="checkbox"
          checked={status?.settings.otlp_capture_content ?? false}
          onChange={(e) => {
            const on = e.target.checked;
            setError(null);
            api
              .setOtlpCaptureContent(on)
              .then(() => refresh())
              .catch((err) => setError(String(err)));
          }}
        />
        <span>
          Include the messages themselves
          <span className="hint">
            Off, a trace describes a turn. On, it contains it: your prompts,
            the files read, the commands run and their output, sent to the
            endpoint above. Worth it for a collector you run yourself. Applies
            from the next turn.
          </span>
        </span>
      </label>
      <FromProject field="otlp_capture_content" />

      {error && <Problem>{error}</Problem>}
    </>
  );
}

/**
 * What this run is doing, in the terms of what's saved.
 *
 * Exported for its tests: each branch is a case somebody gets confused by —
 * the variable winning over the field, a typo'd endpoint exporting nothing, a
 * saved change that hasn't been picked up.
 */
export function describe(running: TraceExport, saved: string): string {
  const now = running.endpoint ?? "";
  if (running.failed) {
    return `This run couldn't export to ${now}: ${running.failed}`;
  }
  if (running.from_env) {
    return `This run exports to ${now}, from OTEL_EXPORTER_OTLP_ENDPOINT, which wins over the field.`;
  }
  if (now === saved.trim()) {
    return now ? `Exporting to ${now} now.` : "Not exporting.";
  }
  if (!now) return "This run isn't exporting. Restart Taurus to start.";
  return saved.trim()
    ? `This run exports to ${now}. Restart Taurus to use the saved one.`
    : `This run exports to ${now}. Restart Taurus to stop.`;
}
