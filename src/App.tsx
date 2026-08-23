import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useShallow } from "zustand/react/shallow";

import { Attachments } from "./components/Attachments";
import { CommandMenu, commandQuery, matches } from "./components/CommandMenu";
import { ConversationTitle } from "./components/ConversationTitle";
import { PermissionDialog } from "./components/PermissionDialog";
import { TrustBanner } from "./components/TrustBanner";
import { PlanPanel } from "./components/PlanPanel";
import { Rail, type ProviderHealth } from "./components/Rail";
import {
  RAIL_WIDTH,
  ResizeHandle,
  TERMINAL_HEIGHT,
  useResizableHeight,
  useResizableWidth,
} from "./components/ResizeHandle";
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

/*
 * The panels that open over the app, loaded when one is first needed rather
 * than while the window is still trying to paint.
 *
 * Every one of these already mounts behind a flag, so nothing about *when*
 * they appear changes. What changes is when their code is read and evaluated:
 * `Settings` alone is the largest module in the frontend, and it and the five
 * drawers were parsed on the main thread before React's first render, for a
 * user who in most sessions opens none of them. There is no download to save
 * in Tauri — the chunks are on disk beside the app — so this is purely about
 * getting the parse off the path to a usable window.
 *
 * One list, used twice: `lazy` below hangs off it, and `warm` walks it once
 * the window is idle. That second half is what keeps this from being a trade —
 * without it the parse merely moves onto the click that opens the drawer.
 */
const PANELS = {
  settings: () => import("./components/Settings"),
  skills: () => import("./components/SkillsDrawer"),
  agents: () => import("./components/AgentsDrawer"),
  mcp: () => import("./components/McpDrawer"),
  memory: () => import("./components/MemoryDrawer"),
  changes: () => import("./components/ChangesDrawer"),
  delegate: () => import("./components/DelegateTranscript"),
};

const Settings = lazy(() => PANELS.settings().then((m) => ({ default: m.Settings })));
const SkillsDrawer = lazy(() =>
  PANELS.skills().then((m) => ({ default: m.SkillsDrawer })),
);
const AgentsDrawer = lazy(() =>
  PANELS.agents().then((m) => ({ default: m.AgentsDrawer })),
);
const McpDrawer = lazy(() => PANELS.mcp().then((m) => ({ default: m.McpDrawer })));
const MemoryDrawer = lazy(() =>
  PANELS.memory().then((m) => ({ default: m.MemoryDrawer })),
);
const ChangesDrawer = lazy(() =>
  PANELS.changes().then((m) => ({ default: m.ChangesDrawer })),
);
const DelegateTranscript = lazy(() =>
  PANELS.delegate().then((m) => ({ default: m.DelegateTranscript })),
);

/*
 * The Data pane, deliberately not in `PANELS` either.
 *
 * `PANELS` is warmed at idle because a drawer should open instantly, and every
 * session has drawers. This is a surface most workspaces never have anything
 * to put in — the tab does not exist until a dataset is loaded — so paying to
 * parse it on every launch would be paying for a feature nobody in that
 * session is using. It loads when the tab is first chosen, and the module map
 * answers every switch after that.
 */
const DataPane = lazy(() =>
  import("./components/DataPane").then((m) => ({ default: m.DataPane })),
);

/*
 * The terminal, deliberately not in `PANELS`.
 *
 * Everything above is warmed once the window goes idle, because a drawer is
 * small and opening one should be instant. This is neither: it carries a whole
 * terminal emulator, which is the largest thing the frontend can import, and a
 * session that never opens the dock should never pay to parse it. So it loads
 * on the first ⌃` and not before — and once loaded it stays, because the module
 * map answers the second import.
 */
const TerminalDock = lazy(() =>
  import("./components/TerminalDock").then((m) => ({ default: m.TerminalDock })),
);

/**
 * Reads the panels in once the window has nothing better to do.
 *
 * A dynamic import is answered from the module map the second time, so this
 * costs nothing at the point of use — it only decides *when* the cost is paid.
 * Idle rather than on a timer, and a timeout so a permanently busy window still
 * gets there; `requestIdleCallback` is missing on some WebKit builds, which is
 * what the fallback is for.
 */
function warmPanels() {
  const read = () => Object.values(PANELS).forEach((load) => void load());
  if (typeof requestIdleCallback === "function") {
    requestIdleCallback(read, { timeout: 2_000 });
  } else {
    setTimeout(read, 500);
  }
}

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
      stopping: s.stopping,
      resuming: s.resuming,
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
      switchModel: s.switchModel,
      setWorkspace: s.setWorkspace,
      dismissError: s.dismissError,
      answerPermission: s.answerPermission,
      answerQuestions: s.answerQuestions,
      resolveProposal: s.resolveProposal,
      resolveAgentProposal: s.resolveAgentProposal,
      decideTrust: s.decideTrust,
      datasets: s.datasets,
      refreshDatasets: s.refreshDatasets,
      forgetDataset: s.forgetDataset,
    })),
  );
  const rail = useResizableWidth({ storageKey: "taurus.railWidth", ...RAIL_WIDTH });
  const dock = useResizableHeight({
    storageKey: "taurus.terminalHeight",
    ...TERMINAL_HEIGHT,
  });
  /**
   * Whether the terminal dock is showing.
   *
   * Unmounted rather than hidden when it is not: hiding it would keep a shell
   * running behind a pane nobody can see, and a laid-out-to-nothing terminal
   * would still be told it is zero columns wide. Closing it ends the shell, the
   * same as closing a terminal window anywhere else.
   */
  const [terminalOpen, setTerminalOpen] = useState(false);
  /**
   * Which surface the centre column is showing.
   *
   * A mode rather than a drawer, because the alternative to the conversation
   * here is not something laid *over* it — it is the other thing this window is
   * for. The rail and the composer are outside it and never move, so the
   * conversation is one click away and typing still starts a turn from either
   * side.
   *
   * Falls back to the transcript on its own when the last dataset is forgotten
   * — see the effect below. A mode with nothing behind it is a blank pane and
   * a tab that has gone.
   */
  const [pane, setPane] = useState<"conversation" | "data">("conversation");
  /** Which dataset the Data pane has open. Held here rather than in the pane
   *  so a card in the transcript can choose one — see `DatasetCard`. */
  const [dataset, setDataset] = useState<string | null>(null);
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
    warmPanels();
    // Intentionally once: init wires the event listeners.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /*
   * Control-backtick shows and hides the terminal, which is what it does in
   * every other editor a developer has open at the same time.
   *
   * On the window rather than on the dock, because the point of a toggle is
   * that it works when the thing is not there — and once the dock has focus,
   * every other key belongs to the shell inside it. Ctrl on all three
   * platforms, not Cmd: a macOS terminal has always been reached with Control
   * here, and ⌘` is the system's "next window".
   */
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key !== "`" || !e.ctrlKey || e.metaKey || e.altKey) return;
      e.preventDefault();
      setTerminalOpen((open) => !open);
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
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
  const busy = store.busy;
  useEffect(() => {
    const ask = () => {
      void refresh();
      // The other thing that changed while the window was not looked at: a
      // skill or an agent written in an editor. Skipped mid-turn, because a
      // turn runs against the catalog and roster it started with — and the
      // turn about to finish refreshes them itself.
      if (!busy) void api.rescanLibrary().catch(() => {});
    };
    window.addEventListener("focus", ask);
    return () => window.removeEventListener("focus", ask);
  }, [refresh, busy]);

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

  // The switch disappears with the last dataset, so a window still showing the
  // Data pane would be left on a surface with no way back to the conversation.
  // Covers forgetting the last one and switching to a folder that has none.
  useEffect(() => {
    if (store.datasets.length === 0) setPane("conversation");
  }, [store.datasets.length]);

  /**
   * Takes the conversation on screen to another model, or opens one there when
   * there is nothing to take.
   *
   * Both pickers go through here. Neither starts a new conversation any more:
   * the usual reason to reach for one of them is a second opinion on the
   * question just asked, and answering that with a blank transcript meant
   * retyping it. Nothing in the history is provider-shaped, so carrying it
   * across is a matter of saying so — see `switch_model`.
   */
  /*
   * Whether a move to another model or backend is in flight.
   *
   * A switch recreates the session, which is a round trip and a capability
   * lookup. The pickers stayed live through it, so a second change could be
   * made against a session the first was still replacing — and nothing on
   * screen said the first had been heard.
   */
  const [moving, setMoving] = useState(false);
  const moveTo = async (provider: string, model: string) => {
    setMoving(true);
    try {
      await (store.session
        ? store.switchModel(provider, model)
        : store.startSession(provider, model));
    } finally {
      setMoving(false);
    }
  };

  /**
   * Switches provider, which means moving the conversation to that backend's
   * default model — a provider on its own does not answer anything.
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
      if (first) await moveTo(id, first);
    } catch {
      setModels("failed");
      // A backend with no model listing is still usable when the config names
      // what to talk to — an Azure APIM route often exposes the chat endpoint
      // and nothing else.
      const named = config?.default_model ?? offered("failed", config)[0]?.id;
      if (named) await moveTo(id, named);
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
  /*
   * How many skills and agents there are, as one value to notice moving.
   *
   * The `/` menu is a snapshot of the last scan, and this is what tells it the
   * scan has been redone: a rescan that found something new emits a status with
   * a different count, and the composer re-reads the namespace off the back of
   * it. Counting rather than comparing the lists themselves because the counts
   * are already on screen — the rail draws both — and a menu that refreshed on
   * an unchanged catalog would refetch on every status.
   */
  const library = `${store.status?.skill_count ?? 0}:${store.status?.agent_count ?? 0}`;

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

  /** Opens a dataset in the Data pane, from a card in the transcript. */
  const showDataset = (name: string) => {
    setDataset(name);
    setPane("data");
  };

  const forgetDataset = (name: string) => void store.forgetDataset(name);

  return (
    <div className="app">
      <Rail
        width={rail.size}
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
        onTerminal={() => setTerminalOpen((open) => !open)}
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
              disabled={store.busy || moving}
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
            disabled={store.busy || moving || !providerId}
            onChange={(e) => providerId && moveTo(providerId, e.target.value)}
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

        {/* Only once there is something behind it. A workspace that has never
            loaded a file shows no switch at all, which is the whole of how
            this surface stays out of the way of everyone not using it — the
            same rule the composer's `/` hint and the rail's MCP badge follow.
            Drawn above the pane rather than in the topbar: the topbar names
            the conversation and the model, and neither of those changes when
            the centre column does. */}
        {store.datasets.length > 0 && (
          <div className="pane-switch" role="tablist" aria-label="View">
            <button
              role="tab"
              aria-selected={pane === "conversation"}
              className={`seg${pane === "conversation" ? " on" : ""}`}
              onClick={() => setPane("conversation")}
            >
              Conversation
            </button>
            <button
              role="tab"
              aria-selected={pane === "data"}
              className={`seg${pane === "data" ? " on" : ""}`}
              onClick={() => setPane("data")}
            >
              Data
              <span className="count">{store.datasets.length}</span>
            </button>
          </div>
        )}

        <main>
          {pane === "data" ? (
            <Suspense fallback={<div className="data-pane" />}>
              <DataPane
                datasets={store.datasets}
                selected={dataset}
                onSelect={setDataset}
                onForget={forgetDataset}
                onRan={() => void store.refreshDatasets()}
              />
            </Suspense>
          ) : (
            <TranscriptPane
              busy={store.busy}
              stopping={store.stopping}
              pending={store.resuming}
              onAnswer={store.answerQuestions}
              onOpenDelegate={setDelegate}
              onOpenDataset={showDataset}
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
          )}

          {/* Kept out of the Data pane. A proposal is about the conversation
              that raised it, and answering one from a screen that does not
              show it would be approving something you cannot read. */}
          {pane === "conversation" &&
            (store.proposals.length > 0 ||
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
          stopping={store.stopping}
          ready={!!store.session}
          vision={store.session?.vision ?? false}
          workspace={workspace}
          onPickWorkspace={pickWorkspace}
          library={library}
          onSend={store.send}
          onStop={store.stop}
        />

        {/* Below the composer rather than between it and the transcript: the
            box you type in belongs to the conversation above it, and a
            terminal wedged between the two would separate a message from the
            thread it is being written into. Mounted only while it is showing
            — see `terminalOpen`. */}
        {terminalOpen && (
          <>
            <ResizeHandle pane={dock} label="Terminal height" />
            <Suspense fallback={<div className="dock-loading" />}>
              <div className="dock-slot" style={{ height: dock.size }}>
                <TerminalDock
                  // A new folder is a new shell. Keyed rather than handled
                  // inside, so the old one is torn down by the same path that
                  // closes the dock instead of a second one that has to agree
                  // with it.
                  key={workspace ?? "none"}
                  workspace={workspace}
                  theme={theme ?? "system"}
                  onClose={() => setTerminalOpen(false)}
                />
              </div>
            </Suspense>
          </>
        )}
      </div>

      {store.permission && (
        <PermissionDialog
          request={store.permission}
          onDecide={store.answerPermission}
        />
      )}

      {/* One boundary for all of them: they are mutually exclusive in practice
          and none is ever nested inside another, so a boundary each would be
          seven of the same thing. `null` rather than a spinner because by the
          time a drawer is opened its module is almost always already in hand —
          see `warmPanels` — and a flash of skeleton for a frame that usually
          does not happen reads worse than the drawer simply appearing. */}
      <Suspense fallback={null}>
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
      </Suspense>
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
  stopping,
  ready,
  vision,
  workspace,
  library,
  onPickWorkspace,
  onSend,
  onStop,
}: {
  busy: boolean;
  /** Pressed Stop, and the turn has not finished unwinding yet. */
  stopping: boolean;
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
  /**
   * A signature of how many skills and agents there are.
   *
   * Not shown. It is what the `/` namespace re-reads on: the list below is the
   * last scan the backend did, and this changing is the only signal that it did
   * another one. See where `App` builds it.
   */
  library: string;
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
  const box = useRef<HTMLTextAreaElement>(null);

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

  // Fetched once the composer is usable, again whenever a workspace change
  // could have brought a different library with it, and again whenever a rescan
  // found a different number of things to offer.
  useEffect(() => {
    if (!ready) return;
    api.listCommands().then(setCommands).catch(() => setCommands([]));
  }, [ready, workspace, library]);

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
    // Sending with Enter never lost focus; sending with the button did, so the
    // next thing typed went nowhere. Both routes end in the same place.
    box.current?.focus();
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
          ref={box}
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
            // Disabled while it takes, because a second press does nothing the
            // first did not — and a button that still says "Stop" after being
            // pressed reads as one that did not register.
            <button
              className="danger composer-send"
              onClick={onStop}
              disabled={stopping}
            >
              {stopping ? "Stopping…" : "Stop"}
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
