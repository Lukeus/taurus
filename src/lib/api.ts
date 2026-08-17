/**
 * Typed wrappers over the Tauri command surface.
 *
 * Every payload type here is generated from Rust by ts-rs, so a change to a
 * command's shape breaks the type check rather than the running app.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { AgentSummary } from "../bindings/AgentSummary";
import type { AgentTier } from "../bindings/AgentTier";
import type { AllowedRule } from "../bindings/AllowedRule";
import type { Answer } from "../bindings/Answer";
import type { AppStatus } from "../bindings/AppStatus";
import type { Attachment } from "../bindings/Attachment";
import type { Checkpoint } from "../bindings/Checkpoint";
import type { CommandKind } from "../bindings/CommandKind";
import type { CommandSummary } from "../bindings/CommandSummary";
import type { Commit } from "../bindings/Commit";
import type { ContentBlock } from "../bindings/ContentBlock";
import type { DiffHunk } from "../bindings/DiffHunk";
import type { DiffLine } from "../bindings/DiffLine";
import type { DiffLineKind } from "../bindings/DiffLineKind";
import type { FileDiff } from "../bindings/FileDiff";
import type { CreatedSession } from "../bindings/CreatedSession";
import type { IndexProgress } from "../bindings/IndexProgress";
import type { Instructions } from "../bindings/Instructions";
import type { KeyStatus } from "../bindings/KeyStatus";
import type { McpEnvironment } from "../bindings/McpEnvironment";
import type { McpServerDraft } from "../bindings/McpServerDraft";
import type { McpServerRef } from "../bindings/McpServerRef";
import type { McpServerView } from "../bindings/McpServerView";
import type { McpTransport } from "../bindings/McpTransport";
import type { McpValue } from "../bindings/McpValue";
import type { Message } from "../bindings/Message";
import type { ModelInfo } from "../bindings/ModelInfo";
import type { PermissionDecision } from "../bindings/PermissionDecision";
import type { PermissionRequest } from "../bindings/PermissionRequest";
import type { Problem } from "../bindings/Problem";
import type { ProblemSource } from "../bindings/ProblemSource";
import type { ProviderConfig } from "../bindings/ProviderConfig";
import type { AgentProposal } from "../bindings/AgentProposal";
import type { AgentSaveTarget } from "../bindings/AgentSaveTarget";
import type { ProviderKind } from "../bindings/ProviderKind";
import type { RepoStatus } from "../bindings/RepoStatus";
import type { Restored } from "../bindings/Restored";
import type { ResumedSession } from "../bindings/ResumedSession";
import type { SaveTarget } from "../bindings/SaveTarget";
import type { Scope } from "../bindings/Scope";
import type { SearchBackend } from "../bindings/SearchBackend";
import type { SearchSettings } from "../bindings/SearchSettings";
import type { ServerStatus } from "../bindings/ServerStatus";
import type { ModelEntry } from "../bindings/ModelEntry";
import type { SessionMeta } from "../bindings/SessionMeta";
import type { SkillProposal } from "../bindings/SkillProposal";
import type { SkillSummary } from "../bindings/SkillSummary";
import type { Skipped } from "../bindings/Skipped";
import type { Step } from "../bindings/Step";
import type { StepState } from "../bindings/StepState";
import type { Theme } from "../bindings/Theme";
import type { TranscriptView } from "../bindings/TranscriptView";
import type { TurnChange } from "../bindings/TurnChange";
import type { UiEvent } from "../bindings/UiEvent";

export type {
  AgentProposal,
  AgentSaveTarget,
  AgentSummary,
  AgentTier,
  AllowedRule,
  Answer,
  AppStatus,
  Attachment,
  Checkpoint,
  CommandKind,
  CommandSummary,
  Commit,
  ContentBlock,
  CreatedSession,
  DiffHunk,
  DiffLine,
  DiffLineKind,
  FileDiff,
  IndexProgress,
  Instructions,
  KeyStatus,
  McpEnvironment,
  McpServerDraft,
  McpServerRef,
  McpServerView,
  McpTransport,
  McpValue,
  Message,
  ModelEntry,
  ModelInfo,
  PermissionDecision,
  PermissionRequest,
  Problem,
  ProblemSource,
  ProviderConfig,
  ProviderKind,
  RepoStatus,
  Restored,
  ResumedSession,
  SaveTarget,
  Scope,
  SearchBackend,
  SearchSettings,
  ServerStatus,
  SessionMeta,
  SkillProposal,
  SkillSummary,
  Skipped,
  Step,
  StepState,
  Theme,
  TranscriptView,
  TurnChange,
  UiEvent,
};

export const EVENT_PERMISSION_REQUEST = "taurus://permission-request";
export const EVENT_SKILL_PROPOSAL = "taurus://skill-proposal";
export const EVENT_AGENT_PROPOSAL = "taurus://agent-proposal";

export const getStatus = () => invoke<AppStatus>("get_status");

export const setWorkspace = (path: string) =>
  invoke<string>("set_workspace", { path });

export const listModels = (providerId: string) =>
  invoke<ModelInfo[]>("list_models", { providerId });

export const createSession = (providerId: string, model: string) =>
  invoke<CreatedSession>("create_session", { providerId, model });

/**
 * Runs one turn. Resolves when the turn is over; progress arrives on the
 * channel in order.
 */
export function sendMessage(
  sessionId: string,
  text: string,
  onEvent: (event: UiEvent) => void,
  images: Attachment[] = [],
): Promise<void> {
  const channel = new Channel<UiEvent>();
  channel.onmessage = onEvent;
  return invoke("send_message", { sessionId, text, images, onEvent: channel });
}

export const cancelSession = (sessionId: string) =>
  invoke<void>("cancel_session", { sessionId });

/** Saved conversations, newest first. `all` crosses workspaces. */
export const listSessions = (all = false) =>
  invoke<SessionMeta[]>("list_sessions", { all });

export const resumeSession = (sessionId: string, providerId?: string) =>
  invoke<ResumedSession>("resume_session", {
    sessionId,
    providerId: providerId ?? null,
  });

/**
 * Erases a saved conversation — its transcript, and the checkpoints that made
 * its turns undoable. The workspace itself is untouched.
 *
 * Rejected while that conversation is mid-turn, so the UI must be prepared for
 * a failure it did not ask a question about.
 */
export const deleteSession = (sessionId: string) =>
  invoke<void>("delete_session", { sessionId });

export const respondPermission = (id: string, decision: PermissionDecision) =>
  invoke<void>("respond_permission", { response: { id, decision } });

/**
 * Answers a question card, releasing the tool call waiting behind it.
 *
 * `id` is the call's own id, which is what the card was drawn from. One answer
 * per question, in the order they were asked; an empty one is a skip.
 */
export const answerQuestions = (id: string, answers: Answer[]) =>
  invoke<void>("answer_questions", { id, answers });

export const listPermissionRules = () =>
  invoke<AllowedRule[]>("list_permission_rules");

/** Removes a rule from one layer; the same rule may be granted in both. */
export const revokePermissionRule = (rule: string, scope: Scope) =>
  invoke<void>("revoke_permission_rule", { rule, scope });

export const listSkills = () => invoke<SkillSummary[]>("list_skills");

/** The standing brief in force, in the order it reaches the prompt. */
export const listInstructions = () =>
  invoke<Instructions[]>("list_instructions");

/** Skills and sub-agents runnable as `/name`, for completion in the composer. */
export const listCommands = () => invoke<CommandSummary[]>("list_commands");

/**
 * The sub-agent roster. Rescans the agent directories first, so what comes back
 * is what is on disk rather than what was there at startup — the whole
 * authoring surface for an agent is a text editor.
 */
export const listAgents = () => invoke<AgentSummary[]>("list_agents");

/** Characters of every request the roster costs. */
export const agentRosterCost = () => invoke<number>("agent_roster_cost");

/**
 * Writes a starter agent file in `scope` and opens it, returning its path.
 *
 * Disk stays the source of truth; this only means nobody has to already know
 * the frontmatter to write their first one.
 */
/** Every tool this session has, for the agent editor's picker. */
export const listTools = () => invoke<string[]>("list_tools");

/**
 * Writes an agent from the editor. `draft` is an `AgentProposal` minus the
 * fields only a model-made proposal carries — the backend fills in the id and
 * the review-card metadata.
 */
export const saveAgent = (
  draft: Pick<
    AgentProposal,
    "name" | "description" | "prompt" | "tools" | "max_iterations"
  >,
  target: AgentSaveTarget,
) => invoke<string>("save_agent", { draft, target });

/** Drafts an agent from a description, for the editor to fill in. */
export const generateAgent = (
  description: string,
  providerId: string,
  model: string,
) =>
  invoke<AgentProposal>("generate_agent", { description, providerId, model });

export const createAgent = (scope: Scope, name: string) =>
  invoke<string>("create_agent", { scope, name });

/**
 * Opens a layer's `mcp.json`, creating it if absent, and returns its path.
 *
 * The file stays the authority even with the panel there: the format is the one
 * Claude Desktop uses and entries get moved between the two, so anything the
 * panel cannot express is edited here. Every write the panel makes preserves
 * what it does not understand, so the two routes mix freely.
 */
export const openMcpConfig = (scope: Scope) =>
  invoke<string>("open_mcp_config", { scope });

/** Every configured server, merged across layers, with how it is doing. */
export const listMcpServers = () => invoke<McpServerView[]>("list_mcp_servers");

/**
 * Where the app looks for a stdio server's program.
 *
 * The panel shows this because "command not found" for a command that plainly
 * exists is otherwise unexplainable from inside the app — a window started from
 * the Dock has the launcher's PATH, not the shell's.
 */
export const mcpEnvironment = () => invoke<McpEnvironment>("mcp_environment");

/**
 * Writes one server to its layer's `mcp.json` and reconnects.
 *
 * `previous` is the entry being edited, and is sent whenever this is an edit
 * rather than an add. It is what a rename or a move between layers resolves
 * against: the stored secrets come from there, and it is removed afterwards if
 * the draft no longer lives at that name and scope.
 *
 * All of these return the fresh listing, so a caller never has to follow a
 * write with a read that could disagree with it.
 */
export const saveMcpServer = (
  draft: McpServerDraft,
  previous?: McpServerRef,
) => invoke<McpServerView[]>("save_mcp_server", { draft, previous });

export const deleteMcpServer = (scope: Scope, name: string) =>
  invoke<McpServerView[]>("delete_mcp_server", { scope, name });

export const setMcpServerDisabled = (
  scope: Scope,
  name: string,
  disabled: boolean,
) =>
  invoke<McpServerView[]>("set_mcp_server_disabled", { scope, name, disabled });

/**
 * Connects to one entry, reports the tools it offers, and disconnects.
 *
 * Takes the draft rather than a saved name, so an edit can be checked before it
 * is written — and nothing is registered, so testing cannot disturb a server
 * that is currently working.
 */
export const testMcpServer = (
  draft: McpServerDraft,
  previous?: McpServerRef,
) => invoke<string[]>("test_mcp_server", { draft, previous });

/**
 * Reconnects every MCP server without rescanning skills, agents, or providers.
 *
 * Narrower than {@link reloadConfig} on purpose: a change to `mcp.json` cannot
 * affect any of those, and restarting them costs a visible pause.
 */
export const reloadMcp = () => invoke<McpServerView[]>("reload_mcp");

/**
 * Re-reads every config layer, rescans skills and agents, and reconnects MCP
 * servers. Named for what it does: as `reloadSkills` it promised less, so
 * nobody who had edited an agent would think to press it.
 */
export const reloadConfig = () => invoke<void>("reload_config");

export const respondSkillProposal = (
  id: string,
  approve: boolean,
  target?: SaveTarget,
  edited?: SkillProposal,
) =>
  invoke<string | null>("respond_skill_proposal", {
    response: { id, approve, target: target ?? null, edited: edited ?? null },
  });

export const setSkillSynthesis = (enabled: boolean) =>
  invoke<void>("set_skill_synthesis", { enabled });

/** Model turns one message may take. Clamped by the host. */
export const setMaxIterations = (limit: number) =>
  invoke<void>("set_max_iterations", { limit });

/**
 * Retunes one agent's iteration limit in place, preserving everything else in
 * its file. Resolves to the file that now holds it — for a built-in that is a
 * user-tier override which did not exist before the call.
 */
export const setAgentIterations = (name: string, limit: number) =>
  invoke<string>("set_agent_iterations", { name, limit });

export const respondAgentProposal = (
  id: string,
  approve: boolean,
  target?: AgentSaveTarget,
  edited?: AgentProposal,
) =>
  invoke<string | null>("respond_agent_proposal", {
    response: { id, approve, target: target ?? null, edited: edited ?? null },
  });

export const setAgentSynthesis = (enabled: boolean) =>
  invoke<void>("set_agent_synthesis", { enabled });

export const setTheme = (theme: Theme) => invoke<void>("set_theme", { theme });

/** Which embedding model semantic search runs on. Empty turns it off. */
export const setEmbeddingModel = (model: string) =>
  invoke<void>("set_embedding_model", { model });

/**
 * Builds this workspace's code index now, resolving to its one-line summary.
 *
 * The alternative is paying for it inside the first turn that reaches for
 * `search_code`, where it is a tool call that does not return for most of a
 * minute. `onProgress` fires about twenty times over a build.
 */
export const buildIndex = (onProgress: (p: IndexProgress) => void) => {
  const channel = new Channel<IndexProgress>();
  channel.onmessage = onProgress;
  return invoke<string>("build_index", { onProgress: channel });
};

export const stopIndexBuild = () => invoke<void>("stop_index_build");

export const saveProviders = (providers: ProviderConfig[]) =>
  invoke<void>("save_providers", { providers });

/**
 * The global provider layer, which is what the settings editor must edit.
 *
 * `AppStatus.providers` is the *effective* list with this workspace's
 * overrides folded in; saving that back would write one project's settings
 * into every project's config.
 */
export const listGlobalProviders = () =>
  invoke<ProviderConfig[]>("list_global_providers");

/**
 * Where each provider's API key comes from, as `[providerId, status]` pairs.
 *
 * Status only. The key itself never crosses into the frontend: a secret handed
 * to the webview lives in JavaScript memory and in whatever the DOM does with
 * it, and the settings screen has no use for the old value — the field is a
 * place to type a new key, not to review the current one.
 */
export const listKeyStatuses = () =>
  invoke<[string, KeyStatus][]>("list_key_statuses");

/** Whether this machine has a credential store to save keys into at all. */
export const keychainAvailable = () => invoke<boolean>("keychain_available");

export const setProviderKey = (providerId: string, key: string) =>
  invoke<void>("set_provider_key", { providerId, key });

export const clearProviderKey = (providerId: string) =>
  invoke<void>("clear_provider_key", { providerId });

/**
 * The global `search.json`, plus where each backend's key comes from and
 * whether search is actually running.
 *
 * `active` is not the same as "a backend is selected": a selection with no key
 * resolves to nothing and registers no tools.
 */
export const getSearchSettings = () =>
  invoke<SearchSettings>("get_search_settings");

/** Saves the global search layer. Selecting `null` turns web search off. */
export const saveSearchSettings = (
  selected: string | null,
  backends: SearchBackend[],
) => invoke<void>("save_search_settings", { selected, backends });

export const setSearchKey = (backendId: string, key: string) =>
  invoke<void>("set_search_key", { backendId, key });

export const clearSearchKey = (backendId: string) =>
  invoke<void>("clear_search_key", { backendId });

/** Turns in this conversation that changed files, oldest first. */
export const listCheckpoints = (sessionId: string) =>
  invoke<Checkpoint[]>("list_checkpoints", { sessionId });

/**
 * Restores the workspace to just before `turn`, undoing it and every turn
 * after it.
 *
 * `dryRun` reports what that would do and writes nothing, which is how the UI
 * shows the plan before asking.
 */
export const rewindTo = (sessionId: string, turn: number, dryRun: boolean) =>
  invoke<Restored[]>("rewind_to", { sessionId, turn, dryRun });

/**
 * What one turn changed, file by file, as a diff.
 *
 * Fetched per turn rather than with the listing: a long conversation would
 * otherwise ship every diff it ever made to draw a drawer showing one.
 */
export const turnChanges = (sessionId: string, turn: number) =>
  invoke<TurnChange[]>("turn_changes", { sessionId, turn });

/** Where the workspace stands with git. Never throws for "not a repository". */
export const repoStatus = () => invoke<RepoStatus>("repo_status");

/**
 * Commits exactly the files one turn changed, leaving the index and every
 * other path alone.
 *
 * The turn is named rather than the paths: the checkpoint log is the only
 * thing that decides what goes in.
 */
export const commitTurn = (sessionId: string, turn: number, message: string) =>
  invoke<Commit>("commit_turn", { sessionId, turn, message });

export const onPermissionRequest = (
  handler: (request: PermissionRequest) => void,
): Promise<UnlistenFn> =>
  listen<PermissionRequest>(EVENT_PERMISSION_REQUEST, (e) => handler(e.payload));

export const onSkillProposal = (
  handler: (proposal: SkillProposal) => void,
): Promise<UnlistenFn> =>
  listen<SkillProposal>(EVENT_SKILL_PROPOSAL, (e) => handler(e.payload));

export const onAgentProposal = (
  handler: (proposal: AgentProposal) => void,
): Promise<UnlistenFn> =>
  listen<AgentProposal>(EVENT_AGENT_PROPOSAL, (e) => handler(e.payload));
