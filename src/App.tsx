import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import { ChangesDrawer } from "./components/ChangesDrawer";
import { PermissionDialog } from "./components/PermissionDialog";
import { Rail, type ProviderHealth } from "./components/Rail";
import { Settings } from "./components/Settings";
import { SkillsDrawer } from "./components/SkillsDrawer";
import { SkillProposalCard } from "./components/SkillProposalCard";
import { Transcript } from "./components/Transcript";
import * as api from "./lib/api";
import type { ModelInfo } from "./lib/api";
import { basename, plural } from "./lib/format";
import { useStore } from "./state/store";

export default function App() {
  const store = useStore();
  const [models, setModels] = useState<ModelInfo[] | "failed" | null>(null);
  const [skillsOpen, setSkillsOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [changesOpen, setChangesOpen] = useState(false);

  useEffect(() => {
    store.init();
    // Intentionally once: init wires the event listeners.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const providerId = store.session?.provider_id ?? store.status?.providers[0]?.id;

  useEffect(() => {
    if (!providerId) return;
    setModels(null);
    api
      .listModels(providerId)
      .then(setModels)
      .catch(() => setModels("failed"));
  }, [providerId]);

  const available = Array.isArray(models) ? models : [];
  const workspace = store.status?.workspace ?? null;

  const pickWorkspace = async () => {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") await store.setWorkspace(chosen);
  };

  const newConversation = () => {
    const model = store.session?.model ?? available[0]?.id;
    if (providerId && model) return store.startSession(providerId, model);
    // Nothing to start a conversation with yet; the place to fix that is here.
    setSettingsOpen(true);
  };

  const title =
    store.sessions.find((s) => s.id === store.session?.id)?.title ||
    "New conversation";

  return (
    <div className="app">
      <Rail
        workspace={workspace}
        sessions={store.sessions}
        currentId={store.session?.id}
        changedCount={store.changed.length}
        busy={store.busy}
        skillCount={store.status?.skill_count ?? null}
        health={health(store.status?.providers.length, providerId, models)}
        onPickWorkspace={pickWorkspace}
        onNew={newConversation}
        onOpen={store.resume}
        onSkills={() => setSkillsOpen(true)}
        onSettings={() => setSettingsOpen(true)}
      />

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

          <select
            className="model-select"
            aria-label="Model"
            value={store.session?.model ?? ""}
            disabled={store.busy || !providerId}
            onChange={(e) => providerId && store.startSession(providerId, e.target.value)}
          >
            {available.length === 0 && <option value="">no models</option>}
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

          {store.proposals.length > 0 && (
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
      {settingsOpen && <Settings onClose={() => setSettingsOpen(false)} />}
    </div>
  );
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
