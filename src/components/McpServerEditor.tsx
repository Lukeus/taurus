import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type {
  McpServerDraft,
  McpServerRef,
  McpServerView,
  McpTransport,
  McpValue,
  Scope,
} from "../lib/api";
import { plural } from "../lib/format";
import {
  blankDraft,
  blankValue,
  canKeep,
  draftFrom,
  draftProblem,
  joinCommandLine,
  keepable,
  keepsByDefault,
  splitCommandLine,
  withKeep,
} from "../lib/mcp";
import { DrawerHead } from "./Drawer";
import { Modal } from "./Modal";

/**
 * Adding or changing one MCP server.
 *
 * The form is a way into the file, not a replacement for it: a save merges one
 * entry and copies the rest of `mcp.json` through untouched, so a server written
 * here and one pasted in by hand are the same thing, and neither route can
 * destroy the other's work.
 *
 * Test is the reason this is a form at all rather than a link to an editor.
 * Before it, the only way to find out whether an entry worked was to save it,
 * reload, and read a status line — which meant a broken entry was live in the
 * meantime, and a typo cost a full round trip. Testing connects the draft in
 * isolation, reports the tools it found, and registers nothing.
 */
export function McpServerEditor({
  server,
  initial,
  taken = [],
  onClose,
  onSaved,
}: {
  /** The server being changed, or null to add one. */
  server: McpServerView | null;
  /**
   * A draft to open on, from the catalogue.
   *
   * The whole of how an installed server and one typed by hand stay the same
   * thing: the catalogue fills this in and hands it over, and everything from
   * here — the command line on screen, Test, the merge into `mcp.json` — is the
   * path an entry typed by hand already takes. There is deliberately no route
   * into the file that skips this form.
   */
  initial?: McpServerDraft | null;
  /**
   * The servers already configured. A save is an upsert by name, so a new
   * entry, or a rename, that lands on one of these would replace it. The
   * backend refuses that too; this says so while the name is being typed.
   */
  taken?: McpServerRef[];
  onClose: () => void;
  onSaved: (servers: McpServerView[]) => void;
}) {
  const [draft, setDraft] = useState<McpServerDraft>(
    server ? draftFrom(server) : (initial ?? blankDraft("global")),
  );
  /*
   * A command line as one field, because that is how server documentation is
   * written and how it arrives on the clipboard: `npx -y @scope/pkg /tmp`. Held
   * as text rather than derived from `draft.args` so that typing a space does
   * not re-render into a shape that fights the cursor; it is split on save and
   * on test, and the split is shown below the field so it is never a guess.
   */
  const [commandLine, setCommandLine] = useState(() => {
    const from = server ?? initial;
    return from ? joinCommandLine([from.command, ...from.args]) : "";
  });

  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** Asked once. Until it answers, typed tokens are written into the file. */
  const [keychain, setKeychain] = useState(false);

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

  const stdio = draft.transport === "stdio";
  const values = stdio ? draft.env : draft.headers;
  const change = (patch: Partial<McpServerDraft>) => {
    setDraft((d) => ({ ...d, ...patch }));
    // A change invalidates the last answer. Leaving a green "8 tools" above a
    // field someone has since edited is the one thing a Test button must not do.
    setResult(null);
    setError(null);
  };
  const setValues = (next: McpValue[]) =>
    change(stdio ? { env: next } : { headers: next });

  /** The draft as it would be written, with the command line split. */
  const resolved = (): McpServerDraft => {
    if (!stdio) return draft;
    const [command = "", ...args] = splitCommandLine(commandLine);
    return { ...draft, command, args };
  };

  const clash = taken.find(
    (t) =>
      t.scope === draft.scope &&
      t.name === draft.name.trim() &&
      !(server && server.scope === t.scope && server.name === t.name),
  );
  const problem =
    draftProblem(resolved()) ??
    (clash
      ? `A server called ${clash.name} is already in ${
          clash.scope === "global" ? "~/.taurus/mcp.json" : ".taurus/mcp.json"
        }. Give this one another name, or edit that one.`
      : null);

  /*
   * The entry being edited, sent on every save and every test rather than only
   * on a rename. It is what the backend reads the held-back secrets from, and
   * what it removes when the draft has moved — so a rename, a change of layer,
   * and neither are all one code path. Sending it only when something changed
   * meant a plain save could not find the token it was meant to keep.
   */
  const previous: McpServerRef | undefined = server
    ? { scope: server.scope, name: server.name }
    : undefined;

  const test = async () => {
    setTesting(true);
    setResult(null);
    setError(null);
    try {
      setResult(await api.testMcpServer(resolved(), previous));
    } catch (e) {
      setError(String(e));
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      onSaved(await api.saveMcpServer(withKeep(resolved(), keychain), previous));
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  };

  const parts = splitCommandLine(commandLine);

  return (
    <Modal onClose={onClose} className="scrim modal-scrim">
      <div className="modal mcp-editor" onClick={(e) => e.stopPropagation()}>
        <DrawerHead title={server ? `Edit ${server.name}` : "Add MCP server"} onClose={onClose} />

        <div className="agent-fields">
          <div className="field-row">
            <label className="field">
              <span className="micro">Name</span>
              <input
                className="font-mono"
                value={draft.name}
                placeholder="filesystem"
                aria-label="Name"
                onChange={(e) => change({ name: e.target.value })}
              />
              <span className="hint font-mono">
                Its tools are called mcp__{draft.name.trim() || "name"}__…
              </span>
            </label>
            <label className="field narrow">
              <span className="micro">Applies to</span>
              <select
                value={draft.scope}
                onChange={(e) => change({ scope: e.target.value as Scope })}
              >
                <option value="global">All projects</option>
                <option value="workspace">This project</option>
              </select>
              <span className="hint">
                {draft.scope === "workspace"
                  ? "Written to .taurus/mcp.json, which is committed"
                  : "Written to ~/.taurus/mcp.json"}
              </span>
            </label>
          </div>

          <div className="field">
            <span className="micro">Reached by</span>
            <div className="pill-row">
              {(["stdio", "http"] as McpTransport[]).map((kind) => (
                <button
                  key={kind}
                  className={`pill${draft.transport === kind ? " on" : ""}`}
                  aria-pressed={draft.transport === kind}
                  onClick={() => change({ transport: kind })}
                >
                  {kind === "stdio" ? "Running a program" : "An HTTP endpoint"}
                </button>
              ))}
            </div>
          </div>

          {stdio ? (
            <label className="field">
              <span className="micro">Command</span>
              <input
                className="font-mono"
                value={commandLine}
                placeholder="npx -y @modelcontextprotocol/server-filesystem /tmp"
                aria-label="Command"
                onChange={(e) => {
                  setCommandLine(e.target.value);
                  setResult(null);
                  setError(null);
                }}
              />
              {/* The split, shown rather than assumed. Quoting is honoured, so a
                  path with a space in it is one argument — and this is where you
                  see that it was. */}
              {parts.length > 0 ? (
                <div className="tool-chips">
                  {parts.map((part, i) => (
                    <span
                      key={`${i}-${part}`}
                      className={`tool-chip${i === 0 ? " lead" : ""}`}
                    >
                      {part}
                    </span>
                  ))}
                </div>
              ) : (
                <span className="hint">
                  Paste the whole line from the server's README. Quote anything
                  with a space in it.
                </span>
              )}
            </label>
          ) : (
            <label className="field">
              <span className="micro">URL</span>
              <input
                className="font-mono"
                value={draft.url}
                placeholder="https://mcp.example.com/mcp"
                aria-label="URL"
                onChange={(e) => change({ url: e.target.value })}
              />
              <span className="hint">
                Streamable HTTP. A `${"{VAR}"}` here is read from the environment.
              </span>
            </label>
          )}

          <ValueEditor
            legend={stdio ? "Environment variables" : "Headers"}
            hint={
              stdio
                ? canKeep(draft, keychain)
                  ? "Passed to the program. A token typed here with Keychain ticked is saved to your keychain on save, and the file gets ${secret:name}. ${VAR} reads one from your environment instead."
                  : "Passed to the program. A token typed here is written into the file: a project's own program can't use a saved credential. Write ${VAR} to read it from your environment instead."
                : canKeep(draft, keychain)
                  ? "Sent with every request. A token typed here with Keychain ticked is saved to your keychain for this server's host, and the file gets `Bearer ${secret:name}`."
                  : "Sent with every request. Write `Bearer ${VAR}` to read a token from your environment instead of this file."
            }
            values={values}
            onChange={setValues}
            keychain={canKeep(draft, keychain)}
          />
        </div>

        <footer className="agent-editor-foot">
          <button disabled={testing || saving || !!problem} onClick={test}>
            {testing ? "Connecting…" : "Test"}
          </button>

          {/* The whole point of testing here rather than after a save: the
              answer sits beside the fields that produced it. */}
          {result && (
            <span className="test-result ok">
              Connected · {plural(result.length, "tool")}
              {result.length > 0 && `: ${result.slice(0, 4).join(", ")}`}
              {result.length > 4 && ` +${result.length - 4} more`}
            </span>
          )}
          {!result && (error || problem) && (
            <span className="test-result error">{error ?? problem}</span>
          )}
          {!result && !error && !problem && (
            <span className="hint">
              Test connects without saving and registers nothing.
            </span>
          )}

          <button className="primary" disabled={saving || !!problem} onClick={save}>
            {saving ? "Saving…" : server ? "Save and reconnect" : "Add and connect"}
          </button>
        </footer>
      </div>
    </Modal>
  );
}

/**
 * The name/value rows behind `env` and `headers`.
 *
 * One control for both, because they are the same idea written under two keys —
 * things the server needs that are not part of its address.
 *
 * A stored literal never comes back from the backend, so a row for one shows
 * that it is set and nothing else. Leaving it alone keeps it; typing replaces
 * it. That is what stops a save the user made for an unrelated reason from
 * blanking a token the form was never given.
 */
export function ValueEditor({
  legend,
  hint,
  values,
  onChange,
  keychain = false,
}: {
  legend: string;
  hint: string;
  values: McpValue[];
  onChange: (values: McpValue[]) => void;
  /** Whether a typed token can be kept in the keychain instead of the file. */
  keychain?: boolean;
}) {
  const header = legend === "Headers";
  const patch = (index: number, next: Partial<McpValue>) =>
    onChange(values.map((v, i) => (i === index ? { ...v, ...next } : v)));

  return (
    <div className="field">
      <span className="micro">{legend}</span>
      {values.map((value, index) => (
        <div key={index} className="value-row">
          <input
            className="font-mono"
            value={value.key}
            placeholder={legend === "Headers" ? "Authorization" : "API_KEY"}
            aria-label={`${legend} name ${index + 1}`}
            onChange={(e) => patch(index, { key: e.target.value })}
          />
          <input
            className="font-mono"
            type={value.secret && value.value === "" ? "password" : "text"}
            value={value.value}
            placeholder={
              value.secret && value.value === ""
                ? "•••••••• (kept)"
                : "${GITHUB_TOKEN}"
            }
            aria-label={`${legend} value ${index + 1}`}
            onChange={(e) =>
              // Typing supersedes the held-back value; clearing the box again
              // must not silently resurrect it, so `secret` stays off.
              patch(index, { value: e.target.value, secret: false })
            }
          />
          {/* Only on a value just typed that holds something other than names.
              A reference already keeps the token out of the file, and a held-back
              literal isn't in the form to move. */}
          {keepable(value, keychain) && (
            <label className="keep-toggle" data-tip="Save this to your keychain, and write its name in the file">
              <input
                type="checkbox"
                aria-label={`Keep ${legend.toLowerCase()} value ${index + 1} in the keychain`}
                checked={value.keep ?? keepsByDefault(header, value.key)}
                onChange={(e) => patch(index, { keep: e.target.checked })}
              />
              <span>Keychain</span>
            </label>
          )}
          <button
            aria-label={`Remove ${value.key || `${legend} ${index + 1}`}`}
            onClick={() => onChange(values.filter((_, i) => i !== index))}
          >
            ✕
          </button>
        </div>
      ))}
      <button
        className="link"
        onClick={() => onChange([...values, blankValue()])}
      >
        + Add {legend === "Headers" ? "header" : "variable"}
      </button>
      <span className="hint">{hint}</span>
    </div>
  );
}
