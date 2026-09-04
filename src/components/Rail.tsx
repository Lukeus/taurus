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
  WaterfallIcon,
  BookmarkIcon,
  SparkIcon,
  SunIcon,
  SwapIcon,
  TerminalIcon,
  TrashIcon,
} from "./icons";

/*
 * The class lists more than one element wears.
 *
 * Named for the same reason the stylesheet named `.rail-link` instead of
 * repeating nine declarations eight times: eight copies of one string is eight
 * places for it to drift, and the eighth is the one nobody notices. Only the
 * genuinely shared ones are here — a list used once reads better at the element
 * that wears it than as a name two hundred lines away.
 */

/** One of the seven panels in the footer, and the glyph that leads it. */
const LINK =
  "rail-link flex items-center gap-2 py-2 px-3 rounded-sm border-0 bg-transparent text-13 text-dim text-left hover:not-disabled:bg-hover";
const GLYPH = "glyph w-3.5 flex-none inline-flex items-center";

/**
 * The trash can, and the two controls it becomes once it is armed.
 *
 * The 28px floor is the point of the shared half — see the comment at the row —
 * and `styles.test.ts` reads it back out of whichever of these an element
 * wears, so it cannot be dropped from one and kept in the others.
 */
const DELETE =
  "rail-delete flex-none inline-flex items-center justify-center border-0 rounded-sm bg-transparent min-w-7 min-h-7 hover:not-disabled:text-danger hover:not-disabled:bg-danger/12";
const DELETE_ICON = `${DELETE} p-1.5 text-faint`;
const DELETE_CONFIRM = `${DELETE} py-1 px-2 font-mono text-11 text-danger`;

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
  jobsRunning,
  health,
  theme,
  brand,
  onPickWorkspace,
  onNew,
  onOpen,
  onDelete,
  onTheme,
  onSkills,
  onAgents,
  onMemory,
  onUsage,
  onTraces,
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
  /**
   * How many background commands are running right now.
   *
   * On this row because it is the way into the dock, and because the dock is
   * where they are: a build the model started while the pane was shut is one
   * nothing else on screen would mention. Zero draws nothing at all.
   */
  jobsRunning: number;
  health: ProviderHealth;
  /** The preference, not the resolved palette — the row names what was chosen. */
  theme: Theme;
  /**
   * The mark and the word in the top-left corner, when a custom theme has
   * replaced them.
   *
   * Null is the app's own, which is the overwhelmingly common case and is why
   * this is a prop rather than something the rail reads for itself. An empty
   * `wordmark` is a real answer and means a mark on its own — the two are
   * distinguished because "no opinion" and "deliberately nothing" produce
   * different corners.
   */
  brand: { name: string; wordmark: string | null; logo: string | null } | null;
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
  /** Opens the trace panel. Beside the context account because the two are the
   *  same question asked about different currencies — one about tokens, one
   *  about seconds — and somebody who has just found a turn expensive is a
   *  step away from wondering why it was also slow. */
  onTraces: () => void;
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
      /*
       * A conversation and the one destructive thing that can be done to it.
       *
       * The row carries the padding, the hover and the selection because what
       * is in it is two controls, not one: a button cannot contain another
       * button, and a trash can that only appeared on hover could not be
       * reached from the keyboard at all.
       *
       * Armed, it holds its own ground hover or not — under the pointer it
       * would otherwise look exactly like the four rows around it, one of
       * which is about to be deleted and three of which are not.
       */
      <div
        key={session.id}
        className="rail-row group flex items-center gap-1 min-w-0 py-2 pr-1.5 pl-2.5 rounded-sm border-l-2 border-l-transparent hover:bg-hover data-current:bg-active data-current:border-l-accent data-armed:bg-danger/10 data-armed:border-l-danger data-armed:hover:bg-danger/10"
        data-current={current || undefined}
        data-armed={armed || undefined}
      >
        {/* A conversation reads as text; only the hover tells you it is a
            control, which is why the row lights up and the title inside it
            must not light up again. Disabled while it is the open one, and
            that is not the same as unavailable — so it keeps its full
            opacity where a genuinely unavailable one fades. */}
        <button
          className="rail-item flex-1 min-w-0 flex flex-col gap-px p-0 border-0 rounded-none bg-transparent text-left hover:not-disabled:bg-transparent group-data-current:disabled:opacity-100"
          // Switching mid-turn would leave the running turn streaming into a
          // transcript nobody is looking at.
          disabled={busy || current}
          data-tip={session.title || "No turns yet"}
          data-tip-side="right"
          onClick={() => onOpen(session.id)}
        >
          <b className="text-13 font-normal text-dim truncate group-data-current:text-ink">
            {title}
          </b>
          {/* Armed, the row spends its subtitle line saying what is about to
              be lost; there is no other room at this width to say it. */}
          <span className="font-mono text-10 text-faint truncate group-data-armed:text-danger">
            {armed
              ? "delete this and its undo history?"
              : subtitle(session, current ? changedCount : null, branch)}
          </span>
        </button>

        {armed ? (
          <>
            <button
              className={DELETE_CONFIRM}
              data-confirm
              data-tip="Erase the transcript and the checkpoints that made its turns undoable"
              onClick={() => {
                setArming(null);
                onDelete(session.id);
              }}
            >
              Delete
            </button>
            <button
              className={DELETE_ICON}
              aria-label="Keep this conversation"
              data-tip="Keep it"
              onClick={() => setArming(null)}
            >
              ✕
            </button>
          </>
        ) : (
          /*
              The one control in the rail that cannot be taken back, and it
              measured 23×23 — the smallest target in the app, sitting against
              a row whose remaining 200-odd pixels merely select. Missing it is
              safe; missing the row is not, and the two were four pixels apart.
              The glyph stays 14px: what grows is the area around it, which is
              the part a pointer is aiming at, and `styles.test.ts` reads the
              floor back out of this class list.
          */
          <button
            className={DELETE_ICON}
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
    <aside
      className="rail flex-none flex flex-col min-h-0 bg-rail"
      style={{ width }}
    >
      {/* Clears the macOS traffic lights, which float over this corner, and
          carries the wordmark at the one height that lines its rule up with
          the topbar's across the fold. */}
      <div className="rail-brand h-11 flex-none flex items-center gap-2 px-gutter-rail border-b border-rule">
        {brand?.logo ? (
          /*
             A theme's own mark, inlined as a data URI by the loader.

             Capped rather than trusted: the file is somebody's SVG and its
             intrinsic size is whatever their editor wrote. Unbounded, a 200px
             logo takes the 44px brand row with it — and that row is what
             clears the macOS traffic lights, so the failure is a window whose
             close button is under a picture. `object-contain` keeps the aspect
             ratio inside the box rather than squashing a wordmark that is
             wider than it is tall.
          */
          <img
            className="rail-logo w-auto max-w-24 h-4 object-contain object-left"
            src={brand.logo}
            alt=""
            aria-hidden="true"
          />
        ) : (
          <Logo />
        )}
        {/* Null is "no opinion" and falls back to the app's name; empty string
            is a decision, and means a mark standing on its own. */}
        {brand?.wordmark !== "" && (
          <span className="font-mono text-11-5 tracking-[0.06em] text-dim">
            {brand?.wordmark ?? "taurus"}
          </span>
        )}
      </div>

      <div className="rail-pad px-2 pt-2.5 pb-2.5">
        <button
          className="rail-workspace w-full flex items-center gap-2 py-2 px-3 rounded-md bg-hover text-left hover:not-disabled:bg-active"
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
          <span className="rail-workspace-name flex-1 min-w-0">
            <b className="block text-12-5 font-normal text-ink truncate">
              {workspace ? basename(workspace) : "No workspace"}
            </b>
            <span className="block font-mono text-10 text-faint truncate">
              {workspace ? parentDir(workspace) : "choose a folder"}
            </span>
          </span>
          <span className="rail-workspace-swap flex-none inline-flex text-faint">
            <SwapIcon size={12} />
          </span>
        </button>
      </div>

      <div className="rail-pad px-2 pb-2.5">
        <button
          className="primary rail-new w-full py-2 px-2.5 text-13"
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

      {/*
          The conversations, which get the room that is left and a floor under
          it.

          The floor is the point. The footer below is eight rows and three
          headers, and at the window's 520px minimum it wants more height than
          the rail has — so without `min-h-24` the list it is supposed to sit
          under is squeezed to nothing and the footer overflows the rail
          instead. Conversations are the app's spine; they are the one thing
          here that must still be on screen when everything is competing for
          the same pixels, so the footer is what gives way.
      */}
      <div className="rail-scroll flex-auto min-h-24 overflow-y-auto">
        {sessions.length === 0 && (
          <p className="rail-empty pt-1 px-gutter-rail text-12-5 text-faint">
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
            <div className="rail-list px-2 flex flex-col gap-px">{today.map(item)}</div>
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
            <div className="rail-list px-2 flex flex-col gap-px">{earlier.map(item)}</div>
          </Section>
        )}
      </div>

      {/*
          The panels, the way out, and the provider.

          `flex-initial` rather than a fixed height: it shrinks and scrolls
          instead of holding its full size, which is what keeps it from pushing
          the conversation list off the bottom of a short window. It takes what
          it needs whenever there is room — every window that is not close to
          the 520px minimum — so the scrolling is a floor under the worst case
          rather than something anybody meets in normal use.
      */}
      <div className="rail-foot flex-initial overflow-y-auto border-t border-rule p-2 flex flex-col gap-px">
        {/*
            The seven panels, in three folds rather than one.

            They were behind a single fold called "Tools", and the label had
            stopped predicting what was inside it: skills, agents and memory
            are what the model can do here and what it already knows; MCP and
            the terminal are the things outside this window that it talks to;
            context and traces are the same question asked about a turn that
            has already happened — one in tokens, one in seconds. Three names
            that each describe their contents is what makes a shut fold safe
            to leave shut, which is the whole point of a fold in a rail this
            dense.

            Settings, the theme and the provider stay outside all three: the
            first two are how you get out of a state you did not mean to be
            in, and the third is a status line rather than a place to go. A
            fold that can hide the way to Settings is a fold that can strand
            somebody.
        */}
        <Section
          name="agent"
          label="Agent"
          sections={sections}
          pad="pt-1"
          tip="What the model can do in this workspace, and what it already knows"
        >
          <button className={LINK} onClick={onSkills} data-tip={SKILLS_HINT}>
            <span className={`${GLYPH} text-accent`}>
              <SparkIcon />
            </span>
            <b className="flex-1 font-normal">Skills</b>
            {skillCount !== null && <span className="count font-mono text-10 text-faint">{skillCount}</span>}
          </button>
          <button className={LINK} onClick={onAgents} data-tip={AGENTS_HINT}>
            <span className={`${GLYPH} text-faint`}>
              <DelegateIcon />
            </span>
            <b className="flex-1 font-normal">Agents</b>
            {agentCount !== null && <span className="count font-mono text-10 text-faint">{agentCount}</span>}
          </button>
          <button
            className={LINK}
            onClick={onMemory}
            data-tip="What earlier conversations here left for this one"
          >
            <span className={`${GLYPH} text-faint`}>
              <BookmarkIcon />
            </span>
            <b className="flex-1 font-normal">Memory</b>
            {noteCount !== null && noteCount > 0 && (
              <span className="count font-mono text-10 text-faint">{noteCount}</span>
            )}
          </button>
        </Section>

        <Section
          name="connections"
          label="Connections"
          sections={sections}
          pad="pt-2.5"
          tip={connectionsHint(mcp, jobsRunning)}
          /* What a collapsed section is still obliged to say. A server that is
             configured and not answering is the one thing in here that is
             wrong *now*, and folding the section away must not be a way to
             stop hearing about it. */
          warn={mcp !== null && mcp.connected < mcp.total ? mcpHint(mcp) : null}
          /* And a command still running is the one thing in here that is
             happening now. Folding a section is a way to stop looking at it,
             not a way to stop being told that something in it is still going
             — a build the model started while this was shut is exactly the
             job nothing else on screen would mention. */
          live={jobsRunning}
        >
          <button className={LINK} onClick={onMcp} data-tip={mcpHint(mcp)}>
            <span className={`${GLYPH} text-faint`}>
              <PlugIcon />
            </span>
            <b className="flex-1 font-normal">MCP</b>
            {mcp !== null && mcp.total > 0 && (
              /* Marked when some server is not answering, so a failure is
                 visible from the rail rather than only from inside the panel.
                 A configured server that is not there is the one state this
                 feature keeps being reported for. */
              <span
                className="count font-mono text-10 text-faint data-warn:text-warn"
                data-warn={mcp.connected < mcp.total || undefined}
              >
                {mcp.connected < mcp.total
                  ? `${mcp.connected}/${mcp.total}`
                  : mcp.total}
              </span>
            )}
          </button>
          <button
            className={LINK}
            onClick={onTerminal}
            data-tip={
              jobsRunning > 0
                ? `${plural(jobsRunning, "command")} running in the background`
                : "A shell in this folder (⌃`)"
            }
          >
            <span className={`${GLYPH} text-faint`}>
              <TerminalIcon />
            </span>
            <b className="flex-1 font-normal">Terminal</b>
            {jobsRunning > 0 && <span className="count font-mono text-10 text-accent" data-live>
              {jobsRunning}
            </span>}
          </button>
        </Section>

        <Section
          name="activity"
          label="Activity"
          sections={sections}
          pad="pt-2.5"
          tip="What the turns so far have cost — tokens on one side, seconds on the other"
        >
          <button
            className={LINK}
            onClick={onUsage}
            data-tip="What has filled the context window, and what every request costs before it starts"
          >
            <span className={`${GLYPH} text-faint`}>
              <GaugeIcon />
            </span>
            <b className="flex-1 font-normal">Context</b>
          </button>
          <button
            className={LINK}
            onClick={onTraces}
            data-tip="Where each turn's time went — model calls, tools, and everything in between"
          >
            <span className={`${GLYPH} text-faint`}>
              <WaterfallIcon />
            </span>
            <b className="flex-1 font-normal">Traces</b>
          </button>
        </Section>

        <button
          className={LINK}
          onClick={onSettings}
          data-tip="Providers, models, permissions and everything else"
        >
          <span className={`${GLYPH} text-faint`}>
            <SlidersIcon />
          </span>
          <b className="flex-1 font-normal">Settings</b>
        </button>
        {/* Three preferences on one row, so it cycles rather than toggles.
            A light/dark switch here would quietly throw away "follow the
            system", which is both the default and the only one of the three
            that can change on its own. */}
        <button
          className={LINK}
          data-tip={THEME_HINT[theme]}
          onClick={() => onTheme(NEXT_THEME[theme])}
        >
          <span className={`${GLYPH} text-faint`}>{themeIcon(theme)}</span>
          <b className="flex-1 font-normal">{THEME_LABEL[theme]}</b>
        </button>
        <div className="rail-status flex items-center gap-2 py-2 px-3" data-tip={healthTitle(health)}>
          <span className={`dot ml-1 ${healthDot(health)}`} />
          <span className="font-mono text-10-5 text-faint truncate">{healthLabel(health)}</span>
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
  live,
  sections,
  pad = "pt-4",
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
  /**
   * How many things inside are happening right now.
   *
   * A separate channel from `warn` because it is a separate kind of fact: a
   * warning is something that is wrong and will stay wrong until somebody
   * acts, while this is something that is *working* and will stop on its own.
   * Painting both as the same dot would teach that the dot means "look here",
   * which is exactly the reading that makes a real warning easy to miss.
   */
  live?: number;
  sections: { collapsed: (name: string) => boolean; toggle: (name: string) => void };
  /**
   * How much air the header takes above it.
   *
   * A header in the scrolling list can afford 16px, because only one of them is
   * ever on screen at a time. Stacked three deep in a footer that also carries
   * Settings, the theme and the provider, that same 16px is most of a
   * conversation row's worth of nothing between each pair — so the footer's
   * headers take the next step down the ladder, and its first takes less still
   * because the rule along the top is already doing the separating.
   */
  pad?: string;
  children: ReactNode;
}) {
  const shut = sections.collapsed(name);
  const counted = count !== undefined || (live !== undefined && live > 0);
  return (
    <>
      {/*
          Still a micro-label to read — mono, tracked, faint — because the
          sections are furniture and the conversations inside them are the
          content. What changed when it became a button is only that it answers
          a pointer: no border, no fill, and the same left edge every other line
          in the rail sits on, so a column of headers does not read as a column
          of controls.

          The caret hangs in the gutter rather than in the text column. `pl-2`
          is exactly where the conversation rows below start their hover
          rectangles and where a selected row draws its 2px bar; that plus the
          caret's 8px box and the 4px gap puts the *label* back on
          --gutter-rail with every other line in the rail. Left of that line is
          where the rail keeps the marks that are about a row rather than part
          of it, and a disclosure triangle is one of those.

          Shut, the header is the only thing standing for what is inside it, so
          it stops being furniture and steps up to the weight of a row — and
          the caret and the count come with it, which is why neither names a
          colour of its own.
      */}
      <button
        className={`rail-group micro group flex items-center gap-1 w-full pr-gutter-rail pb-1.5 pl-2 border-0 rounded-none bg-transparent text-left hover:not-disabled:bg-transparent hover:not-disabled:text-dim data-shut:text-dim ${pad}`}
        data-shut={shut || undefined}
        aria-expanded={!shut}
        data-tip={warn ?? tip}
        onClick={() => sections.toggle(name)}
      >
        {/* Fixed width, so the label does not shift sideways as the caret
            changes glyph. */}
        <span className="rail-caret w-2 flex-none text-[8.5px] leading-none text-faint group-data-shut:text-dim">
          {shut ? "▸" : "▾"}
        </span>
        <b className="font-normal">{label}</b>
        {/* Only while it is shut. Open, the rows are right there to be counted,
            and a number beside them is one more thing on a dense surface. */}
        {shut && count !== undefined && (
          <span className="count ml-auto font-mono text-10 tracking-normal">{count}</span>
        )}
        {shut && live !== undefined && live > 0 && (
          <span className="count ml-auto font-mono text-10 tracking-normal text-accent" data-live>
            {live}
          </span>
        )}
        {/* A count and a warning together would put two things in one slot, so
            they sit as a pair and only the first one takes the margin. */}
        {shut && warn && (
          <span className={`dot warn ${counted ? "ml-1.5" : "ml-auto"}`} />
        )}
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

/**
 * What the Connections fold says on hover.
 *
 * Built rather than fixed, because the two rows behind it are the two in the
 * rail with live state: a server that is not answering and a command that is
 * still running. Shut, this string is the only thing standing for both.
 */
function connectionsHint(
  mcp: { total: number; connected: number } | null,
  jobsRunning: number,
): string {
  const running =
    jobsRunning > 0 ? `${plural(jobsRunning, "command")} running` : null;
  const servers =
    mcp === null || mcp.total === 0 ? null : mcpHint(mcp).replace(/^\w/, (c) => c.toLowerCase());
  const parts = [servers, running].filter((p): p is string => p !== null);
  return parts.length === 0
    ? "External tool servers and a shell in this folder"
    : `Tool servers and the terminal — ${parts.join(", ")}`;
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
