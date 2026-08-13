import type { SessionMeta } from "../lib/api";
import { basename, isToday, parentDir, plural, when } from "../lib/format";

/** How the provider behind the current session is actually doing. */
export type ProviderHealth =
  | { state: "unknown" }
  | { state: "none" }
  | { state: "connected"; id: string; models: number }
  | { state: "unreachable"; id: string };

/**
 * The left rail.
 *
 * Conversations are the app's spine, so they are always on screen rather than
 * behind a drawer: switching between two of them is a thing people do many
 * times an hour, and every click of indirection is paid every time. The rest
 * of the rail is the things that are true of the workspace as a whole — which
 * folder, which skills, whether the model is answering.
 */
export function Rail({
  width,
  workspace,
  sessions,
  currentId,
  changedCount,
  busy,
  skillCount,
  agentCount,
  health,
  onPickWorkspace,
  onNew,
  onOpen,
  onSkills,
  onAgents,
  onSettings,
}: {
  /** Set by the handle beside it; the rail only has to wear the number. */
  width: number;
  workspace: string | null;
  sessions: SessionMeta[];
  currentId: string | undefined;
  changedCount: number;
  busy: boolean;
  skillCount: number | null;
  agentCount: number | null;
  health: ProviderHealth;
  onPickWorkspace: () => void;
  onNew: () => void;
  onOpen: (sessionId: string) => void;
  onSkills: () => void;
  onAgents: () => void;
  onSettings: () => void;
}) {
  const today = sessions.filter((s) => isToday(s.updated));
  const earlier = sessions.filter((s) => !isToday(s.updated));

  const item = (session: SessionMeta) => (
    <button
      key={session.id}
      className={`rail-item${session.id === currentId ? " current" : ""}`}
      // Switching mid-turn would leave the running turn streaming into a
      // transcript nobody is looking at.
      disabled={busy || session.id === currentId}
      title={session.title || "No turns yet"}
      onClick={() => onOpen(session.id)}
    >
      <b>{session.title || "New conversation"}</b>
      <span>{subtitle(session, session.id === currentId ? changedCount : null)}</span>
    </button>
  );

  return (
    <aside className="rail" style={{ width }}>
      <div className="rail-drag" />

      <div className="rail-pad">
        <button
          className="rail-workspace"
          onClick={onPickWorkspace}
          title={workspace ?? "Choose a workspace"}
        >
          <span className="mark">t</span>
          <span className="rail-workspace-name">
            <b>{workspace ? basename(workspace) : "No workspace"}</b>
            <span>{workspace ? parentDir(workspace) : "choose a folder"}</span>
          </span>
          <span className="rail-workspace-swap">⇅</span>
        </button>
      </div>

      <div className="rail-pad">
        <button className="primary rail-new" onClick={onNew} disabled={busy}>
          New conversation
        </button>
      </div>

      <div className="rail-scroll">
        {sessions.length === 0 && (
          <p className="rail-empty">
            Nothing saved yet. Every conversation is written to disk as it
            happens.
          </p>
        )}
        {today.length > 0 && (
          <>
            <div className="rail-group micro">Today</div>
            <div className="rail-list">{today.map(item)}</div>
          </>
        )}
        {earlier.length > 0 && (
          <>
            <div className="rail-group micro">Earlier</div>
            <div className="rail-list">{earlier.map(item)}</div>
          </>
        )}
      </div>

      <div className="rail-foot">
        <button className="rail-link accent" onClick={onSkills}>
          <span className="glyph">✦</span>
          <b>Skills</b>
          {skillCount !== null && <span className="count">{skillCount}</span>}
        </button>
        <button className="rail-link" onClick={onAgents}>
          <span className="glyph">◇</span>
          <b>Agents</b>
          {agentCount !== null && <span className="count">{agentCount}</span>}
        </button>
        <button className="rail-link" onClick={onSettings}>
          <span className="glyph">◈</span>
          <b>Settings</b>
        </button>
        <div className="rail-status" title={healthTitle(health)}>
          <span className={`dot ${healthDot(health)}`} />
          <span>{healthLabel(health)}</span>
        </div>
      </div>
    </aside>
  );
}

/**
 * What the rail says under a conversation's title.
 *
 * For the open one that is how much of the workspace it has rewritten, which
 * is the fact you want before switching away from it. For the rest the model
 * and the time are all that has been read off disk.
 */
function subtitle(session: SessionMeta, changed: number | null): string {
  const ago = when(session.updated);
  if (changed === null) return `${session.model} · ${ago}`;
  return changed === 0
    ? `read-only · ${ago}`
    : `${plural(changed, "file")} changed · ${ago}`;
}

function healthDot(health: ProviderHealth): string {
  switch (health.state) {
    case "connected":
      return "ok";
    case "unreachable":
      return "error";
    default:
      return "";
  }
}

function healthLabel(health: ProviderHealth): string {
  switch (health.state) {
    case "connected":
      return `${health.id} · ${plural(health.models, "model")}`;
    case "unreachable":
      return `${health.id} · unreachable`;
    case "none":
      return "no provider configured";
    case "unknown":
      return "connecting…";
  }
}

function healthTitle(health: ProviderHealth): string {
  switch (health.state) {
    case "unreachable":
      return "Taurus could not list models from this provider. Check it is running, and its base URL in Settings.";
    case "none":
      return "Add a provider in Settings to start a conversation.";
    default:
      return "";
  }
}
