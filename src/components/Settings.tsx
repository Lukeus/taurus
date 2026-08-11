import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { AllowedRule, ProviderConfig, ProviderKind, Scope } from "../lib/api";
import { useStore } from "../state/store";

/**
 * The settings drawer.
 *
 * Everything here is also a file under `~/.taurus` that can be hand-edited,
 * and that shapes the design: the editor works on a draft and writes only on
 * Save, so a half-typed URL never reaches disk, and it edits the *global*
 * layer while showing where this workspace overrides it. Silently folding a
 * project's override into the global file is the one destructive thing a
 * settings screen over layered config can do.
 */
export function Settings({ onClose }: { onClose: () => void }) {
  const status = useStore((s) => s.status);
  const refresh = useStore((s) => s.refresh);

  const [draft, setDraft] = useState<ProviderConfig[] | null>(null);
  const [rules, setRules] = useState<AllowedRule[]>([]);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.listGlobalProviders().then(setDraft).catch((e) => setError(String(e)));
    api.listPermissionRules().then(setRules).catch(() => setRules([]));
  }, []);

  const problems = draft ? validate(draft) : [];
  const dirty = draft !== null && saved === false;

  const update = (index: number, patch: Partial<ProviderConfig>) => {
    setDraft((d) =>
      d ? d.map((p, i) => (i === index ? { ...p, ...patch } : p)) : d,
    );
    setSaved(false);
  };

  const save = async () => {
    if (!draft || problems.length > 0) return;
    setSaving(true);
    setError(null);
    try {
      await api.saveProviders(draft);
      await refresh();
      setDraft(await api.listGlobalProviders());
      setSaved(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="scrim" onClick={onClose}>
      <aside className="drawer" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-head">
          <h2>Settings</h2>
          <button onClick={onClose}>Close</button>
        </header>

        <section className="settings-section">
          <h3>Providers</h3>
          <p className="muted small">
            Saved to <code>~/.taurus/providers.json</code>, shared with the CLI.
            API keys are never stored here — name the environment variable that
            holds one.
          </p>

          {draft?.map((provider, index) => (
            <ProviderForm
              key={index}
              provider={provider}
              overriddenBy={overrideOf(provider, status?.providers ?? [])}
              onChange={(patch) => update(index, patch)}
              onRemove={() => {
                setDraft((d) => d?.filter((_, i) => i !== index) ?? d);
                setSaved(false);
              }}
            />
          ))}

          {draft?.length === 0 && (
            <p className="muted drawer-empty">
              No providers configured. Add one to get started.
            </p>
          )}

          <div className="settings-actions">
            <button
              onClick={() => {
                setDraft((d) => [...(d ?? []), blankProvider(d ?? [])]);
                setSaved(false);
              }}
            >
              Add provider
            </button>
            <div className="spacer" />
            {saved && !saving && <span className="muted small">Saved</span>}
            <button
              className="primary"
              onClick={save}
              disabled={saving || !dirty || problems.length > 0 || !draft}
            >
              {saving ? "Saving…" : "Save"}
            </button>
          </div>

          {problems.map((problem) => (
            <p key={problem} className="settings-problem small">
              {problem}
            </p>
          ))}
          {error && <p className="settings-problem small">{error}</p>}
        </section>

        <section className="settings-section">
          <h3>Behavior</h3>
          <label className="settings-check">
            <input
              type="checkbox"
              checked={status?.settings.skill_synthesis_enabled ?? true}
              onChange={async (e) => {
                await api.setSkillSynthesis(e.target.checked);
                await refresh();
              }}
            />
            <span>
              Let Taurus propose skills
              <span className="muted small">
                {" "}
                — it offers a procedure it worked out; nothing is saved without
                your approval.
              </span>
            </span>
          </label>
        </section>

        <section className="settings-section">
          <h3>Permissions</h3>
          {rules.length === 0 ? (
            <p className="muted drawer-empty">
              Nothing has been granted permanently. Approvals you mark “always”
              will appear here.
            </p>
          ) : (
            <ul className="skill-list">
              {rules.map((allowed) => (
                <li key={`${allowed.scope}:${allowed.rule}`}>
                  <div className="skill-row">
                    <code>{allowed.rule}</code>
                    <span className={`tag ${allowed.scope}`}>
                      {SCOPE_LABEL[allowed.scope]}
                    </span>
                    <div className="spacer" />
                    <button
                      className="danger"
                      onClick={async () => {
                        await api.revokePermissionRule(
                          allowed.rule,
                          allowed.scope,
                        );
                        setRules(await api.listPermissionRules());
                      }}
                    >
                      Revoke
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="settings-section">
          <h3>Locations</h3>
          <dl className="settings-paths">
            <dt>Workspace</dt>
            <dd>{status?.workspace ?? "—"}</dd>
            <dt>Config &amp; skills</dt>
            <dd>~/.taurus</dd>
            <dt>This project</dt>
            <dd>{status ? `${status.workspace}/.taurus` : "—"}</dd>
          </dl>
        </section>
      </aside>
    </div>
  );
}

const SCOPE_LABEL: Record<Scope, string> = {
  global: "every workspace",
  workspace: "this workspace",
};

const KINDS: { value: ProviderKind; label: string }[] = [
  { value: "ollama", label: "Ollama" },
  { value: "open_ai_compatible", label: "OpenAI-compatible" },
];

function ProviderForm({
  provider,
  overriddenBy,
  onChange,
  onRemove,
}: {
  provider: ProviderConfig;
  overriddenBy: string[];
  onChange: (patch: Partial<ProviderConfig>) => void;
  onRemove: () => void;
}) {
  // Only the OpenAI-compatible adapter has these; Ollama probes its own
  // capabilities per model, so showing them there would invite settings that
  // are silently ignored.
  const compatible = provider.kind === "open_ai_compatible";

  return (
    <div className="settings-provider">
      <div className="skill-row">
        <input
          className="settings-id"
          value={provider.id}
          aria-label="Provider id"
          placeholder="id"
          onChange={(e) => onChange({ id: e.target.value })}
        />
        <select
          value={provider.kind}
          aria-label="Provider kind"
          onChange={(e) => onChange({ kind: e.target.value as ProviderKind })}
        >
          {KINDS.map((kind) => (
            <option key={kind.value} value={kind.value}>
              {kind.label}
            </option>
          ))}
        </select>
        <div className="spacer" />
        <button className="danger" onClick={onRemove}>
          Remove
        </button>
      </div>

      <Field label="Base URL">
        <input
          value={provider.base_url}
          placeholder="http://localhost:11434"
          onChange={(e) => onChange({ base_url: e.target.value })}
        />
      </Field>

      <Field label="Default model">
        <input
          value={provider.default_model ?? ""}
          placeholder="optional"
          onChange={(e) => onChange({ default_model: blank(e.target.value) })}
        />
      </Field>

      {compatible && (
        <>
          <Field
            label="API key variable"
            hint="The name of an environment variable. The key itself is never written to disk."
          >
            <input
              value={provider.api_key_env ?? ""}
              placeholder="e.g. OPENAI_API_KEY"
              onChange={(e) => onChange({ api_key_env: blank(e.target.value) })}
            />
          </Field>

          <Field
            label="API key header"
            hint="Leave blank for Authorization: Bearer. Azure OpenAI reads api-key; an Azure APIM gateway reads Ocp-Apim-Subscription-Key."
          >
            <input
              value={provider.api_key_header ?? ""}
              placeholder="Authorization: Bearer"
              onChange={(e) => onChange({ api_key_header: blank(e.target.value) })}
            />
          </Field>

          <Field
            label="API prefix"
            hint="Defaults to /v1. Azure OpenAI behind APIM usually needs /openai/v1; OpenVINO Model Server before 2026.3 needs /v3."
          >
            <input
              value={provider.api_prefix ?? ""}
              placeholder="/v1"
              onChange={(e) => onChange({ api_prefix: blank(e.target.value) })}
            />
          </Field>

          <Field
            label="Tool calling"
            hint="These cannot be probed over the OpenAI API, so they have to be declared."
          >
            <select
              value={triState(provider.native_tools)}
              onChange={(e) =>
                onChange({ native_tools: fromTriState(e.target.value) })
              }
            >
              <option value="auto">Assume supported</option>
              <option value="yes">Native</option>
              <option value="no">Prompted — model has no tool support</option>
            </select>
          </Field>

          <Field
            label="Context length"
            hint="Drives when history is compacted. 8192 for OpenVINO on NPU."
          >
            <input
              inputMode="numeric"
              value={provider.context_length ?? ""}
              placeholder="128000"
              onChange={(e) =>
                onChange({ context_length: parseContextLength(e.target.value) })
              }
            />
          </Field>
        </>
      )}

      {overriddenBy.length > 0 && (
        <p className="settings-note small">
          This workspace overrides {listSentence(overriddenBy)} in its own{" "}
          <code>.taurus/providers.json</code>. Changes here apply everywhere
          else; the override still wins in this project.
        </p>
      )}
    </div>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="settings-field">
      <span className="settings-label">{label}</span>
      {children}
      {hint && <span className="muted small">{hint}</span>}
    </label>
  );
}

/** An empty box means "unset", not an empty string. */
function blank(value: string): string | null {
  return value.trim() === "" ? null : value;
}

/**
 * Keeps a half-typed number from clearing a configured one.
 *
 * Returning `null` for unparseable input would delete the value the moment
 * someone selected it to retype, so only a genuinely empty box unsets it.
 */
export function parseContextLength(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const parsed = Number.parseInt(trimmed, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

function triState(value: boolean | null): string {
  if (value === null) return "auto";
  return value ? "yes" : "no";
}

function fromTriState(value: string): boolean | null {
  if (value === "auto") return null;
  return value === "yes";
}

/**
 * Which fields this workspace overrides for a given global provider.
 *
 * Compares the entry as stored globally against the effective one the host
 * resolved. Anything that differs came from the workspace layer, and the user
 * needs to know that editing here will not change what this project uses.
 */
export function overrideOf(
  global: ProviderConfig,
  effective: ProviderConfig[],
): string[] {
  const match = effective.find((p) => p.id === global.id);
  if (!match) return [];
  const fields: (keyof ProviderConfig)[] = [
    "kind",
    "base_url",
    "default_model",
    "api_key_env",
    "api_key_header",
    "native_tools",
    "context_length",
    "api_prefix",
  ];
  return fields.filter((field) => (global[field] ?? null) !== (match[field] ?? null));
}

/** Duplicate ids and missing required fields, as sentences. */
export function validate(providers: ProviderConfig[]): string[] {
  const problems: string[] = [];
  const seen = new Set<string>();

  for (const provider of providers) {
    const id = provider.id.trim();
    if (id === "") {
      problems.push("Every provider needs an id.");
    } else if (seen.has(id)) {
      problems.push(`Two providers share the id "${id}".`);
    } else {
      seen.add(id);
    }
    if (provider.base_url.trim() === "") {
      problems.push(`"${id || "A provider"}" needs a base URL.`);
    }
  }
  // Same problem stated twice reads as two problems.
  return [...new Set(problems)];
}

/** A new entry that will not collide with an existing id. */
export function blankProvider(existing: ProviderConfig[]): ProviderConfig {
  const taken = new Set(existing.map((p) => p.id));
  let id = "new-provider";
  for (let n = 2; taken.has(id); n++) id = `new-provider-${n}`;
  return {
    id,
    kind: "open_ai_compatible",
    base_url: "",
    default_model: null,
    api_key_env: null,
    api_key_header: null,
    native_tools: null,
    context_length: null,
    api_prefix: null,
  };
}

function listSentence(items: string[]): string {
  const pretty = items.map((i) => i.replace(/_/g, " "));
  if (pretty.length === 1) return pretty[0];
  return `${pretty.slice(0, -1).join(", ")} and ${pretty[pretty.length - 1]}`;
}
