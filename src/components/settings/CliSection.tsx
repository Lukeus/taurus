import { useEffect, useState } from "react";

import * as api from "../../lib/api";
import { Problem } from "../Problem";

/**
 * The `taurus` command the app ships, and a way to put it on the PATH.
 *
 * On the About tab, beside Updates, because they're the same story: an update
 * replaces the app, and with it the command, so the CLI in a terminal is
 * always the app's version. Reading where it is touches only the disk; the
 * button is the only thing that writes.
 *
 * The button's label is quoted by `taurus doctor` when it finds a CLI behind
 * the app, so it says the same words.
 */
export function CliSection() {
  const [status, setStatus] = useState<api.CliStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .cliStatus()
      .then(setStatus)
      .catch((e) => setError(String(e)));
  }, []);

  const install = () => {
    setError(null);
    setBusy(true);
    api
      .installCli()
      .then(setStatus)
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const windows = status?.target?.toLowerCase().endsWith(".exe") ?? false;
  const dir = status?.not_on_path;

  return (
    <section className="section">
      <span className="micro">Command line</span>
      <div className="card-row">
        <span className="text-12-5 text-dim">
          {status?.installed ? (
            <>
              <span className="font-mono">taurus</span> is at{" "}
              <span className="font-mono">{status.target}</span> and updates with the app.
            </>
          ) : (
            <>
              Use Taurus from a terminal as <span className="font-mono">taurus</span>, the same
              version as this app.
            </>
          )}
        </span>
        <div className="spacer" />
        {status && !status.cannot_install && (
          <button
            className={status.installed ? undefined : "primary"}
            disabled={busy}
            onClick={install}
          >
            {busy ? "Installing…" : status.installed ? "Reinstall" : "Install the taurus command"}
          </button>
        )}
      </div>

      {status?.cannot_install && <p className="hint">{status.cannot_install}</p>}
      {status && !status.installed && status.target && !status.cannot_install && (
        <p className="hint">
          Puts it at <span className="font-mono">{status.target}</span>
          {status.needs_admin ? ", which asks for an administrator password." : "."}
        </p>
      )}
      {status?.installed && dir && (
        <p className="hint">
          {windows ? (
            <>
              <span className="font-mono">{dir}</span> was added to your PATH. Terminals opened
              from now on will find it; if one doesn't, sign out and back in.
            </>
          ) : (
            <>
              <span className="font-mono">{dir}</span> isn't on your PATH. Add{" "}
              <span className="font-mono">export PATH="{dir}:$PATH"</span> to your shell's
              profile.
            </>
          )}
        </p>
      )}
      {status?.installed && status.shadowed_by && (
        <p className="hint">
          A terminal runs <span className="font-mono">{status.shadowed_by}</span> first, an older
          copy perhaps. Remove it, or put this one's folder ahead of it on your PATH.
        </p>
      )}

      {error && <Problem>{error}</Problem>}
    </section>
  );
}
