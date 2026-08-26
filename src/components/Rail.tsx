import { type ReactNode, useState } from "react";

import type { SessionMeta, Theme } from "../lib/api";
import { basename, isToday, parentDir, plural, when } from "../lib/format";
import { useSections } from "../lib/sections";
import {
  DelegateIcon,
  DisplayIcon,
  GaugeIcon,
  Logo,
  MoonIcon,
  PlugIcon,
  SlidersIcon,
  BookmarkIcon,
  SparkIcon,
  SunIcon,
  SwapIcon,
  TerminalIcon,
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
  branch,
  busy,
  skillCount,
  agentCount,
  noteCount,
  mcp,
  health,
  theme,
  onPickWorkspace,
  onNew,
  onOpen,
  onDelete,
  onTheme,
  onSkills,
  onAgents,
  onMemory,
  onUsage,
  onMcp,
  onTerminal,
  onSettings,
}: {
  /** Set by the handle beside it; the rail only has to wear the number. */
  width: number;
  workspace: string | null;
  sessions: SessionMeta[];
  currentId: string | undefined;
  changedCount: number;
  /**
   * The branch checked out right now, or null where there is no repository.
   *
   * Used to spot the conversations that were started somewhere else — see
   * [`subtitle`].
   */
  branch: string | null;
  busy: boolean;
  skillCount: number | null;
  agentCount: number | null;
  /** How many notes earlier conversations left here. Null before the first
   *  status has landed, which is when a count would be a guess. */
  noteCount: number | null;
  /**
   * How many MCP servers there are and how many answered, or null before the
   * first status lands.
   *
   * Two numbers rather than one: a bare count cannot tell four working servers
   * from four broken ones, and a server that is configured and not connected is
   * the whole thing this row exists to make visible.
   */
  mcp: { total: number; connected: number } | null;
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
  onMemory: () => void;
  /** Opens the context account. Reached from here as well as from the meter
   *  above the composer, because the meter hides itself while the window is
   *  less than half full — and "what does a request cost before I start" is a
   *  question worth asking exactly then. */
  onUsage: () => void;
  onMcp: () => void;
  /** Shows or hides the terminal dock. The dock says which it is; this row
   *  only has to be the way to reach it. */
  onTerminal: () => void;
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

  /** Which sections are folded. Outlives the window — see `useSections`. */
  const sections = useSections();

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
          data-tip={session.title || "No turns yet"}
          data-tip-side="right"
          onClick={() => onOpen(session.id)}
        >
          <b>{title}</b>
          <span className={armed ? "warn" : undefined}>
            {armed
              ? "delete this and its undo history?"
              : subtitle(session, current ? changedCount : null, branch)}
          </span>
        </button>

        {armed ? (
          <>
            <button
              className="rail-delete confirm"
              data-tip="Erase the transcript and the checkpoints that made its turns undoable"
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
              data-tip="Keep it"
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
            data-tip="Delete this conversation"
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
          // Switching folders closes the conversation and reconnects every MCP
          // server, neither of which a running turn survives.
          disabled={busy}
          onClick={onPickWorkspace}
          data-tip={
            busy
              ? "Stop the running turn before switching workspace"
              : (workspace ?? "Choose a workspace")
          }
          data-tip-side="bottom"
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
        <button
          className="primary rail-new"
          onClick={onNew}
          disabled={busy}
          data-tip={
            busy
              ? "Stop the running turn before starting another conversation"
              : "Start a new conversation in this workspace"
          }
          data-tip-side="bottom"
        >
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
          <Section
            name="today"
            label="Today"
            count={today.length}
            sections={sections}
            tip="Conversations you have touched today"
          >
            <div className="rail-list">{today.map(item)}</div>
          </Section>
        )}
        {earlier.length > 0 && (
          <Section
            name="earlier"
            label="Earlier"
            count={earlier.length}
            sections={sections}
            tip="Everything older than today"
          >
            <div className="rail-list">{earlier.map(item)}</div>
          </Section>
        )}
      </div>

      <div className="rail-foot">
        {/* The five panels, behind one fold.
            Settings, the theme and the provider stay outside it: the first two
            are how you get out of a state you did not mean to be in, and the
            third is a status line rather than a place to go. A fold that can
            hide the way to Settings is a fold that can strand somebody. */}
        <Section
          name="tools"
          label="Tools"
          sections={sections}
          tip="Skills, agents, memory, MCP and the terminal"
          /* What a collapsed section is still obliged to say. A server that is
             configured and not answering is the one thing in here that is
             wrong *now*, and folding the section away must not be a way to
             stop hearing about it. */
          warn={mcp !== null && mcp.connected < mcp.total ? mcpHint(mcp) : null}
        >
          <button className="rail-link accent" onClick={onSkills} data-tip={SKILLS_HINT}>
            <span className="glyph">
              <SparkIcon />
            </span>
            <b>Skills</b>
            {skillCount !== null && <span className="count">{skillCount}</span>}
          </button>
          <button className="rail-link" onClick={onAgents} data-tip={AGENTS_HINT}>
            <span className="glyph">
              <DelegateIcon />
            </span>
            <b>Agents</b>
            {agentCount !== null && <span className="count">{agentCount}</span>}
          </button>
          <button
            className="rail-link"
            onClick={onMemory}
            data-tip="What earlier conversations here left for this one"
          >
            <span className="glyph">
              <BookmarkIcon />
            </span>
            <b>Memory</b>
            {noteCount !== null && noteCount > 0 && (
              <span className="count">{noteCount}</span>
            )}
          </button>
          <button
            className="rail-link"
            onClick={onUsage}
            data-tip="What has filled the context window, and what every request costs before it starts"
          >
            <span className="glyph">
              <GaugeIcon />
            </span>
            <b>Context</b>
          </button>
          <button className="rail-link" onClick={onMcp} data-tip={mcpHint(mcp)}>
            <span className="glyph">
              <PlugIcon />
            </span>
            <b>MCP</b>
            {mcp !== null && mcp.total > 0 && (
              /* Marked when some server is not answering, so a failure is
                 visible from the rail rather than only from inside the panel.
                 A configured server that is not there is the one state this
                 feature keeps being reported for. */
              <span
                className={`count${mcp.connected < mcp.total ? " warn" : ""}`}
              >
                {mcp.connected < mcp.total
                  ? `${mcp.connected}/${mcp.total}`
                  : mcp.total}
              </span>
            )}
          </button>
          <button
            className="rail-link"
            onClick={onTerminal}
            data-tip="A shell in this folder (⌃`)"
          >
            <span className="glyph">
              <TerminalIcon />
            </span>
            <b>Terminal</b>
          </button>
        </Section>

        <button
          className="rail-link"
          onClick={onSettings}
          data-tip="Providers, models, permissions and everything else"
        >
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
          data-tip={THEME_HINT[theme]}
          onClick={() => onTheme(NEXT_THEME[theme])}
        >
          <span className="glyph">{themeIcon(theme)}</span>
          <b>{THEME_LABEL[theme]}</b>
        </button>
        <div className="rail-status" data-tip={healthTitle(health)}>
          <span className={`dot ${healthDot(health)}`} />
          <span>{healthLabel(health)}</span>
        </div>
      </div>
    </aside>
  );
}

/**
 * A foldable run of rows, with a label that is also the control.
 *
 * The whole header is the button rather than the caret beside it. A caret is
 * about eleven pixels wide and the row it sits in is two hundred; making the
 * eleven the target is the difference between a fold people use and one they
 * find by accident.
 *
 * Folded, the children are *not rendered* rather than hidden with CSS. A
 * `display: none` subtree still holds focusable buttons in some engines and
 * still answers a screen reader's list of controls, so a folded section would
 * quietly become a place the Tab key goes and nothing appears to happen.
 */
function Section({
  name,
  label,
  count,
  tip,
  warn,
  sections,
  children,
}: {
  /** The key this section is remembered under. */
  name: string;
  label: string;
  /** Shown folded, so the fold says how much is behind it. */
  count?: number;
  tip: string;
  /**
   * Something wrong inside, which the header has to carry while it is shut.
   *
   * The string is the reason, and it becomes the header's tip: a dot that says
   * "something is wrong" and cannot say what is a dot people learn to ignore.
   */
  warn?: string | null;
  sections: { collapsed: (name: string) => boolean; toggle: (name: string) => void };
  children: ReactNode;
}) {
  const shut = sections.collapsed(name);
  return (
    <>
      <button
        className={`rail-group micro${shut ? " shut" : ""}`}
        aria-expanded={!shut}
        data-tip={warn ?? tip}
        onClick={() => sections.toggle(name)}
      >
        <span className="rail-caret">{shut ? "▸" : "▾"}</span>
        <b>{label}</b>
        {/* Only while it is shut. Open, the rows are right there to be counted,
            and a number beside them is one more thing on a dense surface. */}
        {shut && count !== undefined && <span className="count">{count}</span>}
        {shut && warn && <span className="dot warn" />}
      </button>
      {!shut && children}
    </>
  );
}

/**
 * What the rail says under a conversation's title.
 *
 * For the open one that is how much of the workspace it has rewritten, which
 * is the fact you want before switching away from it. For the rest the model
 * and the time are all that has been read off disk.
 *
 * A branch is named only when it is not the one checked out now. Every file
 * path in that conversation, and every pre-image behind its rewind, describes
 * a tree that is no longer there — so the row that would otherwise look like
 * any other says where it came from. Printing the branch on every row instead
 * would make the common case noisier to make the rare case visible, which is
 * the wrong trade in a list this dense.
 */
function subtitle(
  session: SessionMeta,
  changed: number | null,
  branch: string | null,
): string {
  const ago = when(session.updated);
  // Only when both are known. A session with no recorded branch predates the
  // field or was started outside a repository, and neither is "elsewhere".
  const elsewhere =
    session.branch && branch && session.branch !== branch ? `on ${session.branch} · ` : "";
  if (changed === null) return `${elsewhere}${session.model} · ${ago}`;
  return changed === 0
    ? `${elsewhere}read-only · ${ago}`
    : `${elsewhere}${plural(changed, "file")} changed · ${ago}`;
}

/** What the MCP row says on hover, which is where the reason for a badge goes. */
function mcpHint(mcp: { total: number; connected: number } | null): string {
  if (mcp === null || mcp.total === 0)
    return "External tool servers. None configured yet.";
  if (mcp.connected === mcp.total)
    return `${plural(mcp.total, "MCP server")} connected`;
  return `${mcp.total - mcp.connected} of ${plural(
    mcp.total,
    "MCP server",
  )} not connected`;
}

const SKILLS_HINT = "Reusable instructions Taurus can load into a turn";
const AGENTS_HINT = "Scoped helpers a turn can delegate work to";

const NEXT_THEME: Record<Theme, Theme> = {
  system: "light",
  light: "dark",
  dark: "system",
};

/** Exported so a test can find the theme control the way a reader does:
 *  by the word on it, not by its position in a row that grows. */
export const THEME_LABEL: Record<Theme, string> = {
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
