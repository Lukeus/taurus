import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import { ChangesDrawer } from "./components/ChangesDrawer";
import { PermissionDialog } from "./components/PermissionDialog";
import { Rail, type ProviderHealth } from "./components/Rail";
import {
  RAIL_WIDTH,
  ResizeHandle,
  useResizableWidth,
} from "./components/ResizeHandle";
import { Settings } from "./components/Settings";
import { AgentsDrawer } from "./components/AgentsDrawer";
import { SkillsDrawer } from "./components/SkillsDrawer";
import { AgentProposalCard } from "./components/AgentProposalCard";
import { SkillProposalCard } from "./components/SkillProposalCard";
import { Transcript } from "./components/Transcript";
import * as api from "./lib/api";
import type { ModelInfo, ProviderConfig, Theme } from "./lib/api";
import { basename, plural } from "./lib/format";
import { applyTheme, watchSystemTheme } from "./lib/theme";
import { useStore } from "./state/store";

export default function App() {
  const store = useStore();
  const rail = useResizableWidth({ storageKey: "taurus.railWidth", ...RAIL_WIDTH });
  const [models, setModels] = useState<ModelInfo[] | "failed" | null>(null);
  const [skillsOpen, setSkillsOpen] = useState(false);
  const [agentsOpen, setAgentsOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [changesOpen, setChangesOpen] = useState(false);

  useEffect(() => {
    store.init();
    // Intentionally once: init wires the event listeners.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Settings are the authority; main.tsx only guessed from the last run. Also
  // where following the OS is honoured — while the preference is `system`, a
  // machine that switches at dusk switches the app with it, and the listener is
  // torn down the moment someone picks a side.
  const theme = store.status?.settings.theme;
  useEffect(() => {
    if (!theme) return;
    applyTheme(theme);
    return watchSystemTheme(theme);
  }, [theme]);

  const providers = store.status?.providers ?? [];
  const providerId = currentProvider(
    providers,
    store.session?.provider_id,
    store.status?.settings.last_provider,
  );

  // Which provider the visible model list belongs to. Without it, switching
  // providers lists twice — once here, and again when starting the session
  // moves `providerId` on to the new one.
  const listedFor = useRef<string | null>(null);

  useEffect(() => {
    if (!providerId || listedFor.current === providerId) return;
    listedFor.current = providerId;
    setModels(null);
    api
      .listModels(providerId)
      .then(setModels)
      .catch(() => setModels("failed"));
  }, [providerId]);

  /**
   * Switches provider, which means starting a conversation on it.
   *
   * The same thing choosing a model does: a session is bound to one provider
   * and model, so changing either is a new conversation rather than a setting
   * applied to this one.
   */
  const chooseProvider = async (id: string) => {
    if (id === providerId) return;
    listedFor.current = id;
    setModels(null);
    const config = providers.find((p) => p.id === id);
    try {
      const list = await api.listModels(id);
      setModels(list);
      // The named default ahead of whatever the backend happened to list
      // first, which is the order `resolve_model` uses for the CLI.
      const first = config?.default_model ?? list[0]?.id;
      if (first) await store.startSession(id, first);
    } catch {
      setModels("failed");
      // A backend with no model listing is still usable when the config names
      // what to talk to — an Azure APIM route often exposes the chat endpoint
      // and nothing else.
      const named = config?.default_model ?? offered("failed", config)[0]?.id;
      if (named) await store.startSession(id, named);
    }
  };

  const provider = providers.find((p) => p.id === providerId);
  const available = offered(models, provider, store.session?.model);
  const workspace = store.status?.workspace ?? null;

  const pickWorkspace = async () => {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") await store.setWorkspace(chosen);
  };

  const newConversation = () => {
    const model =
      store.session?.model ?? provider?.default_model ?? available[0]?.id;
    if (providerId && model) return store.startSession(providerId, model);
    // Nothing to start a conversation with yet; the place to fix that is here.
    setSettingsOpen(true);
  };

  const title =
    store.sessions.find((s) => s.id === store.session?.id)?.title ||
    "New conversation";

  /**
   * The same two steps `ThemePicker` takes, because the rail row and the
   * Settings pills set one preference between them: paint immediately so the
   * click is answered by the screen it changed, then write, then re-read — the
   * settings file stays the authority, and the effect above repaints from it.
   */
  const chooseTheme = async (next: Theme) => {
    applyTheme(next);
    await api.setTheme(next);
    await store.refresh();
  };

  return (
    <div className="app">
      <Rail
        width={rail.width}
        workspace={workspace}
        sessions={store.sessions}
        currentId={store.session?.id}
        changedCount={store.changed.length}
        busy={store.busy}
        skillCount={store.status?.skill_count ?? null}
        agentCount={store.status?.agent_count ?? null}
        health={health(store.status?.providers.length, providerId, models)}
        theme={theme ?? "system"}
        onPickWorkspace={pickWorkspace}
        onNew={newConversation}
        onOpen={store.resume}
        onDelete={store.remove}
        onTheme={chooseTheme}
        onSkills={() => setSkillsOpen(true)}
        onAgents={() => setAgentsOpen(true)}
        onSettings={() => setSettingsOpen(true)}
      />

      <ResizeHandle pane={rail} label="Rail width" />

      <div className="pane">
        <header className="topbar">
          <span className="topbar-title">{title}</span>

          {store.session && !store.session.native_tools && (
            <span
              className="tag warn"
              title="This model has no built-in tool calling; Taurus prompts for it instead."
            >
              prompted tools
            </span>
          )}

          <div className="spacer" />

          {store.session && (
            <button
              className="chip"
              title="Files this conversation changed, and the way back"
              onClick={() => setChangesOpen(true)}
            >
              <span className={`dot${store.changed.length > 0 ? " accent" : ""}`} />
              {store.changed.length > 0
                ? `${plural(store.changed.length, "file")} changed`
                : "No file changes"}
            </button>
          )}

          {/* Only worth a control when there is a choice to make. One provider
              is the common case and the picker would be a dropdown that can
              only ever say what the model list already implies. */}
          {providers.length > 1 && (
            <select
              className="provider-select"
              aria-label="Provider"
              value={providerId ?? ""}
              disabled={store.busy}
              onChange={(e) => chooseProvider(e.target.value)}
            >
              {providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.id}
                </option>
              ))}
            </select>
          )}

          <select
            className="model-select"
            aria-label="Model"
            value={store.session?.model ?? ""}
            disabled={store.busy || !providerId}
            onChange={(e) => providerId && store.startSession(providerId, e.target.value)}
          >
            {available.length === 0 && <option value="">no models</option>}
            {/* `available` already carries the running session's model even
                when nothing listed it, so the select can always show what the
                conversation is actually on. */}
            {available.map((m) => (
              <option key={m.id} value={m.id}>
                {m.display_name}
              </option>
            ))}
          </select>
        </header>

        <main>
          <Transcript
            entries={store.entries}
            busy={store.busy}
            empty={
              <FirstRun
                workspace={workspace}
                ready={!!store.session}
                health={health(store.status?.providers.length, providerId, models)}
                onPickWorkspace={pickWorkspace}
                onSettings={() => setSettingsOpen(true)}
              />
            }
          />

          {(store.proposals.length > 0 ||
            store.agentProposals.length > 0) && (
            <div className="proposals">
              {store.proposals.map((p) => (
                <SkillProposalCard
                  key={p.id}
                  proposal={p}
                  onResolve={(approve, target) =>
                    store.resolveProposal(p.id, approve, target)
                  }
                />
              ))}
              {store.agentProposals.map((p) => (
                <AgentProposalCard
                  key={p.id}
                  proposal={p}
                  onResolve={(approve, target) =>
                    store.resolveAgentProposal(p.id, approve, target)
                  }
                />
              ))}
            </div>
          )}
        </main>

        {store.error && (
          <div className="banner error">
            {store.error}
            <div className="spacer" />
            <button onClick={store.dismissError}>Dismiss</button>
          </div>
        )}

        <Composer
          busy={store.busy}
          ready={!!store.session}
          workspace={workspace}
          onPickWorkspace={pickWorkspace}
          onSend={store.send}
          onStop={store.stop}
        />
      </div>

      {store.permission && (
        <PermissionDialog
          request={store.permission}
          onDecide={store.answerPermission}
        />
      )}

      {changesOpen && store.session && (
        <ChangesDrawer
          sessionId={store.session.id}
          busy={store.busy}
          onClose={() => setChangesOpen(false)}
        />
      )}
      {skillsOpen && <SkillsDrawer onClose={() => setSkillsOpen(false)} />}
      {agentsOpen && <AgentsDrawer onClose={() => setAgentsOpen(false)} />}
      {settingsOpen && <Settings onClose={() => setSettingsOpen(false)} />}
    </div>
  );
}

/**
 * Which provider the header is showing.
 *
 * The open conversation decides it, because a session is bound to the provider
 * it was started on. With no session, the one this workspace was last worked in
 * — falling back to the first configured only when that provider is gone, which
 * is what happens after it is removed in Settings.
 *
 * `last_provider` matters more than it looks: the store restores it on launch,
 * so ignoring it here meant the header disagreed with the session actually
 * running whenever the restore failed.
 */
export function currentProvider(
  providers: ProviderConfig[],
  sessionProvider: string | undefined,
  lastProvider: string | null | undefined,
): string | undefined {
  if (sessionProvider) return sessionProvider;
  const remembered = providers.find((p) => p.id === lastProvider);
  return (remembered ?? providers[0])?.id;
}

/**
 * What the model picker can offer.
 *
 * A listing is the answer when there is one. When there is not, the config
 * still knows: a gateway with no `/v1/models` is exactly the case `models` and
 * `default_model` exist for, and the picker used to say "no models" while a
 * conversation ran happily on one of them — the app disagreeing with itself
 * about something the user can see in two places at once.
 *
 * `current` is folded in for the same reason. A session already running on a
 * model nothing listed still has to be selectable, or the `<select>` falls
 * back to displaying its first option and names the wrong model as chosen.
 */
export function offered(
  models: ModelInfo[] | "failed" | null,
  provider: ProviderConfig | undefined,
  current?: string,
): ModelInfo[] {
  const listed = Array.isArray(models) ? models : [];
  const from = listed.length > 0 ? listed : declared(provider);
  if (current && !from.some((m) => m.id === current)) {
    return [...from, named(current)];
  }
  return from;
}

/** The models a provider's own config names, in the order it names them. */
function declared(provider: ProviderConfig | undefined): ModelInfo[] {
  if (!provider) return [];
  if (provider.models.length > 0) {
    return provider.models.map((m) => ({
      id: m.id,
      display_name: m.display_name ?? m.id,
      context_length: m.context_length ?? null,
    }));
  }
  // Predates `models`, and still the whole config for a provider that serves
  // one thing.
  return provider.default_model ? [named(provider.default_model)] : [];
}

function named(id: string): ModelInfo {
  return { id, display_name: id, context_length: null };
}

/**
 * Whether the provider behind this session is answering.
 *
 * There is no health endpoint to ask, so the model listing stands in for one:
 * it is the first thing the app does with a provider and the first thing that
 * fails when the provider is not there.
 */
export function health(
  providerCount: number | undefined,
  providerId: string | undefined,
  models: ModelInfo[] | "failed" | null,
): ProviderHealth {
  if (providerCount === 0) return { state: "none" };
  if (!providerId || models === null) return { state: "unknown" };
  if (models === "failed") return { state: "unreachable", id: providerId };
  return { state: "connected", id: providerId, models: models.length };
}

/**
 * What fills the transcript before there is one.
 *
 * Says the same thing in both of the states it covers — this is a folder
 * Taurus works in and every change is undoable — but only offers setup when
 * setup is what is missing.
 */
function FirstRun({
  workspace,
  ready,
  health,
  onPickWorkspace,
  onSettings,
}: {
  workspace: string | null;
  ready: boolean;
  health: ProviderHealth;
  onPickWorkspace: () => void;
  onSettings: () => void;
}) {
  return (
    <div className="hero">
      <div className="hero-mark">t</div>
      <div className="hero-copy">
        <h1>
          {ready && workspace
            ? `Ready in ${basename(workspace)}`
            : "Point Taurus at a folder"}
        </h1>
        <p>
          It reads and edits files there, runs commands with your approval, and
          remembers every change so any turn can be undone.
        </p>
      </div>
      <div className="hero-actions">
        <button className="primary" onClick={onPickWorkspace}>
          {ready ? "Change workspace" : "Choose a workspace"}
        </button>
        <button onClick={onSettings}>
          {ready ? "Providers" : "Connect a model"}
        </button>
      </div>
      <div className="hero-status">
        <span className={`dot ${health.state === "connected" ? "ok" : health.state === "unreachable" ? "error" : ""}`} />
        {health.state === "connected"
          ? `${health.id} · ${plural(health.models, "model")} available`
          : health.state === "unreachable"
            ? `${health.id} is not answering`
            : health.state === "none"
              ? "no provider configured yet"
              : "looking for a provider…"}
      </div>
    </div>
  );
}

function Composer({
  busy,
  ready,
  workspace,
  onPickWorkspace,
  onSend,
  onStop,
}: {
  busy: boolean;
  ready: boolean;
  workspace: string | null;
  onPickWorkspace: () => void;
  onSend: (text: string) => void;
  onStop: () => void;
}) {
  const [text, setText] = useState("");

  const submit = () => {
    if (!text.trim() || busy) return;
    onSend(text);
    setText("");
  };

  return (
    <footer className="composer">
      <div className="composer-box">
        <textarea
          value={text}
          placeholder={ready ? "Ask Taurus to do something…" : "Connect a model to begin"}
          disabled={!ready}
          rows={1}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Enter sends; Shift+Enter is a newline, matching every chat UI.
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
        />
        <div className="composer-foot">
          <button
            className="pill"
            onClick={onPickWorkspace}
            title={workspace ?? "Choose a workspace"}
          >
            ▤ {workspace ? basename(workspace) : "no workspace"}
          </button>
          <div className="spacer" />
          <span className="composer-hint">↵ send · ⇧↵ newline</span>
          {busy ? (
            <button className="danger composer-send" onClick={onStop}>
              Stop
            </button>
          ) : (
            <button
              className="primary composer-send"
              onClick={submit}
              disabled={!ready || !text.trim()}
            >
              Send
            </button>
          )}
        </div>
      </div>
    </footer>
  );
}
