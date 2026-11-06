// Saved credentials, on the Profile tab: tokens for MCP servers, kept in the
// keychain and named from mcp.json as ${secret:name}.

import { useEffect, useState } from "react";

import * as api from "../../lib/api";
import type { CredentialView } from "../../lib/api";
import { CopyButton } from "../CopyButton";
import { Problem } from "../Problem";
import { Field } from "./Field";

/** Hosts as typed into one box: commas, spaces or new lines between them. */
export function splitHosts(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((h) => h.trim())
    .filter(Boolean);
}

/** What a credential's host list means, in words. */
export function reach(hosts: string[]): string {
  return hosts.length === 0
    ? "Any host, from your own mcp.json only"
    : `Only ${hosts.join(", ")}`;
}

/**
 * The list, and the form that adds to it.
 *
 * A token is never shown or sent back: the list says whether the keychain
 * holds one, and editing a credential leaves its token alone unless a new one
 * is pasted. The same rule the MCP panel keeps for the values it holds back.
 */
export function CredentialsSection() {
  const [list, setList] = useState<CredentialView[] | null>(null);
  const [keychain, setKeychain] = useState(true);
  const [error, setError] = useState<string | null>(null);
  /** The credential being edited, or "" for a new one, or null for neither. */
  const [editing, setEditing] = useState<string | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);

  const load = async () => {
    try {
      const [loaded, available] = await Promise.all([
        api.listCredentials(),
        api.keychainAvailable(),
      ]);
      setList(loaded);
      setKeychain(available);
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const remove = async (name: string) => {
    setError(null);
    try {
      await api.removeCredential(name);
      setRemoving(null);
      await load();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <section className="section credentials">
      <span className="micro">Saved credentials</span>
      <p className="hint m-0">
        Tokens for MCP servers, kept in your keychain instead of a file. Write{" "}
        <code>{"${secret:name}"}</code> in a server's environment or headers to use one.
        Each is sent only to the hosts you list. With none listed, only servers in your own
        mcp.json get it. A project's own program never does.
      </p>

      {!keychain && (
        <Problem>
          This system has no keychain Taurus can use, so a credential can't be saved here. Write
          {" ${VAR}"} in the server's entry to read the token from your environment instead.
        </Problem>
      )}

      {list === null ? (
        !error && <p className="drawer-loading">Reading…</p>
      ) : (
        list.length > 0 && (
          <ul className="card-list">
            {list.map((credential) =>
              editing === credential.name ? (
                <li key={credential.name} className="card">
                  <CredentialForm
                    existing={credential}
                    onDone={async () => {
                      setEditing(null);
                      await load();
                    }}
                    onCancel={() => setEditing(null)}
                  />
                </li>
              ) : (
                <li key={credential.name} className="card credential-card">
                  <div className="card-body">
                    <div className="card-row">
                      <span className="card-title font-mono text-12-5 min-w-0 wrap-anywhere">
                        {credential.name}
                      </span>
                      <div className="spacer" />
                      <CopyButton
                        text={`\${secret:${credential.name}}`}
                        label="copy reference"
                      />
                      {removing !== credential.name && (
                        <>
                          <button onClick={() => setEditing(credential.name)}>Edit</button>
                          <button
                            className="danger"
                            onClick={() => setRemoving(credential.name)}
                          >
                            Remove
                          </button>
                        </>
                      )}
                    </div>
                    <p className="hint m-0">
                      {reach(credential.hosts)}.{" "}
                      {credential.used_by.length > 0
                        ? `Used by ${credential.used_by.join(", ")}.`
                        : "No server uses it."}
                    </p>
                    {!credential.stored && (
                      <Problem>
                        The keychain has nothing for this one. Edit it and paste the token again.
                      </Problem>
                    )}
                    {removing === credential.name && (
                      <div className="card-row">
                        <span className="hint">
                          {credential.used_by.length > 0
                            ? `${credential.used_by.join(", ")} will stop connecting until you give ${credential.used_by.length > 1 ? "them" : "it"} another.`
                            : "The token is deleted from your keychain."}
                        </span>
                        <div className="spacer" />
                        <button className="danger" onClick={() => void remove(credential.name)}>
                          Remove {credential.name}
                        </button>
                        <button onClick={() => setRemoving(null)}>Keep</button>
                      </div>
                    )}
                  </div>
                </li>
              ),
            )}
          </ul>
        )
      )}

      {keychain &&
        (editing === "" ? (
          <div className="card">
            <CredentialForm
              onDone={async () => {
                setEditing(null);
                await load();
              }}
              onCancel={() => setEditing(null)}
            />
          </div>
        ) : (
          <div>
            <button onClick={() => setEditing("")}>+ Add a credential</button>
          </div>
        ))}

      {error && <Problem>{error}</Problem>}
    </section>
  );
}

/** Adds a credential, or changes one's hosts and optionally its token. */
function CredentialForm({
  existing,
  onDone,
  onCancel,
}: {
  existing?: CredentialView;
  onDone: () => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState(existing?.name ?? "");
  const [token, setToken] = useState("");
  const [hosts, setHosts] = useState(existing?.hosts.join(", ") ?? "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      await api.saveCredential(name, splitHosts(hosts), token === "" ? null : token);
      await onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="card-body credential-form">
      <Field label="Name" hint="Letters, digits, - _ and . — what goes after secret: in the file.">
        <input
          className="font-mono"
          aria-label="Credential name"
          placeholder="github"
          value={name}
          // A rename would leave the old keychain entry behind, so it's a new
          // credential and a removal instead.
          disabled={existing !== undefined}
          spellCheck={false}
          onChange={(e) => setName(e.target.value)}
        />
      </Field>
      <Field
        label="Token"
        hint={
          existing
            ? "Leave empty to keep the one saved. It's never shown again."
            : "Goes straight to your keychain. It's never shown again."
        }
      >
        <input
          type="password"
          aria-label="Token"
          autoComplete="off"
          spellCheck={false}
          value={token}
          onChange={(e) => setToken(e.target.value)}
        />
      </Field>
      <Field
        label="Hosts"
        hint="Where it may be sent, like mcp.linear.app or *.example.com. A server URL pasted here is cut down to its host. Empty means any host, from your own mcp.json only."
      >
        <input
          aria-label="Hosts"
          placeholder="any host, from your own mcp.json"
          spellCheck={false}
          value={hosts}
          onChange={(e) => setHosts(e.target.value)}
        />
      </Field>
      {error && <Problem>{error}</Problem>}
      <div className="card-row">
        <button
          className="primary"
          disabled={saving || name.trim() === "" || (!existing && token === "")}
          onClick={() => void save()}
        >
          {saving ? "Saving…" : "Save"}
        </button>
        <button onClick={onCancel}>Cancel</button>
      </div>
    </div>
  );
}
