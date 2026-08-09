/**
 * Typed wrappers over the Tauri command surface.
 *
 * Every payload type here is generated from Rust by ts-rs, so a change to a
 * command's shape breaks the type check rather than the running app.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { AppStatus } from "../bindings/AppStatus";
import type { CreatedSession } from "../bindings/CreatedSession";
import type { ModelInfo } from "../bindings/ModelInfo";
import type { PermissionDecision } from "../bindings/PermissionDecision";
import type { PermissionRequest } from "../bindings/PermissionRequest";
import type { ProviderConfig } from "../bindings/ProviderConfig";
import type { SaveTarget } from "../bindings/SaveTarget";
import type { SkillProposal } from "../bindings/SkillProposal";
import type { SkillSummary } from "../bindings/SkillSummary";
import type { UiEvent } from "../bindings/UiEvent";

export type {
  AppStatus,
  CreatedSession,
  ModelInfo,
  PermissionDecision,
  PermissionRequest,
  ProviderConfig,
  SaveTarget,
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

export const respondPermission = (id: string, decision: PermissionDecision) =>
  invoke<void>("respond_permission", { response: { id, decision } });

export const listPermissionRules = () =>
  invoke<string[]>("list_permission_rules");

export const revokePermissionRule = (rule: string) =>
  invoke<void>("revoke_permission_rule", { rule });

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

export const onPermissionRequest = (
  handler: (request: PermissionRequest) => void,
): Promise<UnlistenFn> =>
  listen<PermissionRequest>(EVENT_PERMISSION_REQUEST, (e) => handler(e.payload));

export const onSkillProposal = (
  handler: (proposal: SkillProposal) => void,
): Promise<UnlistenFn> =>
  listen<SkillProposal>(EVENT_SKILL_PROPOSAL, (e) => handler(e.payload));
