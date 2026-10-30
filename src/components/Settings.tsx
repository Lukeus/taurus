import { useEffect, useState } from "react";
import type {
  AllowedRule,
  CustomTheme,
  KeyStatus,
  ProviderConfig,
  Theme,
} from "../lib/api";
import {
  clampConcurrentAgents,
  clampContextLimit,
  clampIterations,
  DEFAULT_CONCURRENT_AGENTS,
  DEFAULT_CONTEXT_LIMIT,
  DEFAULT_MAX_ITERATIONS,
  MAX_CONCURRENT_AGENTS_LIMIT,
  MAX_ITERATIONS_LIMIT,
} from "../lib/limits";
import { applyTheme, resolveWith } from "../lib/theme";
import { useStore } from "../state/store";
import { CopyButton } from "./CopyButton";
import { Drawer } from "./Drawer";
import { ThemeEditor } from "./ThemeEditor";
import { Problem, Problems } from "./Problem";
import * as api from "../lib/api";
import { SearchTab, useIndexBuild, CodeSearch } from "./settings/SearchTab";
import { Field, FromProject, same } from "./settings/Field";
import { PermissionsTab } from "./settings/PermissionsTab";
import { VoiceTab } from "./settings/VoiceTab";
import { UpdatesSection } from "./settings/UpdatesSection";
import { ToolsTab } from "./settings/ToolsTab";
import { TracingTab } from "./settings/TracingTab";
import { HooksTab } from "./settings/HooksTab";
import { RoutingTab } from "./settings/RoutingTab";
import { TerminalTab } from "./settings/TerminalTab";
import { ProfileTab } from "./settings/ProfileTab";
import { type Row, newRow, rowsOf, ProviderForm, overrideOf, problemsWith, validate, blankProvider } from "./settings/ProvidersTab";
import { onTabKeys } from "../lib/tabs";

export type SettingsTab =
  | "profile"
  | "models"
  | "routing"
  | "search"
  | "tools"
  | "permissions"
  | "hooks"
  | "terminal"
  | "behavior"
  | "voice"
  | "appearance"
  | "tracing"
  | "about";
type Tab = SettingsTab;

/** Where the last tab looked at is kept, so the drawer reopens on it. */
const TAB_KEY = "taurus.settings.tab";

/**
 * The tab the drawer opens on: the one it was closed on, or Models.
 *
 * Per-viewer and allowed to be lost, so local storage rather than
 * settings.json, and every read survives storage that throws.
 */
function lastTab(): Tab {
  try {
    const kept = localStorage.getItem(TAB_KEY);
    return TABS.find(([value]) => value === kept)?.[0] ?? "models";
  } catch {
    return "models";
  }
}

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
 * never discards a draft — the provider draft outlives the tab. Closing does,
 * so a close with unsaved providers asks first. Each tab keeps its own error
 * too: one shared between them put a failed provider save under Behavior.
 */
export function Settings({
  onClose,
  initialTab,
}: {
  onClose: () => void;
  /**
   * The tab to open on, for a panel that sends someone here to change one
   * thing. Otherwise it's the last one looked at.
   */
  initialTab?: Tab;
}) {
  const status = useStore((s) => s.status);
  const refresh = useStore((s) => s.refresh);

  const [tab, setTabState] = useState<Tab>(() => initialTab ?? lastTab());
  const setTab = (next: Tab) => {
    setTabState(next);
    try {
      localStorage.setItem(TAB_KEY, next);
    } catch {
      // Remembering the tab is a convenience; the drawer works without it.
    }
  };
  /** True while a close is waiting on what to do with unsaved providers. */
  const [leaving, setLeaving] = useState(false);
  // Rows rather than bare providers, so each card is keyed by something that
  // stays with it. See `Row`.
  const [draft, setDraft] = useState<Row[] | null>(null);
  // Null until read, and left null when the read fails: an empty list would
  // say nothing had been granted, which a failed read can't know.
  const [rules, setRules] = useState<AllowedRule[] | null>(null);
  const [rulesError, setRulesError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  /** The providers as last read or saved: what Save compares the draft against. */
  const [loaded, setLoaded] = useState<ProviderConfig[] | null>(null);
  /** Why the provider list couldn't be read or saved. */
  const [modelsError, setModelsError] = useState<string | null>(null);
  /** Why a switch or a limit on Behavior didn't take. */
  const [behaviorError, setBehaviorError] = useState<string | null>(null);
  /** Why a revoke didn't take. */
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const [keys, setKeys] = useState<Map<string, KeyStatus>>(new Map());
  const [keychain, setKeychain] = useState(false);

  const providerProblems = (status?.problems ?? []).filter(
    (p) => p.source === "providers",
  );
  // A settings.json that won't parse: every value on Behavior is coming from
  // somewhere else until it's fixed, and every save there will be refused.
  const settingsProblems = (status?.problems ?? []).filter(
    (p) => p.source === "settings",
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
   * not move. The failure goes to the error of the tab the control is on.
   */
  const run = (
    setError: (message: string | null) => void,
    action: () => Promise<unknown>,
  ) => {
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
      .catch((e) => setModelsError(String(e)));
    api
      .listPermissionRules()
      .then(setRules)
      .catch((e) => setRulesError(`Couldn't read the permanent approvals: ${e}`));
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

  /** Saves the providers, and answers whether they were. */
  const save = async (): Promise<boolean> => {
    if (!providers || problems.length > 0) return false;
    setSaving(true);
    setModelsError(null);
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
      return true;
    } catch (e) {
      setModelsError(String(e));
      return false;
    } finally {
      setSaving(false);
    }
  };

  /**
   * Closes, unless that would throw away providers that aren't saved.
   *
   * Every way out comes through here — the ✕, Escape, a click on the scrim —
   * because `Drawer` hands all three the same `onClose`. Asking moves to
   * Models, since that's where the edit is and where the question is shown.
   */
  const close = () => {
    if (dirty) {
      setTab("models");
      setLeaving(true);
    } else {
      onClose();
    }
  };

  return (
    <Drawer title="Settings" onClose={close} panel="settings">
      {/* A column of tabs beside the page rather than a row above it. A row
          of six pills already wrapped at this width, and the tabs still to
          come would have made it three lines of navigation. Under 720px the
          column folds back into a row that scrolls rather than wraps. */}
      <div className="settings-body flex min-h-0 flex-1 gap-4 max-[720px]:flex-col">
        <div
          className="settings-nav flex w-[132px] flex-none flex-col gap-0.5 max-[720px]:w-auto max-[720px]:flex-row max-[720px]:overflow-x-auto"
          role="tablist"
          aria-label="Settings"
          aria-orientation="vertical"
          onKeyDown={onTabKeys}
        >
          {TABS.map(([value, label]) => (
            <button
              key={value}
              id={`settings-tab-${value}`}
              role="tab"
              aria-selected={tab === value}
              aria-controls="settings-pane"
              tabIndex={tab === value ? 0 : -1}
              className={`settings-tab rounded-md border-0 px-2.5 py-1.5 text-left text-13 whitespace-nowrap ${
                tab === value
                  ? "bg-active text-ink"
                  : "bg-transparent text-dim hover:bg-hover hover:text-ink"
              }`}
              onClick={() => setTab(value)}
            >
              {label}
              {/* Edits that wait for Save are visible from every other tab,
                  so they aren't forgotten behind one. */}
              {value === "models" && dirty && (
                <span className="text-accent" aria-label="unsaved changes">
                  {" "}•
                </span>
              )}
            </button>
          ))}
        </div>

        <div
          id="settings-pane"
          role="tabpanel"
          aria-labelledby={`settings-tab-${tab}`}
          className="settings-pane flex min-w-0 flex-1 flex-col gap-4 overflow-y-auto pr-1 [&>.section:first-child]:border-t-0 [&>.section:first-child]:pt-0"
        >
          {tab === "models" && (
            <>
              {leaving && dirty && (
                <div
                  className="settings-leaving flex flex-col gap-2 rounded-md border border-warn p-3 text-13"
                  role="alertdialog"
                  aria-label="Unsaved providers"
                >
                  <span>Your provider changes aren't saved yet.</span>
                  <div className="settings-actions">
                    <button
                      className="primary"
                      disabled={saving || problems.length > 0}
                      data-tip={problems.length > 0 ? "Fix what's listed below first" : undefined}
                      onClick={() => void save().then((ok) => ok && onClose())}
                    >
                      Save and close
                    </button>
                    <button className="danger" onClick={onClose}>
                      Discard
                    </button>
                    <button onClick={() => setLeaving(false)}>Keep editing</button>
                  </div>
                </div>
              )}

              {/* The one tab that waits for Save, said where it applies. A
                  provider is checked as a whole, and saving on every keystroke
                  would hand a running turn a half-typed address. */}
              <p className="drawer-intro">
                Changes here wait for <b>Save</b>, unlike the other tabs: a
                provider is checked as a whole, so a half-typed address never
                reaches a running conversation. Saved to{" "}
                <code>~/.taurus/providers.json</code>, shared with the CLI. Keys
                never go in that file: store one below and it goes to your OS
                keychain, or name an environment variable that holds it.
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

              {/* Pinned to the bottom of the pane: with a few providers open the
                  list is longer than the drawer, and Save was below all of it. */}
              <div className="settings-actions settings-save sticky bottom-0 -mb-px border-t border-rule bg-raised py-2.5">
                {saved && !saving && <span className="text-dim text-12">Saved</span>}
                {dirty && !saving && <span className="text-dim text-12">Unsaved changes</span>}
                <div className="spacer" />
                <button
                  className="primary"
                  onClick={() => void save()}
                  disabled={saving || !dirty || problems.length > 0 || !draft}
                >
                  {saving ? "Saving…" : "Save"}
                </button>
              </div>

              {problems.map((problem) => (
                <Problem key={problem}>{problem}</Problem>
              ))}
              {modelsError && <Problem>{modelsError}</Problem>}
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
                failure={index.failure}
                onBuild={index.onBuild}
              />
            </>
          )}

          {tab === "profile" && <ProfileTab />}

          {tab === "tools" && <ToolsTab />}

          {tab === "routing" && <RoutingTab />}
          {tab === "tracing" && <TracingTab />}

          {tab === "hooks" && <HooksTab />}

          {tab === "terminal" && <TerminalTab />}

          {tab === "permissions" && (
            <PermissionsTab
              rules={rules}
              error={revokeError ?? rulesError}
              onRevoke={(allowed) =>
                run(setRevokeError, async () => {
                  await api.revokePermissionRule(allowed.rule, allowed.scope);
                  setRules(await api.listPermissionRules());
                })
              }
            />
          )}

          {tab === "behavior" && (
            <>
              {settingsProblems.length > 0 && (
                <Problems problems={settingsProblems.map((p) => p.message)} />
              )}

              <label className="settings-check">
                <input
                  type="checkbox"
                  checked={status?.settings.skill_synthesis_enabled ?? true}
                  onChange={(e) => {
                    const on = e.target.checked;
                    run(setBehaviorError, async () => {
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
              <FromProject field="skill_synthesis_enabled" />

              <label className="settings-check">
                <input
                  type="checkbox"
                  checked={status?.settings.agent_synthesis_enabled ?? true}
                  onChange={(e) => {
                    const on = e.target.checked;
                    run(setBehaviorError, async () => {
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
              <FromProject field="agent_synthesis_enabled" />

              <label className="settings-check">
                <input
                  type="checkbox"
                  checked={status?.settings.notify_when_done ?? true}
                  onChange={(e) => {
                    const on = e.target.checked;
                    run(setBehaviorError, async () => {
                      await api.setNotifyWhenDone(on);
                      await refresh();
                    });
                  }}
                />
                <span>
                  Tell me when a turn finishes while I'm elsewhere
                  <span className="hint">
                    A system notification, when Taurus isn't the app in front
                    or you've moved to another conversation. Not for a turn you
                    stopped.
                  </span>
                </span>
              </label>
              <FromProject field="notify_when_done" />

              <label className="settings-check">
                <input
                  type="checkbox"
                  checked={status?.settings.context_strategy === "relay"}
                  onChange={(e) => {
                    const strategy = e.target.checked ? "relay" : "compact";
                    run(setBehaviorError, async () => {
                      await api.setContextStrategy(strategy);
                      await refresh();
                    });
                  }}
                />
                <span>
                  Hand long turns to a fresh context
                  <span className="hint">
                    When the window fills, Taurus starts over from a short brief
                    instead of summarizing. Meant for small local models.
                  </span>
                </span>
              </label>
              <FromProject field="context_strategy" />

              <IterationLimit
                limit={status?.settings.max_iterations ?? DEFAULT_MAX_ITERATIONS}
                onError={setBehaviorError}
              />
              <FromProject field="max_iterations" />
              <DelegateLimit
                count={
                  status?.settings.max_concurrent_agents ?? DEFAULT_CONCURRENT_AGENTS
                }
                onError={setBehaviorError}
              />
              <FromProject field="max_concurrent_agents" />
              <ContextLimit
                limit={status?.settings.context_limit ?? DEFAULT_CONTEXT_LIMIT}
                onError={setBehaviorError}
              />
              <FromProject field="context_limit" />
              {behaviorError && <Problem>{behaviorError}</Problem>}
            </>
          )}

          {tab === "voice" && <VoiceTab />}

          {tab === "appearance" && (
            <ThemePicker
                theme={status?.settings.theme ?? "system"}
                uiScale={status?.settings.ui_scale ?? 100}
              />
          )}

          {/* Its own tab rather than the foot of every one. Under each tab it
              read as part of that tab — Voice ended with two "Files" sections, its
              own and this one — and it pushed every tab's last control further
              from the top for something looked at once a release. */}
          {tab === "about" && (
            <>
              <UpdatesSection />

              <section className="section">
                <span className="micro">Files</span>
                <dl className="settings-paths m-0 grid grid-cols-[88px_1fr] gap-x-3 gap-y-1.5 text-12-5 [&_dt]:text-faint [&_dd]:m-0 [&_dd]:wrap-anywhere [&_dd]:font-mono [&_dd]:text-12 [&_dd]:text-dim">
                  <dt>Config</dt>
                  <dd>~/.taurus</dd>
                  <dt>This project</dt>
                  <dd>{status ? `${status.workspace}/.taurus` : "—"}</dd>
                </dl>
              </section>
            </>
          )}
        </div>
      </div>
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
export function IterationLimit({
  limit,
  onError,
}: {
  limit: number;
  /**
   * Told why a write didn't take, or `null` as one starts. The field goes
   * back to the number in use, so it never shows a limit that isn't one.
   */
  onError?: (message: string | null) => void;
}) {
  const refresh = useStore((s) => s.refresh);
  const [draft, setDraft] = useState(String(limit));

  // The store is the authority: a refresh from anywhere else — another pane,
  // a hand-edited settings file — has to win over a stale draft.
  useEffect(() => setDraft(String(limit)), [limit]);

  const commit = async () => {
    const next = clampIterations(draft, limit);
    setDraft(String(next));
    if (next === limit) return;
    onError?.(null);
    try {
      await api.setMaxIterations(next);
      await refresh();
    } catch (e) {
      setDraft(String(limit));
      onError?.(`Round trips per message stayed at ${limit}: ${e}`);
    }
  };

  return (
    <Field
      label="Round trips per message"
      hint={`How many times the model may call tools and read the results before Taurus stops the turn. Raise it for long refactors; lower it to catch a model going in circles sooner. 1–${MAX_ITERATIONS_LIMIT}, ${DEFAULT_MAX_ITERATIONS} by default.`}
    >
      <input
        inputMode="numeric"
        aria-label="Round trips per message"
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
 * How many sub-agents one turn runs at once. Committed on blur or Enter, for
 * the reason `IterationLimit` gives.
 */
export function DelegateLimit({
  count,
  onError,
}: {
  count: number;
  /** As `IterationLimit`'s. */
  onError?: (message: string | null) => void;
}) {
  const refresh = useStore((s) => s.refresh);
  const [draft, setDraft] = useState(String(count));

  useEffect(() => setDraft(String(count)), [count]);

  const commit = async () => {
    const next = clampConcurrentAgents(draft, count);
    setDraft(String(next));
    if (next === count) return;
    onError?.(null);
    try {
      await api.setMaxConcurrentAgents(next);
      await refresh();
    } catch (e) {
      setDraft(String(count));
      onError?.(`Agents at once stayed at ${count}: ${e}`);
    }
  };

  return (
    <Field
      label="Agents at once"
      hint={`How many sub-agents one message runs at the same time. More wait their turn. Keep it low on a local model sharing one GPU; raise it on a hosted one. 1–${MAX_CONCURRENT_AGENTS_LIMIT}, ${DEFAULT_CONCURRENT_AGENTS} by default.`}
    >
      <input
        inputMode="numeric"
        aria-label="Agents at once"
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
 * How much of a model's window a turn fills before it compacts.
 *
 * Shown in thousands and read back in whatever form was typed, because the
 * numbers are six digits and the one people know is "200k".
 */
export function ContextLimit({
  limit,
  onError,
}: {
  limit: number;
  /** As `IterationLimit`'s. */
  onError?: (message: string | null) => void;
}) {
  const refresh = useStore((s) => s.refresh);
  const shown = (n: number) => (n === 0 ? "0" : `${Math.round(n / 1000)}k`);
  const [draft, setDraft] = useState(shown(limit));

  useEffect(() => setDraft(shown(limit)), [limit]);

  const commit = async () => {
    const next = clampContextLimit(draft, limit);
    setDraft(shown(next));
    if (next === limit) return;
    onError?.(null);
    try {
      await api.setContextLimit(next);
      await refresh();
    } catch (e) {
      setDraft(shown(limit));
      onError?.(`Working context stayed at ${shown(limit)}: ${e}`);
    }
  };

  return (
    <Field
      label="Working context"
      hint={`Tokens of the model's window a conversation may fill before Taurus compacts it. Every step re-sends the conversation, so on a hosted model a larger number costs more per step. 0 uses the model's whole window; ${shown(DEFAULT_CONTEXT_LIMIT)} by default.`}
    >
      <input
        inputMode="numeric"
        aria-label="Working context"
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
export function ThemePicker({
  theme,
  uiScale = 100,
}: {
  theme: Theme;
  /** The interface size in force, in percent. */
  uiScale?: number;
}) {
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
      .catch((e) => {
        // The Brand row still offers the built-in theme, but says why the
        // rest are missing rather than looking like there are none.
        setThemes([]);
        setError(`Couldn't read the themes folder: ${e}`);
      });
  };
  useEffect(load, []);

  // Painted back by hand when the write fails: the settings didn't change, so
  // the refresh that follows has nothing new to repaint from.
  const chooseMode = async (next: Theme) => {
    setError(null);
    applyTheme(next, custom);
    try {
      await api.setTheme(next);
    } catch (e) {
      applyTheme(theme, custom);
      setError(`The theme stayed as it was: ${e}`);
    }
    await refresh();
  };

  const chooseBrand = async (id: string) => {
    setArming(false);
    setError(null);
    const picked = themes?.find((t) => t.id === id) ?? null;
    applyTheme(theme, picked);
    try {
      await api.setThemeId(id);
    } catch (e) {
      applyTheme(theme, custom);
      setError(`The theme stayed as it was: ${e}`);
    }
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
        <div className="pill-row" role="radiogroup" aria-label="Theme" onKeyDown={onTabKeys}>
          {THEMES.map(([value, label]) => (
            <button
              key={value}
              role="radio"
              aria-checked={theme === value}
              tabIndex={theme === value ? 0 : -1}
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
        <span className="micro">Interface size</span>
        <div className="pill-row" role="radiogroup" aria-label="Interface size" onKeyDown={onTabKeys}>
          {UI_SCALES.map((percent) => {
            const on = uiScale === percent;
            return (
              <button
                key={percent}
                role="radio"
                aria-checked={on}
                tabIndex={on ? 0 : -1}
                className={`pill${on ? " on" : ""}`}
                onClick={() => {
                  setError(null);
                  api
                    .setUiScale(percent)
                    .then(() => refresh())
                    .catch((e) => setError(`The size stayed as it was: ${e}`));
                }}
              >
                {percent}%
              </button>
            );
          })}
        </div>
        <p className="hint">
          Text, spacing and the terminal together, in every window. Applied as
          soon as you pick one.
        </p>
      </section>

      <section className="section">
        <span className="micro">Brand</span>
        <div className="pill-row" role="radiogroup" aria-label="Brand" onKeyDown={onTabKeys}>
          <button
            role="radio"
            aria-checked={custom === null}
            tabIndex={custom === null ? 0 : -1}
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
              tabIndex={custom?.id === t.id ? 0 : -1}
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
          A theme is a file in <code>~/.taurus/themes</code> — fourteen colors,
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
          existing={themes ?? []}
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

/**
 * The sizes offered. Steps rather than a slider, because each one is a
 * redraw of the whole window and a slider dragged across six of them would be
 * six redraws for the one somebody let go on.
 */
const UI_SCALES = [75, 90, 100, 110, 125, 150];

const THEMES: [Theme, string][] = [
  ["system", "System"],
  ["light", "Light"],
  ["dark", "Dark"],
];

const TABS: [Tab, string][] = [
  // First, as the one tab that is about you rather than the software.
  ["profile", "Profile"],
  ["models", "Models"],
  // Beside Models, since a route is a provider and a model from there put to
  // one kind of job.
  ["routing", "Routing"],
  ["search", "Search"],
  ["tools", "Tools"],
  ["permissions", "Permissions"],
  ["hooks", "Hooks"],
  ["behavior", "Behavior"],
  ["terminal", "Terminal"],
  ["voice", "Voice"],
  // Its own tab rather than the last section of Behavior, which is where it
  // started. Behavior is what the agent is allowed to do on its own — propose
  // skills, propose sub-agents, take this many steps — and how the window is
  // painted is not one of those. It was three pills when it was filed there;
  // it is now a mode, a brand, an editor and whatever a theme file got wrong,
  // and a tab whose name does not predict its contents is the same problem the
  // rail had with "Tools".
  ["appearance", "Appearance"],
  ["tracing", "Tracing"],
  ["about", "About"],
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

