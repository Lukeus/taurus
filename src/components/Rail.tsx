import { useState } from "react";

import type { SessionMeta, Theme } from "../lib/api";
import { basename, isToday, parentDir, plural, when } from "../lib/format";
import {
  DisplayIcon,
  Logo,
  MoonIcon,
  SlidersIcon,
  SparkIcon,
  SunIcon,
  SwapIcon,
  TrashIcon,
} from "./icons";

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
  theme,
  onPickWorkspace,
  onNew,
  onOpen,
  onDelete,
  onTheme,
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
  /** The preference, not the resolved palette — the row names what was chosen. */
  theme: Theme;
  onPickWorkspace: () => void;
  onNew: () => void;
  onOpen: (sessionId: string) => void;
  onDelete: (sessionId: string) => void;
  onTheme: (theme: Theme) => void;
  onSkills: () => void;
  onAgents: () => void;
  onSettings: () => void;
}) {
  /**
   * Which row is asking to be confirmed.
   *
   * A delete takes the transcript and the checkpoints that made its turns
   * undoable, and neither comes back — so the trash can arms rather than acts.
   * Held here rather than per row so that arming a second one disarms the
   * first, which is what stops the rail filling up with pending questions.
   */
  const [arming, setArming] = useState<string | null>(null);

  const today = sessions.filter((s) => isToday(s.updated));
  const earlier = sessions.filter((s) => !isToday(s.updated));

  const item = (session: SessionMeta) => {
    const current = session.id === currentId;
    const armed = arming === session.id;
    const title = session.title || "New conversation";
    return (
      <div
        key={session.id}
        className={`rail-row${current ? " current" : ""}${armed ? " armed" : ""}`}
      >
        <button
          className="rail-item"
          // Switching mid-turn would leave the running turn streaming into a
          // transcript nobody is looking at.
          disabled={busy || current}
          title={session.title || "No turns yet"}
          onClick={() => onOpen(session.id)}
        >
          <b>{title}</b>
          <span className={armed ? "warn" : undefined}>
            {armed
              ? "delete this and its undo history?"
              : subtitle(session, current ? changedCount : null)}
          </span>
        </button>

        {armed ? (
          <>
            <button
              className="rail-delete confirm"
              title="Erase the transcript and the checkpoints that made its turns undoable"
              onClick={() => {
                setArming(null);
                onDelete(session.id);
              }}
            >
              Delete
            </button>
            <button
              className="rail-delete"
              aria-label="Keep this conversation"
              title="Keep it"
              onClick={() => setArming(null)}
            >
              ✕
            </button>
          </>
        ) : (
          <button
            className="rail-delete"
            // The backend refuses this outright for the conversation a turn is
            // running in; the other rows stay live, because deleting one of
            // them costs the running turn nothing.
            disabled={busy && current}
            aria-label={`Delete ${title}`}
            title="Delete this conversation"
            onClick={() => setArming(session.id)}
          >
            <TrashIcon />
          </button>
        )}
      </div>
    );
  };

  return (
    <aside className="rail" style={{ width }}>
      {/* Clears the macOS traffic lights, which float over this corner, and
          carries the wordmark at the one height that lines its rule up with
          the topbar's across the fold. */}
      <div className="rail-brand">
        <Logo />
        <span>taurus</span>
      </div>

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
          <span className="rail-workspace-swap">
            <SwapIcon size={12} />
          </span>
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
          <span className="glyph">
            <SparkIcon />
          </span>
          <b>Skills</b>
          {skillCount !== null && <span className="count">{skillCount}</span>}
        </button>
        <button className="rail-link" onClick={onAgents}>
          <span className="glyph">◇</span>
          <b>Agents</b>
          {agentCount !== null && <span className="count">{agentCount}</span>}
        </button>
        <button className="rail-link" onClick={onSettings}>
          <span className="glyph">
            <SlidersIcon />
          </span>
          <b>Settings</b>
        </button>
        {/* Three preferences on one row, so it cycles rather than toggles.
            A light/dark switch here would quietly throw away "follow the
            system", which is both the default and the only one of the three
            that can change on its own. */}
        <button
          className="rail-link"
          title={THEME_HINT[theme]}
          onClick={() => onTheme(NEXT_THEME[theme])}
        >
          <span className="glyph">{themeIcon(theme)}</span>
          <b>{THEME_LABEL[theme]}</b>
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

const NEXT_THEME: Record<Theme, Theme> = {
  system: "light",
  light: "dark",
  dark: "system",
};

const THEME_LABEL: Record<Theme, string> = {
  system: "Match system",
  light: "Light theme",
  dark: "Dark theme",
};

const THEME_HINT: Record<Theme, string> = {
  system: "Following your system setting. Click for light.",
  light: "Light in every workspace. Click for dark.",
  dark: "Dark in every workspace. Click to follow the system.",
};

function themeIcon(theme: Theme) {
  switch (theme) {
    case "light":
      return <SunIcon />;
    case "dark":
      return <MoonIcon />;
    case "system":
      return <DisplayIcon />;
  }
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
