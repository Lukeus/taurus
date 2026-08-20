import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useShallow } from "zustand/react/shallow";

import { Attachments } from "./components/Attachments";
import { ChangesDrawer } from "./components/ChangesDrawer";
import { DelegateTranscript } from "./components/DelegateTranscript";
import { CommandMenu, commandQuery, matches } from "./components/CommandMenu";
import { ConversationTitle } from "./components/ConversationTitle";
import { PermissionDialog } from "./components/PermissionDialog";
import { TrustBanner } from "./components/TrustBanner";
import { PlanPanel } from "./components/PlanPanel";
import { Rail, type ProviderHealth } from "./components/Rail";
import {
  RAIL_WIDTH,
  ResizeHandle,
  useResizableWidth,
} from "./components/ResizeHandle";
import { Settings } from "./components/Settings";
import { AgentsDrawer } from "./components/AgentsDrawer";
import { McpDrawer } from "./components/McpDrawer";
import { MemoryDrawer } from "./components/MemoryDrawer";
import { SkillsDrawer } from "./components/SkillsDrawer";
import { AgentProposalCard } from "./components/AgentProposalCard";
import { SkillProposalCard } from "./components/SkillProposalCard";
import { Transcript, type TranscriptProps } from "./components/Transcript";
import * as api from "./lib/api";
import type {
  Attachment,
  CommandSummary,
  ModelInfo,
  ProviderConfig,
  ServerStatus,
  Theme,
} from "./lib/api";
import { basename, plural } from "./lib/format";
import { isImage, toAttachments } from "./lib/images";
import { applyTheme, watchSystemTheme } from "./lib/theme";
import { pinnedPlan, useStore } from "./state/store";

export default function App() {
  // Everything except the transcript, compared field by field.
  //
  // A bare `useStore()` subscribes to the whole store, and the store changes
  // every time a frame of a turn arrives — so the rail, the topbar and the
  // model picker were all redrawn a few dozen times a second to say the same
  // thing. The entries are read where they are drawn instead (`TranscriptPane`,
  // `PinnedPlan`), which leaves this re-rendering when something it actually
  // shows has moved: a turn starting or ending, a session opening, a setting
  // landing. `App.bench.tsx` measures the difference.
  //
  // Listing the fields is safe rather than merely tidy: `store` is typed as
  // what this returns, so reading a field that is not named here is a compile
  // error rather than a value that silently stops updating.
  const store = useStore(
    useShallow((s) => ({
      status: s.status,
      trust: s.trust,
      session: s.session,
      sessions: s.sessions,
      changed: s.changed,
      busy: s.busy,
      error: s.error,
      permission: s.permission,
      proposals: s.proposals,
      agentProposals: s.agentProposals,
      init: s.init,
      send: s.send,
      stop: s.stop,
      resume: s.resume,
      remove: s.remove,
      rename: s.rename,
      refresh: s.refresh,
      startSession: s.startSession,
      setWorkspace: s.setWorkspace,
      dismissError: s.dismissError,
      answerPermission: s.answerPermission,
      answerQuestions: s.answerQuestions,
      resolveProposal: s.resolveProposal,
      resolveAgentProposal: s.resolveAgentProposal,
      decideTrust: s.decideTrust,
    })),
  );
  const rail = useResizableWidth({ storageKey: "taurus.railWidth", ...RAIL_WIDTH });
  const [models, setModels] = useState<ModelInfo[] | "failed" | null>(null);
  const [skillsOpen, setSkillsOpen] = useState(false);
  const [agentsOpen, setAgentsOpen] = useState(false);
  const [mcpOpen, setMcpOpen] = useState(false);
  const [memoryOpen, setMemoryOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [changesOpen, setChangesOpen] = useState(false);
  /**
   * Workspaces whose trust banner has been waved off in this window.
   *
   * Keyed by path rather than a single boolean so switching to another folder
   * asks about that folder. Nothing is written to disk — see `TrustBanner`.
   */
  const [trustDismissed, setTrustDismissed] = useState<string[]>([]);
  // Which delegation's own conversation is open, if any. Held here rather than
  // in the row that offers it: it is a drawer over the whole app, like the
  // others, and a row that unmounted mid-read would take it down with it.
  const [delegate, setDelegate] = useState<{
    session: string;
    agent: string;
  } | null>(null);

  useEffect(() => {
    store.init();
    // Intentionally once: init wires the event listeners.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /*
   * Re-asks the questions nothing can push, when the user comes back.
   *
   * Trust is the one that matters: the answer changes when a file appears in a
   * directory nothing is watching — a `git pull` that adds `.taurus/mcp.json`
   * is the case the gate exists for, and it arrives with no event attached to
   * it. Returning to the window is the closest thing to an event there is, and
   * it is precisely when somebody has most likely just been in a terminal.
   *
   * Everything else on screen is pushed, so this is a narrow catch-up rather
   * than a poll: no timer, and nothing happens while the window is not looked
   * at.
   */
  const refresh = store.refresh;
  useEffect(() => {
    const ask = () => void refresh();
    window.addEventListener("focus", ask);
    return () => window.removeEventListener("focus", ask);
  }, [refresh]);

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

  // The listing entry rather than the session: a title is a fact about the
  // transcript, and a conversation with no entry has none of either yet.
  const listed = store.sessions.find((s) => s.id === store.session?.id);
  const title = listed?.title || "New conversation";

  /**
   * The same two steps `ThemePicker` takes, because the rail row and the
   * Settings pills set one preference between them: paint immediately, so the
   * click is answered by the screen it changed, then write.
   *
   * There is no third step any more. The settings file is still the authority
   * on which theme is in force, and what it now says arrives on `EVENT_STATUS`
   * — which is what the effect above repaints from.
   */
  const chooseTheme = async (next: Theme) => {
    applyTheme(next);
    await api.setTheme(next);
  };

  return (
    <div className="app">
      <Rail
        width={rail.width}
        workspace={workspace}
        sessions={store.sessions}
        currentId={store.session?.id}
        changedCount={store.changed.length}
        branch={store.status?.branch ?? null}
        busy={store.busy}
        skillCount={store.status?.skill_count ?? null}
        agentCount={store.status?.agent_count ?? null}
        noteCount={store.status?.note_count ?? null}
        mcp={mcpCounts(store.status?.mcp_servers)}
        health={health(store.status?.providers.length, providerId, models)}
        theme={theme ?? "system"}
        onPickWorkspace={pickWorkspace}
        onNew={newConversation}
        onOpen={store.resume}
        onDelete={store.remove}
        onTheme={chooseTheme}
        onSkills={() => setSkillsOpen(true)}
        onAgents={() => setAgentsOpen(true)}
        onMemory={() => setMemoryOpen(true)}
        onMcp={() => setMcpOpen(true)}
        onSettings={() => setSettingsOpen(true)}
      />

      <ResizeHandle pane={rail} label="Rail width" />

      <div className="pane">
        <header className="topbar">
          <ConversationTitle
            title={title}
            // Only once there is a transcript to write a name into, which there
            // is from the moment the first question is asked.
            renamable={listed !== undefined}
            onRename={(next) => listed && store.rename(listed.id, next)}
          />

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
          <TranscriptPane
            busy={store.busy}
            onAnswer={store.answerQuestions}
            onOpenDelegate={setDelegate}
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

        {/* Above the error banner, because it is about the state the whole
            session is running in rather than about something that just went
            wrong. Dismissed per workspace, so switching folders asks again
            about the new one rather than inheriting an answer about the old. */}
        {store.trust && !trustDismissed.includes(store.trust.workspace) && (
          <TrustBanner
            trust={store.trust}
            onTrust={() => void store.decideTrust(true)}
            onDismiss={() =>
              setTrustDismissed((seen) => [...seen, store.trust!.workspace])
            }
          />
        )}

        {store.error && (
          <div className="banner error">
            {store.error}
            <div className="spacer" />
            <button onClick={store.dismissError}>Dismiss</button>
          </div>
        )}

        {/* Between the error banner and the composer, so its position does not
            move when an error arrives and clears. The plan is the thing you
            look at while typing the next message; it belongs against the box
            you type in. */}
        <PinnedPlan />

        <Composer
          busy={store.busy}
          ready={!!store.session}
          vision={store.session?.vision ?? false}
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

      {memoryOpen && (
        <MemoryDrawer
          onClose={() => setMemoryOpen(false)}
          // Not offered mid-turn: switching conversations under a running one
          // is not something the rail offers either.
          onOpenSession={
            store.busy
              ? undefined
              : (id) => {
                  setMemoryOpen(false);
                  void store.resume(id);
                }
          }
        />
      )}

      {changesOpen && store.session && (
        <ChangesDrawer
          sessionId={store.session.id}
          busy={store.busy}
          onClose={() => setChangesOpen(false)}
        />
      )}
      {delegate && store.session && (
        <DelegateTranscript
          sessionId={store.session.id}
          subagentId={delegate.session}
          agent={delegate.agent}
          onClose={() => setDelegate(null)}
        />
      )}
      {skillsOpen && <SkillsDrawer onClose={() => setSkillsOpen(false)} />}
      {agentsOpen && <AgentsDrawer onClose={() => setAgentsOpen(false)} />}
      {mcpOpen && <McpDrawer onClose={() => setMcpOpen(false)} />}
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
 * The two numbers the rail shows for MCP: how many servers, and how many of
 * them are answering.
 *
 * Both, rather than a total, because they say different things. A count alone
 * cannot distinguish four working servers from four broken ones, and the whole
 * complaint this feature answers is a server that is configured and not there —
 * which is exactly the case where those two numbers differ.
 *
 * `null` before the first status has landed, so the rail shows no badge rather
 * than a zero it would have to take back.
 */
export function mcpCounts(
  servers: ServerStatus[] | undefined,
): { total: number; connected: number } | null {
  if (!servers) return null;
  return {
    total: servers.length,
    connected: servers.filter((s) => s.connected).length,
  };
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

/**
 * The transcript, reading the entries itself.
 *
 * This is the one thing on screen that changes as a turn streams, and it
 * subscribes on its own behalf so that being redrawn thirty times a second does
 * not mean redrawing the rail thirty times a second alongside it. Everything
 * else it needs comes from `App`, which by then is re-rendering only when
 * something it shows has moved.
 */
function TranscriptPane(props: Omit<TranscriptProps, "entries">) {
  const entries = useStore((s) => s.entries);
  return <Transcript entries={entries} {...props} />;
}

/**
 * The pinned plan, for the same reason.
 *
 * `pinnedPlan` reads the transcript, so leaving it in `App` would have put the
 * entries back into `App`'s subscription and undone the whole arrangement. It
 * returns the view off the entry that carries it, so an unchanged plan is the
 * same object twice and this does not re-render on every frame either.
 */
function PinnedPlan() {
  const plan = useStore((s) => pinnedPlan(s.entries));
  return plan ? <PlanPanel view={plan} /> : null;
}

function Composer({
  busy,
  ready,
  vision,
  workspace,
  onPickWorkspace,
  onSend,
  onStop,
}: {
  busy: boolean;
  ready: boolean;
  /**
   * Whether this session's model reads images.
   *
   * Decides whether a paste or a drop is taken at all. Accepting one on a model
   * that cannot see would be an invitation to a refusal — the backend would
   * turn it down, correctly, one round trip later.
   */
  vision: boolean;
  workspace: string | null;
  onPickWorkspace: () => void;
  onSend: (text: string, images: Attachment[]) => void;
  onStop: () => void;
}) {
  const [text, setText] = useState("");
  const [commands, setCommands] = useState<CommandSummary[]>([]);
  const [active, setActive] = useState(0);
  const [images, setImages] = useState<Attachment[]>([]);
  const [attachError, setAttachError] = useState<string | null>(null);
  // Tracked with a counter rather than a boolean: dragging over a child fires
  // `dragleave` on the parent, so a boolean flickers the highlight off while
  // the file is still over the composer.
  const [dragDepth, setDragDepth] = useState(0);

  const attach = async (files: File[]) => {
    const wanted = files.filter(isImage);
    // A drag carrying no image at all is not this component's business — a
    // file dropped on the composer is a mistake worth naming, but text dragged
    // from another window is not.
    if (wanted.length === 0) {
      if (files.length > 0) {
        setAttachError("Only images can be attached. Use PNG, JPEG, WebP, or GIF.");
      }
      return;
    }
    const { attachments, errors } = await toAttachments(wanted, images.length);
    if (attachments.length > 0) setImages((held) => [...held, ...attachments]);
    setAttachError(errors.length > 0 ? errors.join(" ") : null);
  };

  // Fetched once the composer is usable, and again whenever a workspace change
  // could have brought a different library with it.
  useEffect(() => {
    if (!ready) return;
    api.listCommands().then(setCommands).catch(() => setCommands([]));
  }, [ready, workspace]);

  const query = commandQuery(text);
  const shown = query === null ? [] : matches(commands, query);
  // Clamped rather than reset: the list narrows as the user types, and a
  // highlight left pointing past the end would send the wrong command on Enter.
  const index = Math.min(active, Math.max(shown.length - 1, 0));

  const complete = (command: CommandSummary) => {
    // Trailing space, because every one of these takes arguments and the next
    // thing to happen is typing them.
    setText(`/${command.name} `);
    setActive(0);
  };

  const submit = () => {
    if (!text.trim() || busy) return;
    onSend(text, images);
    setText("");
    setImages([]);
    setAttachError(null);
    setActive(0);
  };

  return (
    <footer className="composer">
      <div
        className={`composer-box${dragDepth > 0 ? " dropping" : ""}`}
        // The whole box is the target, not the textarea: someone dragging a
        // screenshot aims at the thing that looks like the message, and the
        // textarea is one row tall until it is typed into.
        onDragEnter={(e) => {
          if (!vision || !ready) return;
          e.preventDefault();
          setDragDepth((d) => d + 1);
        }}
        onDragOver={(e) => {
          if (vision && ready) e.preventDefault();
        }}
        onDragLeave={() => setDragDepth((d) => Math.max(0, d - 1))}
        onDrop={(e) => {
          if (!vision || !ready) return;
          e.preventDefault();
          setDragDepth(0);
          void attach([...e.dataTransfer.files]);
        }}
      >
        {shown.length > 0 && (
          <CommandMenu commands={shown} active={index} onPick={complete} />
        )}

        {images.length > 0 && (
          <Attachments images={images} onRemove={(i) =>
            setImages((held) => held.filter((_, n) => n !== i))
          } />
        )}

        {attachError && <p className="composer-problem">{attachError}</p>}

        <textarea
          value={text}
          placeholder={ready ? "Ask Taurus to do something…" : "Connect a model to begin"}
          disabled={!ready}
          rows={1}
          onChange={(e) => {
            setText(e.target.value);
            setActive(0);
          }}
          onPaste={(e) => {
            const files = [...e.clipboardData.files];
            if (files.length === 0) return;
            // Only once there is an image in it: pasting a file path as text
            // is still a paste, and stealing it would break copying a path in.
            if (!files.some(isImage)) return;
            if (!vision) {
              setAttachError(
                "This model cannot read images. Switch to a vision model — on Ollama that is one like qwen3-vl or llava.",
              );
              return;
            }
            e.preventDefault();
            void attach(files);
          }}
          onKeyDown={(e) => {
            // The menu takes the keys it needs and lets every other one
            // through, so typing never has to wait for it to be dismissed.
            if (shown.length > 0) {
              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                const step = e.key === "ArrowDown" ? 1 : -1;
                setActive((index + step + shown.length) % shown.length);
                return;
              }
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
                e.preventDefault();
                complete(shown[index]);
                return;
              }
              if (e.key === "Escape") {
                e.preventDefault();
                // Settles the name so the menu closes without discarding what
                // was typed — Escape here means "stop suggesting", not "undo".
                setText(`${text} `);
                return;
              }
            }
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
            // The same rule the rail's workspace row follows: a switch closes
            // the conversation and reconnects every MCP server.
            disabled={busy}
            onClick={onPickWorkspace}
            title={
              busy
                ? "Stop the running turn before switching workspace"
                : (workspace ?? "Choose a workspace")
            }
          >
            ▤ {workspace ? basename(workspace) : "no workspace"}
          </button>
          <div className="spacer" />
          {/* The hint is the only place the slash namespace announces itself,
              and only while there is something in it to run. Paste is the
              same: a model that cannot see must not advertise it. */}
          <span className="composer-hint">
            ↵ send · ⇧↵ newline{commands.length > 0 && " · / commands"}
            {vision && " · paste an image"}
          </span>
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
