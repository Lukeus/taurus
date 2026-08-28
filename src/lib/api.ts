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
import type { Background } from "../bindings/Background";
import type { BackgroundJob } from "../bindings/BackgroundJob";
import type { JobOutput } from "../bindings/JobOutput";
import type { ChangedFiles } from "../bindings/ChangedFiles";
import type { Checkpoint } from "../bindings/Checkpoint";
import type { CommandKind } from "../bindings/CommandKind";
import type { CommandSummary } from "../bindings/CommandSummary";
import type { DataColumn } from "../bindings/DataColumn";
import type { DataColumnKind } from "../bindings/DataColumnKind";
import type { DataColumnProfile } from "../bindings/DataColumnProfile";
import type { DataDistinct } from "../bindings/DataDistinct";
import type { DataFormat } from "../bindings/DataFormat";
import type { DataPage } from "../bindings/DataPage";
import type { DataProfile } from "../bindings/DataProfile";
import type { DataTable } from "../bindings/DataTable";
import type { DataQueryResult } from "../bindings/DataQueryResult";
import type { DataRun } from "../bindings/DataRun";
import type { DataStep } from "../bindings/DataStep";
import type { DataValueCount } from "../bindings/DataValueCount";
import type { Dataset } from "../bindings/Dataset";
import type { Document } from "../bindings/Document";
import type { LineRange } from "../bindings/LineRange";
import type { Recipe } from "../bindings/Recipe";
import type { RecipeStep } from "../bindings/RecipeStep";
import type { RecipeTable } from "../bindings/RecipeTable";
import type { Recipes } from "../bindings/Recipes";
import type { Commit } from "../bindings/Commit";
import type { ContentBlock } from "../bindings/ContentBlock";
import type { DiffHunk } from "../bindings/DiffHunk";
import type { DiffLine } from "../bindings/DiffLine";
import type { DiffLineKind } from "../bindings/DiffLineKind";
import type { FileDiff } from "../bindings/FileDiff";
import type { SchemaCost } from "../bindings/SchemaCost";
import type { SearchResults } from "../bindings/SearchResults";
import type { SessionHit } from "../bindings/SessionHit";
import type { TranscriptMatch } from "../bindings/TranscriptMatch";
import type { ToolUsage } from "../bindings/ToolUsage";
import type { UsageReport } from "../bindings/UsageReport";
import type { FlowEdge } from "../bindings/FlowEdge";
import type { FlowNode } from "../bindings/FlowNode";
import type { FlowStage } from "../bindings/FlowStage";
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
import type { MessageKind } from "../bindings/MessageKind";
import type { ModelInfo } from "../bindings/ModelInfo";
import type { Note } from "../bindings/Note";
import type { DataOnScreen } from "../bindings/DataOnScreen";
import type { DocumentOnScreen } from "../bindings/DocumentOnScreen";
import type { OnScreen } from "../bindings/OnScreen";
import type { Selection } from "../bindings/Selection";
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
import type { Rewind } from "../bindings/Rewind";
import type { ResumedSession } from "../bindings/ResumedSession";
import type { SaveTarget } from "../bindings/SaveTarget";
import type { Scope } from "../bindings/Scope";
import type { SearchBackend } from "../bindings/SearchBackend";
import type { SearchSettings } from "../bindings/SearchSettings";
import type { SequenceMessage } from "../bindings/SequenceMessage";
import type { ServerStatus } from "../bindings/ServerStatus";
import type { ModelEntry } from "../bindings/ModelEntry";
import type { SessionMeta } from "../bindings/SessionMeta";
import type { SkillProposal } from "../bindings/SkillProposal";
import type { SkillSummary } from "../bindings/SkillSummary";
import type { Skipped } from "../bindings/Skipped";
import type { Step } from "../bindings/Step";
import type { Switch } from "../bindings/Switch";
import type { StepState } from "../bindings/StepState";
import type { TerminalEvent } from "../bindings/TerminalEvent";
import type { Theme } from "../bindings/Theme";
import type { CustomTheme } from "../bindings/CustomTheme";
import type { ThemeFile } from "../bindings/ThemeFile";
import type { ThemeModes } from "../bindings/ThemeModes";
import type { Fonts } from "../bindings/Fonts";
import type { Brand } from "../bindings/Brand";
import type { Shape } from "../bindings/Shape";
import type { ToolOutput } from "../bindings/ToolOutput";
import type { ToolResultBlock } from "../bindings/ToolResultBlock";
import type { ModelLatency } from "../bindings/ModelLatency";
import type { SpanKind } from "../bindings/SpanKind";
import type { ToolLatency } from "../bindings/ToolLatency";
import type { TraceReport } from "../bindings/TraceReport";
import type { TraceStep } from "../bindings/TraceStep";
import type { TurnTrace } from "../bindings/TurnTrace";
import type { TranscriptView } from "../bindings/TranscriptView";
import type { PendingConfig } from "../bindings/PendingConfig";
import type { TrustStatus } from "../bindings/TrustStatus";
import type { TurnChange } from "../bindings/TurnChange";
import type { UiEvent } from "../bindings/UiEvent";

export type {
  ModelLatency,
  SpanKind,
  ToolLatency,
  TraceReport,
  TraceStep,
  TurnTrace,
  AgentProposal,
  AgentSaveTarget,
  AgentSummary,
  AgentTier,
  AllowedRule,
  Answer,
  AppStatus,
  Attachment,
  Background,
  BackgroundJob,
  ChangedFiles,
  Checkpoint,
  CommandKind,
  CommandSummary,
  Commit,
  CustomTheme,
  ThemeFile,
  ThemeModes,
  Fonts,
  Brand,
  Shape,
  DataColumn,
  DataColumnKind,
  DataColumnProfile,
  DataDistinct,
  DataFormat,
  DataPage,
  DataProfile,
  DataTable,
  DataQueryResult,
  DataRun,
  DataStep,
  DataValueCount,
  Dataset,
  Document,
  LineRange,
  Recipe,
  RecipeStep,
  RecipeTable,
  Recipes,
  ContentBlock,
  CreatedSession,
  DiffHunk,
  DiffLine,
  DiffLineKind,
  FileDiff,
  SchemaCost,
  SearchResults,
  SessionHit,
  TranscriptMatch,
  ToolUsage,
  UsageReport,
  FlowEdge,
  FlowNode,
  FlowStage,
  IndexProgress,
  Instructions,
  JobOutput,
  KeyStatus,
  McpEnvironment,
  McpServerDraft,
  McpServerRef,
  McpServerView,
  McpTransport,
  McpValue,
  Message,
  MessageKind,
  ModelEntry,
  ModelInfo,
  Note,
  DataOnScreen,
  DocumentOnScreen,
  OnScreen,
  Selection,
  PermissionDecision,
  PermissionRequest,
  Problem,
  ProblemSource,
  ProviderConfig,
  ProviderKind,
  RepoStatus,
  Restored,
  Rewind,
  ResumedSession,
  SaveTarget,
  Scope,
  SearchBackend,
  SearchSettings,
  SequenceMessage,
  ServerStatus,
  SessionMeta,
  SkillProposal,
  SkillSummary,
  Skipped,
  Step,
  StepState,
  Switch,
  TerminalEvent,
  Theme,
  PendingConfig,
  ToolOutput,
  ToolResultBlock,
  TranscriptView,
  TrustStatus,
  TurnChange,
  UiEvent,
};

export const EVENT_PERMISSION_REQUEST = "taurus://permission-request";
export const EVENT_SKILL_PROPOSAL = "taurus://skill-proposal";
export const EVENT_AGENT_PROPOSAL = "taurus://agent-proposal";

/**
 * The whole of {@link AppStatus}, pushed whenever any of it moves.
 *
 * The frontend asks for status once, on mount. Everything after that arrives
 * here — a reload, a workspace switch, a settings write, a turn that left a
 * note behind. Before this, the shell found out by asking after whatever the
 * user had last clicked, so every count on the rail was as old as their last
 * unrelated action.
 */
export const EVENT_STATUS = "taurus://status";

/**
 * One conversation's listing entry, when it appears or changes.
 *
 * Singular, and merged into the list already held: a turn ending costs one
 * file read on the backend rather than a scan of every transcript in the
 * workspace.
 */
export const EVENT_SESSION = "taurus://session";

/**
 * Every file one conversation has changed, when something cut the set back.
 *
 * A turn reports what it changes on its own event stream as it changes them,
 * so this is for the one thing that moves the count the other way: a rewind.
 */
export const EVENT_CHANGED = "taurus://changed";

export const getStatus = () => invoke<AppStatus>("get_status");

export const setWorkspace = (path: string) =>
  invoke<string>("set_workspace", { path });

/**
 * Whether this workspace's own config is being read, and what it holds.
 *
 * Asked for rather than pushed, unlike everything else the shell shows: the
 * answer changes when a file appears in a directory nothing is watching — a
 * `git pull` that adds `.taurus/mcp.json` is the case this exists for, and it
 * arrives with no event attached to it. So it is re-asked at the two moments
 * that stand in for one: opening a folder, and coming back to the window.
 */
export const workspaceTrust = () => invoke<TrustStatus>("workspace_trust");

/** Lets this workspace's config take effect, and answers with the new state. */
export const trustWorkspace = () => invoke<TrustStatus>("trust_workspace");

/** Stops reading it, and answers with the new state. */
export const revokeWorkspaceTrust = () =>
  invoke<TrustStatus>("revoke_workspace_trust");

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
  /**
   * What the Data pane was showing, when the message was sent from it.
   *
   * Reaches the model and not the transcript, which is the same split a
   * `/command` expansion makes — see `taurus_host::onscreen` for why, and for
   * what it costs a conversation reopened later.
   */
  onScreen: OnScreen | null = null,
): Promise<void> {
  const channel = new Channel<UiEvent>();
  channel.onmessage = onEvent;
  return invoke("send_message", {
    sessionId,
    text,
    images,
    onScreen,
    onEvent: channel,
  });
}

/**
 * Moves this conversation to another model, or another backend, keeping
 * everything said in it.
 *
 * Answers with the live conversation's new shape, capabilities included: the
 * model being moved to may read images where the last one did not, or have a
 * different context window, and both change what the composer offers.
 *
 * Refused mid-turn. A turn reads the model out of the session on every attempt,
 * so moving it underneath one would send half an answer to one backend and half
 * to another.
 */
export const switchModel = (
  sessionId: string,
  providerId: string,
  model: string,
) => invoke<CreatedSession>("switch_model", { sessionId, providerId, model });

export const cancelSession = (sessionId: string) =>
  invoke<void>("cancel_session", { sessionId });

/**
 * Drops a live session from the backend, cancelling anything still running in
 * it.
 *
 * The conversation itself is untouched — it is on disk and reopens from the
 * rail. This releases only the in-memory copy, which the backend otherwise
 * holds, transcript and attached images included, for the life of the process.
 */
export const closeSession = (sessionId: string) =>
  invoke<void>("close_session", { sessionId });

/** Saved conversations, newest first. `all` crosses workspaces. */
export const listSessions = (all = false) =>
  invoke<SessionMeta[]>("list_sessions", { all });

/**
 * Gives a conversation a title of its own, and answers with how it now reads.
 *
 * An empty title is a clear rather than an error: the field starts out holding
 * the title derived from the first question, and emptying it is how you ask for
 * that back. The answer is the authority on how the text was shortened, so a
 * caller never has to reproduce that rule to know what it will show.
 *
 * Allowed mid-turn, unlike deleting: it touches the transcript's header and not
 * anything a running turn is appending to.
 */
export const renameSession = (sessionId: string, title: string) =>
  invoke<SessionMeta>("rename_session", { sessionId, title });

export const resumeSession = (sessionId: string, providerId?: string) =>
  invoke<ResumedSession>("resume_session", {
    sessionId,
    providerId: providerId ?? null,
  });

/**
 * The conversation one delegation had, read-only.
 *
 * Scoped to the conversation that spawned it, which is what a delegate's id is
 * unique within. Rejects rather than returning an empty list when there is no
 * such transcript, so a stale reference reads as the error it is.
 */
export const readSubagentTranscript = (sessionId: string, subagentId: string) =>
  invoke<Message[]>("read_subagent_transcript", { sessionId, subagentId });

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

/**
 * What earlier conversations in this workspace wrote down for the next one.
 *
 * Newest first, which is the order they are worth reading in and the order they
 * reach the model's prompt.
 */
export const listNotes = () => invoke<Note[]>("list_notes");

/** Drops one note and answers with what is left, so the drawer redraws from
 *  the file rather than from its own guess about what the file now says. */
export const forgetNote = (id: string) => invoke<Note[]>("forget_note", { id });

/**
 * Where the context window went — for one conversation, or for all of them.
 *
 * `null` accounts for every saved conversation in the workspace. A live
 * conversation is read from memory rather than from its transcript, so the
 * answer includes the turn that is still running.
 *
 * Not cached here. The whole point of the panel is what the window holds *now*,
 * and a figure held over from when the drawer was last opened is the one thing
 * it must not show.
 */
export const usageReport = (sessionId: string | null) =>
  invoke<UsageReport>("usage_report", { sessionId });

/**
 * Where a turn's time went, from the spans this process has finished.
 *
 * `sessionId` names one conversation; `null` reports on everything the window
 * has run since it launched — which, unlike the usage account, includes
 * conversations that have since been closed, because the source is a ring in
 * memory rather than a file on disk.
 */
export const traceReport = (sessionId: string | null) =>
  invoke<TraceReport>("trace_report", { sessionId });

/** Forgets every span recorded so far, so the next reading is of what happens
 *  next. An OTLP collector, if one is configured, keeps what it already got. */
export const clearTraces = () => invoke<void>("clear_traces");

/**
 * Conversations mentioning `query`, newest first.
 *
 * Prose only — what was typed and what the model wrote back, not tool calls
 * and not their results. Searching those would match nearly every conversation
 * for nearly every query: they are file contents and build logs. See
 * `taurus_host::search`.
 *
 * `everywhere` reaches past the open workspace, which is the question "where
 * did I do that" rather than "which conversation was that".
 */
export const searchSessions = (query: string, everywhere: boolean) =>
  invoke<SearchResults>("search_sessions", { query, everywhere });

/**
 * The data files loaded in this workspace, in the order they were loaded.
 *
 * Pointers, not contents. Everything about what is actually in one is asked
 * for separately and computed when asked — see `datasetProfile`.
 */
export const listDatasets = () => invoke<Dataset[]>("list_datasets");

/**
 * Reads a dataset in full and describes every column.
 *
 * The one call in this file that can take real time: it is a scan of the whole
 * file, and on a large one that is seconds rather than milliseconds. Callers
 * show a reading state over it rather than waiting in silence.
 */
export const datasetProfile = (name: string) =>
  invoke<DataProfile>("dataset_profile", { name });

/**
 * Every table a query here can name, with its columns and nothing counted.
 *
 * The query box's own schema, and cheap on purpose: a Parquet footer or a CSV
 * header, where `datasetProfile` is a full scan. That difference is what lets
 * completion ask for this every time the box is opened.
 *
 * A dataset whose file has moved is left out rather than failing the call.
 */
export const datasetTables = () => invoke<DataTable[]>("dataset_tables");

/** A window of a dataset's rows. The backend caps `limit`; asking for the
 *  whole file gets a page rather than a hang. */
export const datasetPage = (name: string, offset: number, limit: number) =>
  invoke<DataPage>("dataset_page", { name, offset, limit });

/**
 * Answers one read-only SQL question over every dataset loaded here.
 *
 * Every dataset is a table under its own name, so a query can join two of
 * them. The engine refuses anything that is not a SELECT, which is what lets
 * this be an ordinary call rather than one behind a confirmation — the box
 * this comes from takes arbitrary text.
 */
export const queryData = (sql: string) =>
  invoke<DataQueryResult>("query_data", { sql });

/** Drops a dataset from the list and answers with what is left. The file it
 *  pointed at is not touched. */
export const forgetDataset = (name: string) =>
  invoke<Dataset[]>("forget_dataset", { name });

/**
 * The recipes this workspace has, with anything wrong with the rest.
 *
 * Read from `.taurus/recipes` every time rather than cached: a recipe is a
 * file in the project that the agent, an editor, or a `git pull` can change,
 * and the pane showing yesterday's steps beside a Run button is the one
 * failure this list can have.
 */
export const listRecipes = () => invoke<Recipes>("list_recipes");

/**
 * Runs a recipe and writes the file it names.
 *
 * The one call in this file that changes the workspace. It asks nothing first,
 * for the same reason `queryData` does not — the person clicked a button that
 * says where it writes. What is still refused, one layer down, is a *step*
 * that writes somewhere other than that path.
 */
export const runRecipe = (name: string) => invoke<DataRun>("run_recipe", { name });

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

/** Picks the custom theme painting over that palette. Empty is the built-in. */
export const setThemeId = (id: string) => invoke<void>("set_theme_id", { id });

/**
 * Every custom theme on the machine.
 *
 * Asked for when the picker opens rather than carried on the status, because
 * each theme brings its logo inlined and the status is pushed after anything
 * that moves a number on screen.
 */
export const listThemes = () => invoke<CustomTheme[]>("list_themes");

export const saveTheme = (scope: Scope, id: string, theme: ThemeFile) =>
  invoke<string>("save_theme", { scope, id, theme });

export const deleteTheme = (scope: Scope, id: string) =>
  invoke<void>("delete_theme", { scope, id });

/** Creates the themes folder if it is not there, and says where it is. */
export const themesDir = (scope: Scope) => invoke<string>("themes_dir", { scope });

/**
 * Repaints the native window behind the webview.
 *
 * The window's own ground is the platform's, not the stylesheet's — it is what
 * shows for the frame before the document paints and in the corners a webview
 * does not cover. `paint_window` sets it at launch from the settings file, and
 * this is the other half: a theme picked while the app is running has to move
 * it too, or the ink around the edges stays the colour of a palette nobody is
 * looking at any more.
 */
export const setWindowBackground = (color: string) =>
  invoke<void>("set_window_background", { color });

/**
 * Which embedding model semantic search runs on, and which backend serves it.
 * An empty model turns search off; an empty provider means the one the
 * conversation is using.
 *
 * Both at once because they are one decision — a model saved without a provider
 * embeds on whichever backend the conversation is on, and Anthropic has no
 * embedding endpoint at all.
 */
export const setEmbeddingModel = (model: string, provider = "") =>
  invoke<void>("set_embedding_model", { model, provider });

/**
 * The optional second stage that reorders search results before the model
 * reads them. An empty model turns it off; an empty provider means the one the
 * index already embeds on.
 *
 * Both at once because they are one decision — a model saved without a
 * provider would rerank on whichever backend the conversation is using, which
 * for a local Ollama is one that cannot rerank at all.
 */
export const setRerank = (model: string, provider: string) =>
  invoke<void>("set_rerank", { model, provider });

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
 * shows the plan before asking. It returns the same warnings a real rewind
 * does, because the moment they are worth reading is the moment before the
 * button.
 */
export const rewindTo = (sessionId: string, turn: number, dryRun: boolean) =>
  invoke<Rewind>("rewind_to", { sessionId, turn, dryRun });

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

/**
 * Re-reads the files a person edits in an editor: instructions, skills,
 * sub-agents, hooks.
 *
 * Called when the window regains focus, which is when somebody has most likely
 * just been in an editor writing one of them. Nothing watches those directories
 * — see the backend's `refresh_config` for why — so returning to the window is
 * the closest thing to an event their arrival has.
 *
 * The same fingerprint check a turn makes, so this is a `stat` per file when
 * nothing moved. Not called while a turn is running: a turn runs against the
 * brief, roster and catalog it started with, and the turn will refresh on its
 * own boundary anyway.
 */
export const rescanLibrary = () => invoke<void>("rescan_library");

/* ----------------------------------------------------------------- canvas */

/**
 * One text file, for the editor beside the conversation.
 *
 * Called when a document is opened, and again whenever what is held might be
 * stale — the model wrote to it, the window came back into focus. Cheap enough
 * to be the answer to both: a file small enough to edit is one read.
 *
 * The path is workspace-relative and goes through the same guard every tool
 * read does, because it arrives from a transcript that may have been reopened
 * or hand-edited.
 */
export const openDocument = (path: string) =>
  invoke<Document>("open_document", { path });

/* ------------------------------------------------------------- background */

/**
 * One look at the commands running in the background.
 *
 * Polled rather than pushed, and that is a decision rather than a shortcut. The
 * alternative is a subscription per job with a lifetime to get right at both
 * ends — and the thing being subscribed to is a buffer that is the record
 * anyway, so a missed message would cost nothing a later read does not repair.
 * A tab that is not on screen asks for nothing; a tab that is asks four times a
 * second, which for text nobody types into is well under what anyone can see.
 *
 * `cursor` is the pane's own place in the output — `0` for a first look, and
 * otherwise whatever the last answer carried. `check_command` keeps a separate
 * one, so the model and the window never take lines from each other.
 */
export const background = (watching: number | null, cursor: number) =>
  invoke<Background>("background", { watching, cursor });

/** Ends one background command, the same way `stop_command` does for the model. */
export const stopBackground = (id: number) =>
  invoke<string>("background_stop", { id });

/* --------------------------------------------------------------- terminal */

/**
 * Starts a shell in the dock, streaming what it prints to `onEvent`.
 *
 * Resolves with the id every other call here takes. The size is the pane's real
 * one, measured after layout: a shell told the wrong geometry wraps its first
 * prompt at the wrong column, and no later resize redraws a line that has
 * already been printed.
 */
export function openTerminal(
  rows: number,
  cols: number,
  onEvent: (event: TerminalEvent) => void,
  cwd?: string,
): Promise<string> {
  const channel = new Channel<TerminalEvent>();
  channel.onmessage = onEvent;
  return invoke<string>("terminal_open", { cwd, rows, cols, onEvent: channel });
}

/**
 * Sends keystrokes to a shell.
 *
 * `data` is what the emulator produced rather than what was typed: an arrow key
 * arrives here as the escape sequence a terminal would have sent, and a paste
 * arrives as its whole text at once.
 */
export const writeTerminal = (id: string, data: string) =>
  invoke<void>("terminal_write", { id, data });

/** Tells a shell how big its window is now, so full-screen programs redraw. */
export const resizeTerminal = (id: string, rows: number, cols: number) =>
  invoke<void>("terminal_resize", { id, rows, cols });

/** Ends a shell, and anything it is running. */
export const closeTerminal = (id: string) => invoke<void>("terminal_close", { id });

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

export const onStatus = (handler: (status: AppStatus) => void): Promise<UnlistenFn> =>
  listen<AppStatus>(EVENT_STATUS, (e) => handler(e.payload));

export const onSession = (handler: (session: SessionMeta) => void): Promise<UnlistenFn> =>
  listen<SessionMeta>(EVENT_SESSION, (e) => handler(e.payload));

export const onChanged = (
  handler: (changed: ChangedFiles) => void,
): Promise<UnlistenFn> =>
  listen<ChangedFiles>(EVENT_CHANGED, (e) => handler(e.payload));
