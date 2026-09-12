import { useEffect, useState } from "react";
import type {
  AllowedRule,
  CustomTheme,
  KeyStatus,
  ProviderConfig,
  Theme,
} from "../lib/api";
import { clampIterations, DEFAULT_MAX_ITERATIONS, MAX_ITERATIONS_LIMIT } from "../lib/limits";
import { applyTheme, resolveWith } from "../lib/theme";
import { useStore } from "../state/store";
import { CopyButton } from "./CopyButton";
import { Drawer } from "./Drawer";
import { ThemeEditor } from "./ThemeEditor";
import { Problem, Problems } from "./Problem";
import * as api from "../lib/api";
import { SearchTab, useIndexBuild, CodeSearch } from "./settings/SearchTab";
import { Field, same } from "./settings/Field";
import { PermissionsTab } from "./settings/PermissionsTab";
import { type Row, newRow, rowsOf, ProviderForm, overrideOf, problemsWith, validate, blankProvider } from "./settings/ProvidersTab";

type Tab = "models" | "search" | "permissions" | "behavior" | "appearance";

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
  // Rows rather than bare providers, so each card is keyed by something that
  // stays with it. See `Row`.
  const [draft, setDraft] = useState<Row[] | null>(null);
  const [rules, setRules] = useState<AllowedRule[]>([]);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  /** The providers as last read or saved: what Save compares the draft against. */
  const [loaded, setLoaded] = useState<ProviderConfig[] | null>(null);
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
  /** Why the stored keys could not be listed, when they could not. */
  const [keysUnreadable, setKeysUnreadable] = useState<string | null>(null);
  const refreshKeys = () => {
    api
      .listKeyStatuses()
      .then((entries) => {
        setKeys(new Map(entries));
        setKeysUnreadable(null);
      })
      .catch((e) => {
        setKeys(new Map());
        setKeysUnreadable(String(e));
      });
  };

  /**
   * Runs a one-shot write from this drawer, and says why when it did not take.
   *
   * The Revoke button and the switches each awaited a write with nothing to
   * catch it: a failure was an unhandled rejection, and the control simply did
   * not move.
   */
  const run = (action: () => Promise<unknown>) => {
    setError(null);
    action().catch((e) => setError(String(e)));
  };

  // Held here rather than in `CodeSearch`, which unmounts with its tab. See
  // `useIndexBuild`.
  const index = useIndexBuild();

  useEffect(() => {
    api
      .listGlobalProviders()
      .then((list) => {
        setDraft(rowsOf(list));
        setLoaded(list);
      })
      .catch((e) => setError(String(e)));
    api.listPermissionRules().then(setRules).catch(() => setRules([]));
    api.keychainAvailable().then(setKeychain).catch(() => setKeychain(false));
    refreshKeys();
  }, []);

  const providers = draft?.map((r) => r.provider) ?? null;
  const problems = providers ? validate(providers) : [];
  // Against what is on disk, not whether a save has happened yet: `!saved`
  // was true the moment the drawer opened, so Save was live before a single
  // field had moved.
  const dirty = providers !== null && loaded !== null && !same(providers, loaded);

  const update = (row: number, patch: Partial<ProviderConfig>) => {
    setDraft((d) =>
      d
        ? d.map((r) =>
            r.row === row ? { ...r, provider: { ...r.provider, ...patch } } : r,
          )
        : d,
    );
    setSaved(false);
  };

  const save = async () => {
    if (!providers || problems.length > 0) return;
    setSaving(true);
    setError(null);
    try {
      await api.saveProviders(providers);
      await refresh();
      const reloaded = await api.listGlobalProviders();
      // The same rows carried over, so a card left open stays open. See
      // `rowsOf`.
      setDraft((d) => rowsOf(reloaded, d));
      setLoaded(reloaded);
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
    <Drawer title="Settings" onClose={onClose}>
      <div className="pill-row" role="tablist" aria-label="Settings">
        {TABS.map(([value, label]) => (
          <button
            key={value}
            role="tab"
            aria-selected={tab === value}
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
            <Problems problems={providerProblems.map((p) => p.message)} />
          )}

          <div className="card-list">
            {draft?.map(({ row, provider }) => (
              <ProviderForm
                key={row}
                provider={provider}
                problems={problemsWith(provider, providers ?? [])}
                overriddenBy={overrideOf(provider, status?.providers ?? [])}
                keyStatus={keys.get(provider.id)}
                keychainAvailable={keychain}
                keysUnreadable={keysUnreadable}
                onKeyChanged={refreshKeys}
                onChange={(patch) => update(row, patch)}
                onRemove={() => {
                  setDraft((d) => d?.filter((r) => r.row !== row) ?? d);
                  setSaved(false);
                }}
              />
            ))}

            <button
              className="card-add"
              onClick={() => {
                setDraft((d) => [
                  ...(d ?? []),
                  newRow(blankProvider(d?.map((r) => r.provider) ?? [])),
                ]);
                setSaved(false);
              }}
            >
              Add a provider
            </button>
          </div>

          <div className="settings-actions">
            {saved && !saving && <span className="text-dim text-12">Saved</span>}
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
            <Problem key={problem}>{problem}</Problem>
          ))}
          {error && <Problem>{error}</Problem>}
        </>
      )}

      {tab === "search" && (
        <>
          <SearchTab />
          <CodeSearch
            model={status?.settings.embedding_model ?? ""}
            provider={status?.settings.embedding_provider ?? ""}
            rerankModel={status?.settings.rerank_model ?? ""}
            rerankProvider={status?.settings.rerank_provider ?? ""}
            progress={index.progress}
            outcome={index.outcome}
            onBuild={index.onBuild}
          />
        </>
      )}

      {tab === "permissions" && (
        <PermissionsTab
          rules={rules}
          error={error}
          onRevoke={(allowed) =>
            run(async () => {
              await api.revokePermissionRule(allowed.rule, allowed.scope);
              setRules(await api.listPermissionRules());
            })
          }
        />
      )}

      {tab === "behavior" && (
        <>
          <label className="settings-check">
            <input
              type="checkbox"
              checked={status?.settings.skill_synthesis_enabled ?? true}
              onChange={(e) => {
                const on = e.target.checked;
                run(async () => {
                  await api.setSkillSynthesis(on);
                  await refresh();
                });
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

          <label className="settings-check">
            <input
              type="checkbox"
              checked={status?.settings.agent_synthesis_enabled ?? true}
              onChange={(e) => {
                const on = e.target.checked;
                run(async () => {
                  await api.setAgentSynthesis(on);
                  await refresh();
                });
              }}
            />
            <span>
              Let Taurus propose sub-agents
              <span className="hint">
                It offers a delegate for work that recurs. It can never be
                given a tool you do not have, and nothing is saved without
                your approval.
              </span>
            </span>
          </label>

          <IterationLimit
            limit={status?.settings.max_iterations ?? DEFAULT_MAX_ITERATIONS}
          />
          {error && <Problem>{error}</Problem>}
        </>
      )}

      {tab === "appearance" && (
        <ThemePicker theme={status?.settings.theme ?? "system"} />
      )}

      <section className="section">
        <span className="micro">Files</span>
        <dl className="settings-paths m-0 grid grid-cols-[88px_1fr] gap-x-3 gap-y-1.5 text-12-5 [&_dt]:text-faint [&_dd]:m-0 [&_dd]:wrap-anywhere [&_dd]:font-mono [&_dd]:text-12 [&_dd]:text-dim">
          <dt>Config</dt>
          <dd>~/.taurus</dd>
          <dt>This project</dt>
          <dd>{status ? `${status.workspace}/.taurus` : "—"}</dd>
        </dl>
      </section>
    </Drawer>
  );
}

/**
 * How many model turns one message may take.
 *
 * Committed on blur or Enter rather than per keystroke: typing `50` passes
 * through `5`, and writing that would drop the limit to five for as long as it
 * took to type the second digit. Out-of-range input is pulled back into range
 * on commit rather than rejected, so the field always shows what will actually
 * be used.
 */
export function IterationLimit({ limit }: { limit: number }) {
  const refresh = useStore((s) => s.refresh);
  const [draft, setDraft] = useState(String(limit));

  // The store is the authority: a refresh from anywhere else — another pane,
  // a hand-edited settings file — has to win over a stale draft.
  useEffect(() => setDraft(String(limit)), [limit]);

  const commit = async () => {
    const next = clampIterations(draft, limit);
    setDraft(String(next));
    if (next === limit) return;
    await api.setMaxIterations(next);
    await refresh();
  };

  return (
    <Field
      label="Steps per message"
      hint={`How many times the model may act before Taurus stops the turn. Raise it for long refactors; lower it to catch a model going in circles sooner. 1–${MAX_ITERATIONS_LIMIT}, ${DEFAULT_MAX_ITERATIONS} by default.`}
    >
      <input
        inputMode="numeric"
        aria-label="Steps per message"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
      />
    </Field>
  );
}

/**
 * Light, dark, or whatever the machine is doing — and whose colours those are.
 *
 * Two rows, because they are two questions. The first is the mode, and it is
 * painted before the write lands, deliberately: a theme change is the one
 * setting whose result is the screen itself, and waiting a round trip to see it
 * makes the app feel like it did not hear you. The write is still the authority
 * — `refresh` follows it, and App repaints from whatever settings actually say,
 * so a failed write corrects the optimism rather than leaving it.
 *
 * The second is which brand is painting over it. Kept apart from the mode
 * rather than folded into one list of options, because folding them means
 * picking a brand throws away "follow the system" — which is both the default
 * and the only setting here that can change on its own, at dusk, without
 * anybody touching it.
 */
export function ThemePicker({ theme }: { theme: Theme }) {
  const status = useStore((s) => s.status);
  const refresh = useStore((s) => s.refresh);
  const custom = status?.theme ?? null;

  const [themes, setThemes] = useState<CustomTheme[] | null>(null);
  /** The theme the editor is open on, or `{ theme: null }` for a new one. */
  const [editing, setEditing] = useState<{ theme: CustomTheme | null } | null>(null);
  const [arming, setArming] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    api
      .listThemes()
      .then(setThemes)
      .catch(() => setThemes([]));
  };
  useEffect(load, []);

  const chooseMode = async (next: Theme) => {
    applyTheme(next, custom);
    await api.setTheme(next);
    await refresh();
  };

  const chooseBrand = async (id: string) => {
    setArming(false);
    const picked = themes?.find((t) => t.id === id) ?? null;
    applyTheme(theme, picked);
    await api.setThemeId(id);
    await refresh();
  };

  const remove = async () => {
    if (!custom) return;
    setArming(false);
    try {
      await api.deleteTheme(custom.scope, custom.id);
      await refresh();
      load();
    } catch (e) {
      setError(String(e));
    }
  };

  /**
   * Creates the themes folder if it is not there, and says where it is.
   *
   * It says rather than opens. The webview is granted almost nothing on
   * purpose — "the harness reaches the filesystem and processes through Rust,
   * not through frontend plugins", as the capabilities file puts it — and
   * launching a file manager would mean either a new permission on the window
   * or a process spawn in Rust, neither of which is worth what it buys over a
   * path you can copy. Creating the directory is the part that actually helps:
   * on a machine that has never had one, an editor opening on nothing is the
   * usual way this feature looks broken.
   */
  const [folder, setFolder] = useState<string | null>(null);
  const showFolder = async () => {
    try {
      setFolder(await api.themesDir("global"));
    } catch (e) {
      setError(String(e));
    }
  };

  // Everything a theme file said that the app could not use. Shown here rather
  // than only in the app-wide list because this is the screen that can fix it,
  // which is what `ProblemSource::where_to_fix` says about them.
  const problems = (status?.problems ?? []).filter((p) => p.source === "themes");

  const pinned = custom !== null && custom.modes !== "both";

  return (
    <>
      <section className="section">
        <span className="micro">Appearance</span>
        <div className="pill-row" role="radiogroup" aria-label="Theme">
          {THEMES.map(([value, label]) => (
            <button
              key={value}
              role="radio"
              aria-checked={theme === value}
              className={`pill${theme === value ? " on" : ""}`}
              /* A theme that fills in only one palette has said which one it
                 is. Leaving the other two live would leave two controls that
                 look available and do nothing — so they say why instead. */
              disabled={pinned}
              data-tip={
                pinned
                  ? `${custom!.name} only defines a ${custom!.modes === "dark_only" ? "dark" : "light"} palette`
                  : undefined
              }
              onClick={() => void chooseMode(value)}
            >
              {label}
            </button>
          ))}
        </div>
        <p className="hint">
          {pinned
            ? `${custom!.name} defines only a ${
                custom!.modes === "dark_only" ? "dark" : "light"
              } palette, so it paints that way whatever the system does. Give it the other one to get this choice back.`
            : theme === "system"
              ? "Follows your system setting, including when it changes at sunset."
              : "Stays this way in every workspace, whatever the system does."}
        </p>
      </section>

      <section className="section">
        <span className="micro">Brand</span>
        <div className="pill-row" role="radiogroup" aria-label="Brand">
          <button
            role="radio"
            aria-checked={custom === null}
            className={`pill${custom === null ? " on" : ""}`}
            onClick={() => void chooseBrand("")}
          >
            Taurus
          </button>
          {(themes ?? []).map((t) => (
            <button
              key={t.id}
              role="radio"
              aria-checked={custom?.id === t.id}
              className={`pill${custom?.id === t.id ? " on" : ""}`}
              data-tip={
                t.scope === "workspace"
                  ? "From this workspace's .taurus/themes"
                  : t.path
              }
              onClick={() => void chooseBrand(t.id)}
            >
              {t.name}
            </button>
          ))}
        </div>

        <div className="settings-actions">
          <button onClick={() => setEditing({ theme: null })}>New theme</button>
          {custom !== null && (
            <button onClick={() => setEditing({ theme: custom })}>
              Edit {custom.name}
            </button>
          )}
          {custom !== null &&
            /* Arms rather than acts. It is one file and it is the only
               irreversible thing on this screen, sitting in a row of controls
               that merely switch between palettes. */
            (arming ? (
              <>
                <button className="danger" onClick={() => void remove()}>
                  Delete {custom.name}
                </button>
                <button onClick={() => setArming(false)}>Keep it</button>
              </>
            ) : (
              <button onClick={() => setArming(true)}>Delete</button>
            ))}
          <button onClick={() => void showFolder()}>Themes folder</button>
        </div>

        {folder && (
          <p className="hint">
            <code>{folder}</code>{" "}
            <CopyButton className="link" text={folder} label="copy path" />
          </p>
        )}

        <p className="hint">
          A theme is a file in <code>~/.taurus/themes</code> — fourteen colours,
          three typefaces, a wordmark and a corner radius. Everything it leaves
          out stays as the app ships it, so changing one accent is four lines.
          A workspace can carry its own in <code>.taurus/themes</code>, which is
          how a repository brands the app for everyone who opens it.
        </p>

        {problems.map((problem) => (
          <Problem key={problem.message}>{problem.message}</Problem>
        ))}
        {error && <Problem>{error}</Problem>}
      </section>

      {editing && (
        <ThemeEditor
          editing={editing.theme}
          mode={resolveWith(theme, custom)}
          onClose={() => setEditing(null)}
          onSaved={(id) => {
            setEditing(null);
            load();
            void chooseBrand(id);
          }}
        />
      )}
    </>
  );
}

const THEMES: [Theme, string][] = [
  ["system", "System"],
  ["light", "Light"],
  ["dark", "Dark"],
];

const TABS: [Tab, string][] = [
  ["models", "Models"],
  ["search", "Search"],
  ["permissions", "Permissions"],
  ["behavior", "Behavior"],
  // Its own tab rather than the last section of Behavior, which is where it
  // started. Behavior is what the agent is allowed to do on its own — propose
  // skills, propose sub-agents, take this many steps — and how the window is
  // painted is not one of those. It was three pills when it was filed there;
  // it is now a mode, a brand, an editor and whatever a theme file got wrong,
  // and a tab whose name does not predict its contents is the same problem the
  // rail had with "Tools".
  ["appearance", "Appearance"],
];

/*
 * Settings is mostly other people's furniture.
 *
 * `card`, `drawer`, `micro`, `spacer` are rules shared with a dozen panels and
 * stay rules — and so, less obviously, do `settings-field`, `settings-actions`
 * and `settings-check`, whose prefix says this drawer owns them and is wrong:
 * the theme editor lays out its fields with the first. What is converted here
 * is only what Settings actually owns. The fourth of them, the sentence saying
 * why something failed, was shared by eleven panels and is now `Problem`.
 */

