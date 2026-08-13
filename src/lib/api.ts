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
import type { AppStatus } from "../bindings/AppStatus";
import type { Checkpoint } from "../bindings/Checkpoint";
import type { ContentBlock } from "../bindings/ContentBlock";
import type { CreatedSession } from "../bindings/CreatedSession";
import type { KeyStatus } from "../bindings/KeyStatus";
import type { Message } from "../bindings/Message";
import type { ModelInfo } from "../bindings/ModelInfo";
import type { PermissionDecision } from "../bindings/PermissionDecision";
import type { PermissionRequest } from "../bindings/PermissionRequest";
import type { Problem } from "../bindings/Problem";
import type { ProblemSource } from "../bindings/ProblemSource";
import type { ProviderConfig } from "../bindings/ProviderConfig";
import type { ProviderKind } from "../bindings/ProviderKind";
import type { Restored } from "../bindings/Restored";
import type { ResumedSession } from "../bindings/ResumedSession";
import type { SaveTarget } from "../bindings/SaveTarget";
import type { Scope } from "../bindings/Scope";
import type { SearchBackend } from "../bindings/SearchBackend";
import type { SearchSettings } from "../bindings/SearchSettings";
import type { ModelEntry } from "../bindings/ModelEntry";
import type { SessionMeta } from "../bindings/SessionMeta";
import type { SkillProposal } from "../bindings/SkillProposal";
import type { SkillSummary } from "../bindings/SkillSummary";
import type { Theme } from "../bindings/Theme";
import type { UiEvent } from "../bindings/UiEvent";

export type {
  AgentSummary,
  AgentTier,
  AllowedRule,
  AppStatus,
  Checkpoint,
  ContentBlock,
  CreatedSession,
  KeyStatus,
  Message,
  ModelEntry,
  ModelInfo,
  PermissionDecision,
  PermissionRequest,
  Problem,
  ProblemSource,
  ProviderConfig,
  ProviderKind,
  Restored,
  ResumedSession,
  SaveTarget,
  Scope,
  SearchBackend,
  SearchSettings,
  SessionMeta,
  SkillProposal,
  SkillSummary,
  Theme,
  UiEvent,
};

export const EVENT_PERMISSION_REQUEST = "taurus://permission-request";
export const EVENT_SKILL_PROPOSAL = "taurus://skill-proposal";

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
): Promise<void> {
  const channel = new Channel<UiEvent>();
  channel.onmessage = onEvent;
  return invoke("send_message", { sessionId, text, onEvent: channel });
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

export const listPermissionRules = () =>
  invoke<AllowedRule[]>("list_permission_rules");

/** Removes a rule from one layer; the same rule may be granted in both. */
export const revokePermissionRule = (rule: string, scope: Scope) =>
  invoke<void>("revoke_permission_rule", { rule, scope });

export const listSkills = () => invoke<SkillSummary[]>("list_skills");

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
export const createAgent = (scope: Scope, name: string) =>
  invoke<string>("create_agent", { scope, name });

/**
 * Opens a layer's `mcp.json`, creating it if absent, and returns its path.
 *
 * Servers are configured by editing that file: the format is the one Claude
 * Desktop uses and entries get moved between the two, so the app points at the
 * file rather than reimplementing a schema it does not own.
 */
export const openMcpConfig = (scope: Scope) =>
  invoke<string>("open_mcp_config", { scope });

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

export const setTheme = (theme: Theme) => invoke<void>("set_theme", { theme });

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

export const onPermissionRequest = (
  handler: (request: PermissionRequest) => void,
): Promise<UnlistenFn> =>
  listen<PermissionRequest>(EVENT_PERMISSION_REQUEST, (e) => handler(e.payload));

export const onSkillProposal = (
  handler: (proposal: SkillProposal) => void,
): Promise<UnlistenFn> =>
  listen<SkillProposal>(EVENT_SKILL_PROPOSAL, (e) => handler(e.payload));
