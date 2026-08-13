import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type {
  AllowedRule,
  KeyStatus,
  ModelEntry,
  ProviderConfig,
  ProviderKind,
  Scope,
  SearchBackend,
  SearchSettings,
  Theme,
} from "../lib/api";
import { applyTheme } from "../lib/theme";
import { useStore } from "../state/store";

type Tab = "models" | "search" | "permissions" | "behavior";

/**
 * The settings drawer.
 *
 * Everything here is also a file under `~/.taurus` that can be hand-edited,
 * and that shapes the design: the editor works on a draft and writes only on
 * Save, so a half-typed URL never reaches disk, and it edits the *global*
 * layer while showing where this workspace overrides it. Silently folding a
 * project's override into the global file is the one destructive thing a
 * settings screen over layered config can do.
 *
 * Each tab is a separate file's worth of state, so switching between them
 * never discards a draft — the provider draft outlives the tab.
 */
export function Settings({ onClose }: { onClose: () => void }) {
  const status = useStore((s) => s.status);
  const refresh = useStore((s) => s.refresh);

  const [tab, setTab] = useState<Tab>("models");
  const [draft, setDraft] = useState<ProviderConfig[] | null>(null);
  const [rules, setRules] = useState<AllowedRule[]>([]);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [keys, setKeys] = useState<Map<string, KeyStatus>>(new Map());
  const [keychain, setKeychain] = useState(false);

  const providerProblems = (status?.problems ?? []).filter(
    (p) => p.source === "providers",
  );

  // Re-read rather than patched locally: storing a key can change what another
  // row reports — an environment variable that was the only source becomes an
  // override — and a status the frontend guessed at would be a status that
  // disagrees with the one the request will actually use.
  const refreshKeys = () => {
    api
      .listKeyStatuses()
      .then((entries) => setKeys(new Map(entries)))
      .catch(() => setKeys(new Map()));
  };

  useEffect(() => {
    api.listGlobalProviders().then(setDraft).catch((e) => setError(String(e)));
    api.listPermissionRules().then(setRules).catch(() => setRules([]));
    api.keychainAvailable().then(setKeychain).catch(() => setKeychain(false));
    refreshKeys();
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
      // A provider that was just added or renamed only now has an id a key can
      // be stored against, so its field has to stop saying "not saved yet".
      refreshKeys();
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
          <button className="drawer-close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>

        <div className="pill-row">
          {TABS.map(([value, label]) => (
            <button
              key={value}
              className={`pill${tab === value ? " on" : ""}`}
              onClick={() => setTab(value)}
            >
              {label}
            </button>
          ))}
        </div>

        {tab === "models" && (
          <>
            <p className="drawer-intro">
              Saved to <code>~/.taurus/providers.json</code>, shared with the
              CLI. API keys are never stored here — name the environment
              variable that holds one.
            </p>

            {/* A providers.json that will not parse is why the list below can
                be empty or stale, so it belongs at the top of this tab rather
                than in a drawer about skills, where it used to appear. */}
            {providerProblems.length > 0 && (
              <section className="section">
                <span className="micro">Could not load</span>
                {providerProblems.map((problem) => (
                  <p key={problem.message} className="settings-problem">
                    {problem.message}
                  </p>
                ))}
              </section>
            )}

            <div className="card-list">
              {draft?.map((provider, index) => (
                <ProviderForm
                  key={index}
                  provider={provider}
                  overriddenBy={overrideOf(provider, status?.providers ?? [])}
                  keyStatus={keys.get(provider.id)}
                  keychainAvailable={keychain}
                  onKeyChanged={refreshKeys}
                  onChange={(patch) => update(index, patch)}
                  onRemove={() => {
                    setDraft((d) => d?.filter((_, i) => i !== index) ?? d);
                    setSaved(false);
                  }}
                />
              ))}

              <button
                className="card-add"
                onClick={() => {
                  setDraft((d) => [...(d ?? []), blankProvider(d ?? [])]);
                  setSaved(false);
                }}
              >
                Add a provider
              </button>
            </div>

            <div className="settings-actions">
              {saved && !saving && <span className="muted small">Saved</span>}
              <div className="spacer" />
              <button
                className="primary"
                onClick={save}
                disabled={saving || !dirty || problems.length > 0 || !draft}
              >
                {saving ? "Saving…" : "Save"}
              </button>
            </div>

            {problems.map((problem) => (
              <p key={problem} className="settings-problem">
                {problem}
              </p>
            ))}
            {error && <p className="settings-problem">{error}</p>}
          </>
        )}

        {tab === "search" && <SearchTab />}

        {tab === "permissions" && (
          <>
            <p className="drawer-intro">
              Approvals you marked “always”. Revoking one puts the next such
              call back in front of you.
            </p>
            {rules.length === 0 ? (
              <p className="drawer-empty">
                Nothing has been granted permanently yet.
              </p>
            ) : (
              <ul className="card-list">
                {rules.map((allowed) => (
                  <li key={`${allowed.scope}:${allowed.rule}`} className="card">
                    <div className="card-body">
                      <div className="card-row">
                        <span className="card-title mono rule-name">
                          {allowed.rule}
                        </span>
                        <span
                          className={`tag${allowed.scope === "workspace" ? " project" : ""}`}
                        >
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
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}

        {tab === "behavior" && (
          <>
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
                <span className="hint">
                  It offers a procedure it worked out; nothing is saved without
                  your approval.
                </span>
              </span>
            </label>

            <ThemePicker theme={status?.settings.theme ?? "system"} />
          </>
        )}

        <section className="section">
          <span className="micro">Files</span>
          <dl className="settings-paths">
            <dt>Config</dt>
            <dd>~/.taurus</dd>
            <dt>This project</dt>
            <dd>{status ? `${status.workspace}/.taurus` : "—"}</dd>
          </dl>
        </section>
      </aside>
    </div>
  );
}

/**
 * Light, dark, or whatever the machine is doing.
 *
 * Painted before the write lands, and deliberately: a theme change is the one
 * setting whose result is the screen itself, and waiting a round trip to see it
 * makes the app feel like it did not hear you. The write is still the authority
 * — `refresh` follows it, and App repaints from whatever settings actually say,
 * so a failed write corrects the optimism rather than leaving it.
 */
export function ThemePicker({ theme }: { theme: Theme }) {
  const refresh = useStore((s) => s.refresh);

  const choose = async (next: Theme) => {
    applyTheme(next);
    await api.setTheme(next);
    await refresh();
  };

  return (
    <section className="section">
      <span className="micro">Appearance</span>
      <div className="pill-row" role="radiogroup" aria-label="Theme">
        {THEMES.map(([value, label]) => (
          <button
            key={value}
            role="radio"
            aria-checked={theme === value}
            className={`pill${theme === value ? " on" : ""}`}
            onClick={() => choose(value)}
          >
            {label}
          </button>
        ))}
      </div>
      <p className="hint">
        {theme === "system"
          ? "Follows your system setting, including when it changes at sunset."
          : "Stays this way in every workspace, whatever the system does."}
      </p>
    </section>
  );
}

const THEMES: [Theme, string][] = [
  ["system", "System"],
  ["light", "Light"],
  ["dark", "Dark"],
];

/**
 * Web search: which backend, and the key it needs.
 *
 * Everything behind this was built and shipped some time ago — the backends,
 * the key handling, the tools — and none of it was reachable without knowing
 * `~/.taurus/search.json` existed and writing its schema by hand.
 *
 * Off is the default and stays a first-class choice, not a disabled state:
 * searching means sending the user's prompt to a third party, and that is not
 * something to slide into because a screen made it the easy option.
 */
export function SearchTab() {
  const [settings, setSettings] = useState<SearchSettings | null>(null);
  const [keychain, setKeychain] = useState(false);
  const [busy, setBusy] = useState(false);

  const load = () => api.getSearchSettings().then(setSettings).catch(() => {});

  useEffect(() => {
    load();
    api.keychainAvailable().then(setKeychain).catch(() => setKeychain(false));
  }, []);

  if (!settings) return <p className="drawer-intro">Loading…</p>;

  const keys = new Map(settings.key_statuses);
  const selected = settings.backends.find((b) => b.id === settings.selected);

  const save = async (id: string | null, backends = settings.backends) => {
    setBusy(true);
    try {
      await api.saveSearchSettings(id, backends);
      await load();
    } finally {
      setBusy(false);
    }
  };

  const patch = (id: string, change: Partial<SearchBackend>) =>
    settings.backends.map((b) => (b.id === id ? { ...b, ...change } : b));

  return (
    <>
      <p className="drawer-intro">
        Lets Taurus look things up on the web. Your prompt goes to whichever
        service you pick, so it stays off until you choose one.
      </p>

      {settings.problems.length > 0 && (
        <section className="section">
          <span className="micro">Could not load</span>
          {settings.problems.map((problem) => (
            <p key={problem.message} className="settings-problem">
              {problem.message}
            </p>
          ))}
        </section>
      )}

      <Field label="Search with" hint={statusHint(settings)}>
        <select
          value={settings.selected ?? ""}
          disabled={busy}
          onChange={(e) => save(e.target.value === "" ? null : e.target.value)}
        >
          <option value="">Off</option>
          {settings.backends.map((backend) => (
            <option key={backend.id} value={backend.id}>
              {backend.id}
            </option>
          ))}
        </select>
      </Field>

      {selected && (
        <div className="card-list">
          <div className="card">
            <div className="card-body">
              <Field
                label="Address"
                hint={
                  selected.kind === "searxng"
                    ? "Your SearXNG instance. There is no default — no instance is the canonical one."
                    : "Override only if you route through a proxy."
                }
              >
                <input
                  value={selected.base_url}
                  disabled={busy}
                  spellCheck={false}
                  placeholder={
                    selected.kind === "searxng" ? "http://localhost:8888" : ""
                  }
                  onChange={(e) =>
                    setSettings({
                      ...settings,
                      backends: patch(selected.id, { base_url: e.target.value }),
                    })
                  }
                  onBlur={() => save(settings.selected)}
                />
              </Field>

              {selected.needs_key && (
                <>
                  <ApiKeyField
                    status={keys.get(selected.id)}
                    available={keychain}
                    onStore={(key) => api.setSearchKey(selected.id, key)}
                    onClear={() => api.clearSearchKey(selected.id)}
                    onChanged={load}
                    unsavedHint="Save this backend before storing a key for it."
                  />

                  <Field
                    label="API key variable"
                    hint="Optional. Names an environment variable that overrides the stored key — useful for CI, and the only option on machines with no keychain."
                  >
                    <input
                      value={selected.api_key_env ?? ""}
                      disabled={busy}
                      spellCheck={false}
                      placeholder="BRAVE_API_KEY"
                      onChange={(e) =>
                        setSettings({
                          ...settings,
                          backends: patch(selected.id, {
                            api_key_env: blank(e.target.value),
                          }),
                        })
                      }
                      onBlur={() => save(settings.selected)}
                    />
                  </Field>
                </>
              )}
            </div>
          </div>
        </div>
      )}
    </>
  );
}

/**
 * Whether search is actually running, which is not the same as whether a
 * backend is picked — a selection with no key resolves to nothing and
 * registers no tools. Saying "on" then would be a lie the transcript
 * immediately contradicts.
 */
export function statusHint(settings: SearchSettings): string {
  if (settings.selected === null) return "Off. Taurus will not search the web.";
  if (settings.active) return `On, through ${settings.selected}.`;
  return `Selected, but not running yet — ${settings.selected} still needs something below.`;
}

const TABS: [Tab, string][] = [
  ["models", "Models"],
  ["search", "Search"],
  ["permissions", "Permissions"],
  ["behavior", "Behavior"],
];

const SCOPE_LABEL: Record<Scope, string> = {
  global: "every workspace",
  workspace: "this workspace",
};

const KINDS: { value: ProviderKind; label: string }[] = [
  { value: "ollama", label: "Ollama" },
  { value: "open_ai_compatible", label: "OpenAI-compatible" },
];

/**
 * The API key field.
 *
 * Write-only on purpose: the stored key is never sent to the frontend, so there
 * is nothing to prefill and the input starts empty every time. What replaces
 * the reassurance of seeing the old value is [`keyLine`] saying where the key
 * in use is coming from — which is the thing a user actually needs to know, and
 * the thing an obscured field full of dots cannot tell them.
 *
 * The key belongs to a *saved* id. A row that has been renamed or never saved
 * has no id to store against yet, so the field says so rather than failing on
 * the button press.
 *
 * Storing and clearing are passed in rather than chosen here: model providers
 * and search backends keep their keys in the same credential store under
 * different namespaces, and the field is identical either way.
 */
function ApiKeyField({
  status,
  available,
  onStore,
  onClear,
  onChanged,
  unsavedHint,
}: {
  status: KeyStatus | undefined;
  available: boolean;
  onStore: (key: string) => Promise<void>;
  onClear: () => Promise<void>;
  onChanged: () => void;
  unsavedHint: string;
}) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!available) {
    return (
      <Field
        label="API key"
        hint="This machine has no credential store Taurus can use, so the key has to come from the environment variable above."
      >
        <div className="settings-key-none">unavailable</div>
      </Field>
    );
  }

  // No status means this provider is not in the saved list: either it was just
  // added, or its id was edited and the old key still belongs to the old id.
  if (!status) {
    return (
      <Field label="API key" hint={unsavedHint}>
        <div className="settings-key-none">not saved yet</div>
      </Field>
    );
  }

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      setValue("");
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const stored = status.kind === "keychain" || status.kind === "overridden";

  return (
    <Field label="API key" hint={keyHint(status)}>
      <div className="settings-key">
        <input
          type="password"
          value={value}
          disabled={busy}
          placeholder={stored ? "replace the stored key" : "paste a key to store it"}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => setValue(e.target.value)}
        />
        <button
          disabled={busy || value.trim() === ""}
          onClick={() => run(() => onStore(value))}
        >
          Store
        </button>
        {stored && (
          <button className="danger" disabled={busy} onClick={() => run(onClear)}>
            Remove
          </button>
        )}
      </div>
      {error && <div className="settings-key-error">{error}</div>}
    </Field>
  );
}

/** What the field says beneath itself about where the key comes from. */
export function keyHint(status: KeyStatus): string {
  switch (status.kind) {
    case "missing":
      return "Stored in your OS keychain, never on disk and never in a config file.";
    case "keychain":
      return "Stored in your OS keychain and in use.";
    case "environment":
      return `Currently coming from $${status.variable}. A key stored here would be used only if that variable were unset.`;
    case "overridden":
      return `Stored here, but $${status.variable} is set and wins. Unset it for the stored key to take effect.`;
  }
}

function ProviderForm({
  provider,
  overriddenBy,
  keyStatus,
  keychainAvailable,
  onKeyChanged,
  onChange,
  onRemove,
}: {
  provider: ProviderConfig;
  overriddenBy: string[];
  keyStatus: KeyStatus | undefined;
  keychainAvailable: boolean;
  onKeyChanged: () => void;
  onChange: (patch: Partial<ProviderConfig>) => void;
  onRemove: () => void;
}) {
  // Only the OpenAI-compatible adapter has these; Ollama probes its own
  // capabilities per model, so showing them there would invite settings that
  // are silently ignored.
  const compatible = provider.kind === "open_ai_compatible";

  return (
    <div className="card settings-provider">
      <div className="card-row">
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

      {compatible && (
        <ModelList
          models={provider.models}
          onChange={(models) => onChange({ models })}
        />
      )}

      <Field
        label="Default model"
        hint={
          compatible
            ? "Which one a new conversation starts on. Optional — the first model above is used otherwise."
            : "Which one a new conversation starts on. Optional."
        }
      >
        <input
          value={provider.default_model ?? ""}
          placeholder="optional"
          onChange={(e) => onChange({ default_model: blank(e.target.value) })}
        />
      </Field>

      {compatible && (
        <>
          <ApiKeyField
            status={keyStatus}
            available={keychainAvailable}
            onStore={(key) => api.setProviderKey(provider.id, key)}
            onClear={() => api.clearProviderKey(provider.id)}
            onChanged={onKeyChanged}
            unsavedHint="Save this provider before storing a key for it."
          />

          <Field
            label="API key variable"
            hint="Optional. Names an environment variable that overrides the stored key — useful for CI, and the only option on machines with no keychain."
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
        <p className="settings-note">
          This workspace overrides {listSentence(overriddenBy)} in its own{" "}
          <code>.taurus/providers.json</code>. Changes here apply everywhere
          else; the override still wins in this project.
        </p>
      )}
    </div>
  );
}

/**
 * The models this endpoint serves.
 *
 * Only shown for OpenAI-compatible providers. Ollama reports its own inventory
 * *and* its own per-model capabilities, so a list here would be a second answer
 * to a question already answered correctly.
 *
 * Empty is the normal state and means "ask the endpoint". Naming anything
 * replaces that listing outright, which is what makes this usable two ways: a
 * gateway with no `/v1/models` route can finally offer more than one model, and
 * one that lists four hundred can be cut to the three there is quota for.
 *
 * The per-model overrides are here rather than only on the provider because a
 * gateway routinely fronts models that share neither a context window nor tool
 * support, and the wire format reports neither.
 */
export function ModelList({
  models,
  onChange,
}: {
  models: ModelEntry[];
  onChange: (models: ModelEntry[]) => void;
}) {
  const patch = (index: number, fields: Partial<ModelEntry>) =>
    onChange(models.map((m, i) => (i === index ? { ...m, ...fields } : m)));

  return (
    <div className="settings-field">
      <div className="section-head">
        <span className="micro">Models</span>
        <button
          className="link"
          onClick={() => onChange([...models, { id: "" }])}
        >
          Add a model
        </button>
      </div>

      <ul className="model-rows">
        {models.map((model, i) => (
          // Keyed by position because nothing else is stable: the id is the
          // field being typed into, and is empty on a row just added.
          <li key={i} className="model-row">
            <input
              className="mono"
              aria-label={`Model ${i + 1} id`}
              value={model.id}
              placeholder="gpt-4o"
              onChange={(e) => patch(i, { id: e.target.value })}
            />
            <button
              className="quiet model-remove"
              aria-label={`Remove ${model.id || `model ${i + 1}`}`}
              onClick={() => onChange(models.filter((_, n) => n !== i))}
            >
              ✕
            </button>
            <div className="model-caps">
              <input
                inputMode="numeric"
                aria-label={`Context length for ${model.id || `model ${i + 1}`}`}
                value={model.context_length ?? ""}
                placeholder="context length"
                onChange={(e) =>
                  patch(i, { context_length: parseContextLength(e.target.value) })
                }
              />
              <select
                aria-label={`Tool calling for ${model.id || `model ${i + 1}`}`}
                value={triState(model.native_tools ?? null)}
                onChange={(e) =>
                  patch(i, { native_tools: fromTriState(e.target.value) })
                }
              >
                <option value="auto">Tools: as below</option>
                <option value="yes">Tools: native</option>
                <option value="no">Tools: prompted</option>
              </select>
            </div>
          </li>
        ))}
      </ul>

      <span className="hint">
        {models.length === 0
          ? "None named, so Taurus asks this endpoint what it serves. Add one to decide the menu yourself — the only option on a gateway with no /v1/models route."
          : "These replace whatever the endpoint would list. Left blank, a model inherits the context length and tool calling set below."}
      </span>
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
      <span className="micro">{label}</span>
      {children}
      {hint && <span className="hint">{hint}</span>}
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
    "models",
    "default_model",
    "api_key_env",
    "api_key_header",
    "native_tools",
    "context_length",
    "api_prefix",
  ];
  return fields.filter((field) => !same(global[field], match[field]));
}

/**
 * Whether two config values are the same setting.
 *
 * `models` is a list, so `!==` on it reports an override for every provider
 * that has one — two equal arrays are never the same object. Compared by
 * content instead; everything else is a scalar and compares as one.
 */
function same(a: unknown, b: unknown): boolean {
  if (Array.isArray(a) || Array.isArray(b)) {
    return JSON.stringify(a ?? []) === JSON.stringify(b ?? []);
  }
  return (a ?? null) === (b ?? null);
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
    models: [],
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
