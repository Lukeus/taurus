import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import * as api from "../lib/api";
import type { CatalogEntry, McpServerDraft, Scope } from "../lib/api";
import {
  credentialFor,
  credentialHosts,
  fromCatalog,
  joinCommandLine,
  keepsInKeychain,
  leaksSecret,
  missingInputs,
  scopeFor,
  withReferences,
} from "../lib/mcp";
import { Problem } from "./Problem";

/**
 * What a catalogue entry needs before it can be written down.
 *
 * The step that makes a catalogue worth more than a list of command lines. An
 * entry that wrote `npx -y …server-filesystem` and asked for no directory would
 * install a server that refuses every path it is handed, and the failure would
 * arrive later, inside a tool call, wearing no connection to the button that
 * caused it. So the entry declares what it needs, and this asks for it.
 *
 * It does not save. What it produces is an ordinary `McpServerDraft`, handed to
 * the same editor an entry typed by hand goes through — so the command line is
 * on screen before anything is written, Test is the same Test, and the file is
 * merged the same way. The catalogue supplies the knowledge and stops there;
 * nothing here is a route into `mcp.json` that the form does not already own.
 *
 * Skipped entirely for an entry with nothing to ask, which is most of them.
 *
 * A token is the one answer that doesn't go into the draft. Where a saved
 * credential can reach the server, Continue stores it in the keychain first and
 * the draft says `${secret:NAME}` in its place, so the file never holds it. See
 * `keepsInKeychain` for where that is.
 */
export function McpSetup({
  entry,
  onReady,
  onCancel,
}: {
  entry: CatalogEntry;
  /** Hands the filled-in draft to the editor. */
  onReady: (draft: McpServerDraft) => void;
  onCancel: () => void;
}) {
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [scope, setScope] = useState<Scope>(scopeFor(entry));
  /*
   * Whether the workspace-file warning has been read and overruled.
   *
   * A separate acknowledgement rather than a disabled control, because there
   * are good reasons to want a project-scoped credential — a token for a
   * throwaway sandbox, a repository nobody else has — and the app is not
   * entitled to refuse them. What it is entitled to do is make sure the choice
   * was made rather than defaulted into.
   */
  const [acknowledged, setAcknowledged] = useState(false);
  /** Asked once. Until it answers, tokens are treated as going in the file. */
  const [keychain, setKeychain] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .keychainAvailable()
      .then((available) => live && setKeychain(available))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);

  const missing = missingInputs(entry, answers);
  const keep = keepsInKeychain(entry, scope, keychain);
  const leaks = leaksSecret(entry, scope) && !keep;
  const draft = fromCatalog(entry, keep ? withReferences(entry, answers) : answers, scope);
  const blocked = missing.length > 0 || (leaks && !acknowledged) || saving;

  /** Stores each token, then hands over a draft that names them. */
  const proceed = async () => {
    if (!keep) return onReady(draft);
    setSaving(true);
    setError(null);
    try {
      const hosts = credentialHosts(entry, answers);
      for (const input of entry.inputs) {
        const value = (answers[input.key] ?? "").trim();
        if (input.kind !== "secret" || !value) continue;
        await api.saveCredential(credentialFor(entry, input.key), hosts, value);
      }
      onReady(draft);
    } catch (e) {
      setError(`The token wasn't saved: ${e}`);
    } finally {
      setSaving(false);
    }
  };

  const set = (key: string, value: string) =>
    setAnswers((held) => ({ ...held, [key]: value }));

  return (
    <div className="mcp-setup">
      <div className="drawer-head">
        <button className="link" onClick={onCancel}>
          ← Back
        </button>
        <div className="spacer" />
        <a className="link" href={entry.homepage} target="_blank" rel="noreferrer">
          Source
        </a>
      </div>

      <h3 className="card-title">{entry.name}</h3>
      <p className="drawer-intro">{entry.blurb}</p>

      {entry.inputs.map((input) => (
        <label key={input.key} className="field">
          <span className="micro">
            {input.label}
            {!input.required && <span className="field-optional"> optional</span>}
          </span>
          <div className="field-with-button">
            <input
              type={input.kind === "secret" ? "password" : "text"}
              value={answers[input.key] ?? ""}
              placeholder={input.placeholder ?? ""}
              // A credential typed into a box the browser might remember is
              // still a credential. Neither of these is a strong guarantee;
              // both are cheap.
              autoComplete={input.kind === "secret" ? "off" : undefined}
              spellCheck={false}
              onChange={(e) => set(input.key, e.target.value)}
            />
            {input.kind === "path" && (
              <button
                onClick={async () => {
                  const picked = await open({ directory: true, multiple: false });
                  if (typeof picked === "string") set(input.key, picked);
                }}
              >
                Choose…
              </button>
            )}
          </div>
          <span className="field-help">
            {input.help}
            {input.link && (
              <>
                {" "}
                <a href={input.link} target="_blank" rel="noreferrer">
                  Get one
                </a>
              </>
            )}
          </span>
        </label>
      ))}

      <label className="field">
        <span className="micro">Where to save it</span>
        <select
          value={scope}
          onChange={(e) => {
            setScope(e.target.value as Scope);
            // Re-asked on every change, so acknowledging the warning once does
            // not silently cover a later switch back into it.
            setAcknowledged(false);
          }}
        >
          <option value="global">Every project — ~/.taurus/mcp.json</option>
          <option value="workspace">
            This project only — .taurus/mcp.json
          </option>
        </select>
        <span className="field-help">
          {scope === "global"
            ? "Available in every workspace you open."
            : "Only in this workspace, and the file lives inside it."}
        </span>
      </label>

      {/* The one footgun the catalogue can catch that somebody filling in a
          form cannot: a token written into a file inside a repository is one
          `git add .` from being published. */}
      {leaks && (
        <label className="mcp-leak">
          <input
            type="checkbox"
            checked={acknowledged}
            onChange={(e) => setAcknowledged(e.target.checked)}
          />
          <span>
            This writes the credential into a file inside the workspace, where a
            commit can publish it. Every entry here that wants one defaults to
            the global file instead. Save it here anyway.
          </span>
        </label>
      )}

      {/* What is about to be written, before it is written. The editor shows it
          again and lets it be changed; this is so that pressing Continue is
          never the first time the command line has been on screen. */}
      <div className="mcp-preview">
        <span className="micro">What this adds</span>
        <pre>
          {entry.transport === "stdio"
            ? joinCommandLine([draft.command, ...draft.args])
            : draft.url}
        </pre>
        {draft.env.concat(draft.headers).map((value) => (
          <pre key={value.key}>
            {value.key}=
            {!keep && entry.inputs.some((i) => i.kind === "secret") ? "••••••" : value.value}
          </pre>
        ))}
        {keep && (
          <span className="field-help">
            The token goes to your keychain as a saved credential, and the file only
            names it. Settings › Profile lists it.
          </span>
        )}
      </div>

      {error && <Problem>{error}</Problem>}

      <div className="actions">
        <button
          className="primary"
          disabled={blocked}
          data-tip={
            missing.length > 0
              ? `Still needs: ${missing.join(", ")}`
              : leaks && !acknowledged
                ? "Read the warning above first"
                : undefined
          }
          onClick={() => void proceed()}
        >
          {saving ? "Saving…" : "Continue"}
        </button>
        <button onClick={onCancel}>Cancel</button>
      </div>
    </div>
  );
}
