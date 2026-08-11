/**
 * Typed wrappers over the Tauri command surface.
 *
 * Every payload type here is generated from Rust by ts-rs, so a change to a
 * command's shape breaks the type check rather than the running app.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { AllowedRule } from "../bindings/AllowedRule";
import type { AppStatus } from "../bindings/AppStatus";
import type { Checkpoint } from "../bindings/Checkpoint";
import type { ContentBlock } from "../bindings/ContentBlock";
import type { CreatedSession } from "../bindings/CreatedSession";
import type { Message } from "../bindings/Message";
import type { ModelInfo } from "../bindings/ModelInfo";
import type { PermissionDecision } from "../bindings/PermissionDecision";
import type { PermissionRequest } from "../bindings/PermissionRequest";
import type { ProviderConfig } from "../bindings/ProviderConfig";
import type { ProviderKind } from "../bindings/ProviderKind";
import type { Restored } from "../bindings/Restored";
import type { ResumedSession } from "../bindings/ResumedSession";
import type { SaveTarget } from "../bindings/SaveTarget";
import type { Scope } from "../bindings/Scope";
import type { SessionMeta } from "../bindings/SessionMeta";
import type { SkillProposal } from "../bindings/SkillProposal";
import type { SkillSummary } from "../bindings/SkillSummary";
import type { UiEvent } from "../bindings/UiEvent";

export type {
  AllowedRule,
  AppStatus,
  Checkpoint,
  ContentBlock,
  CreatedSession,
  Message,
  ModelInfo,
  PermissionDecision,
  PermissionRequest,
  ProviderConfig,
  ProviderKind,
  Restored,
  ResumedSession,
  SaveTarget,
  Scope,
  SessionMeta,
  SkillProposal,
  SkillSummary,
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

export const respondPermission = (id: string, decision: PermissionDecision) =>
  invoke<void>("respond_permission", { response: { id, decision } });

export const listPermissionRules = () =>
  invoke<AllowedRule[]>("list_permission_rules");

/** Removes a rule from one layer; the same rule may be granted in both. */
export const revokePermissionRule = (rule: string, scope: Scope) =>
  invoke<void>("revoke_permission_rule", { rule, scope });

export const listSkills = () => invoke<SkillSummary[]>("list_skills");

export const reloadSkills = () => invoke<number>("reload_skills");

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
