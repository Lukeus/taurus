import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { Rail, type ProviderHealth } from "./Rail";
import type { SessionMeta } from "../lib/api";

const now = Math.floor(Date.now() / 1000);

const session = (id: string, title: string, updated: number): SessionMeta => ({
  id,
  workspace: "/Users/x/code/taurus",
  model: "qwen3.6:27b",
  started: updated,
  updated,
  title,
});

const draw = (props: Partial<Parameters<typeof Rail>[0]> = {}) =>
  renderToStaticMarkup(
    <Rail
      width={236}
      workspace="/Users/x/code/taurus-ai-shell"
      sessions={[]}
      currentId={undefined}
      changedCount={0}
      branch={null}
      busy={false}
      skillCount={12}
      agentCount={3}
      noteCount={2}
      mcp={{ total: 2, connected: 2 }}
      jobsRunning={0}
      health={{ state: "connected", id: "ollama", models: 4 }}
      theme="dark"
      brand={null}
      onPickWorkspace={() => {}}
      onNew={() => {}}
      onOpen={() => {}}
      onDelete={() => {}}
      onTheme={() => {}}
      onSkills={() => {}}
      onAgents={() => {}}
      onMemory={() => {}}
      onUsage={() => {}}
      onTraces={() => {}}
      onMcp={() => {}}
        onTerminal={() => {}}
      onSettings={() => {}}
      {...props}
    />,
  );

describe("the panels, grouped", () => {
  /** The fold headers, in the order the rail stacks them. */
  const groups = (html: string) =>
    [...html.matchAll(/class="rail-group[^"]*"[^>]*>.*?<b[^>]*>([^<]*)<\/b>/g)].map(
      ([, label]) => label,
    );

  it("names each fold after what is behind it", () => {
    // The bug this replaces: seven unlike panels behind one fold called
    // "Tools". A label that does not predict its contents is a label nobody
    // can decide to leave shut, so the fold stops being worth having.
    expect(groups(draw({ sessions: [] }))).toEqual([
      "Agent",
      "Connections",
      "Activity",
    ]);
  });

  it("puts every panel in exactly one of them", () => {
    // Read off the rendered rail rather than restated here, so a panel added
    // later and dropped outside the three is a failure rather than a row that
    // quietly lands beside Settings.
    const html = draw({ sessions: [] });
    const links = [...html.matchAll(/class="rail-link[^"]*"[^>]*>.*?<b[^>]*>([^<]*)<\/b>/g)].map(
      ([, label]) => label,
    );
    expect(links).toEqual([
      "Skills",
      "Agents",
      "Memory",
      "MCP",
      "Terminal",
      "Context",
      "Traces",
      // Outside all three on purpose — a fold that can hide the way out of a
      // state you did not mean to be in is a fold that can strand somebody.
      "Settings",
      "Dark theme",
    ]);
  });
});

describe("branch awareness", () => {
  it("marks a conversation started on a branch that is no longer checked out", () => {
    // Every file path in it, and every pre-image behind its rewind, describes
    // a tree that is not there any more. The row must not look like the rest.
    const html = draw({
      sessions: [{ ...session("a", "Fix the parser", now), branch: "feat/parser" }],
      branch: "main",
    });
    expect(html).toContain("on feat/parser");
  });

  it("stays quiet when the conversation is on the branch you are on", () => {
    // The common case. Printing the branch on every row would make it noisier
    // to make the rare case visible, which is the wrong trade in this list.
    const html = draw({
      sessions: [{ ...session("a", "Fix the parser", now), branch: "main" }],
      branch: "main",
    });
    expect(subtitleOf(html, "Fix the parser")).not.toMatch(/^on /);
  });

  it("says nothing about branches for a workspace that has none", () => {
    // No repository, or a transcript written before branches were recorded.
    // Neither is "elsewhere", and guessing would label every old conversation.
    const html = draw({
      sessions: [session("a", "Fix the parser", now)],
      branch: null,
    });
    expect(subtitleOf(html, "Fix the parser")).not.toMatch(/^on /);
  });
});

/**
 * The line the rail draws under one conversation's title.
 *
 * These two used to assert over the whole rendered rail — that "on main" and
 * then that " on " appeared nowhere in it. Both were standing in for one
 * narrow claim: that the *subtitle* does not open with a branch prefix. The
 * wider form fails on any tooltip elsewhere in the rail that happens to use
 * the word in a sentence, which is a test that breaks on prose rather than on
 * the behaviour it was written for.
 */
function subtitleOf(html: string, title: string): string {
  const row = html.match(new RegExp(`<b[^>]*>${title}</b><span[^>]*>([^<]*)</span>`));
  expect(row, `the rail drew no row titled "${title}"`).not.toBeNull();
  return row![1];
}

describe("the workspace button", () => {
  it("leads with the folder name and keeps the path underneath", () => {
    const html = draw();
    expect(html).toContain("taurus-ai-shell");
    // The full path is too long for 236px, so the parent is abbreviated.
    expect(html).toContain("~/code");
  });

  it("says so when there is no workspace rather than rendering an empty row", () => {
    expect(draw({ workspace: null })).toContain("No workspace");
  });
});

describe("the rail's width", () => {
  it("is whatever the handle beside it has been dragged to", () => {
    // The stylesheet gives the rail no width of its own, so a rail that
    // ignored this prop would collapse to the width of its longest title.
    expect(draw({ width: 312 })).toContain("width:312px");
  });
});

describe("grouping conversations", () => {
  it("separates today from everything before it", () => {
    const html = draw({
      sessions: [
        session("a", "Rename parseContextLength", now - 60),
        session("b", "Add the context-length field", now - 3 * 86_400),
      ],
    });
    expect(html).toContain("Today");
    expect(html).toContain("Earlier");
  });

  it("omits a heading with nothing under it", () => {
    const html = draw({ sessions: [session("a", "Only one", now - 60)] });
    expect(html).toContain("Today");
    expect(html).not.toContain("Earlier");
  });

  it("names an untitled conversation instead of leaving the row blank", () => {
    // A session exists on disk from the moment it is created, before the
    // first turn has given it a title.
    expect(draw({ sessions: [session("a", "", now)] })).toContain(
      "New conversation",
    );
  });
});

describe("what a conversation row says about itself", () => {
  it("reports the open conversation by what it changed", () => {
    const html = draw({
      sessions: [session("a", "Rename it", now - 60)],
      currentId: "a",
      changedCount: 2,
    });
    expect(html).toContain("2 files changed");
  });

  it("calls the open conversation read-only when it changed nothing", () => {
    const html = draw({
      sessions: [session("a", "Summarize the crates", now - 60)],
      currentId: "a",
      changedCount: 0,
    });
    expect(html).toContain("read-only");
  });

  it("falls back to the model for conversations that are not open", () => {
    // The changed-file count is only known for the session in memory; the
    // others would need a checkpoint read each, so they show what was listed.
    const html = draw({
      sessions: [session("a", "Older", now - 3 * 86_400)],
      changedCount: 2,
    });
    expect(html).toContain("qwen3.6:27b");
    expect(html).not.toContain("2 files changed");
  });

  it("cannot be clicked into while a turn is running", () => {
    const html = draw({ sessions: [session("a", "Older", now)], busy: true });
    expect(html).toContain("disabled");
  });
});

describe("deleting a conversation", () => {
  it("offers it on every row, by name, so the control is not hover-only", () => {
    // A trash can that appears on :hover is unreachable by keyboard and
    // invisible to a screen reader; the label is what makes four identical
    // buttons tellable apart once it is reached.
    const html = draw({ sessions: [session("a", "Rename it", now - 60)] });
    expect(html).toContain('aria-label="Delete Rename it"');
  });

});

describe("the theme row", () => {
  it("names the preference rather than the palette on screen", () => {
    // "Match system" and "Dark theme" are different rows even when both are
    // painting dark, and the one that is actually set is the one to show.
    expect(draw({ theme: "system" })).toContain("Match system");
    expect(draw({ theme: "dark" })).toContain("Dark theme");
    expect(draw({ theme: "light" })).toContain("Light theme");
  });
});

describe("provider health", () => {
  const label = (health: ProviderHealth) => draw({ health });

  it("names the provider and how many models it offered", () => {
    expect(label({ state: "connected", id: "ollama", models: 4 })).toContain(
      "ollama · 4 models",
    );
  });

  it("says a provider is unreachable rather than showing it as idle", () => {
    const html = label({ state: "unreachable", id: "apim" });
    expect(html).toContain("apim · unreachable");
    expect(html).toMatch(/class="dot[^"]*\berror\b/);
  });

  it("points at the missing configuration when there is no provider at all", () => {
    expect(label({ state: "none" })).toContain("no provider configured");
  });
});

describe("the foot links", () => {
  it("carries every count, so no library is behind an unlabelled door", () => {
    const html = draw({
      skillCount: 12,
      agentCount: 3,
      noteCount: 2,
      mcp: { total: 4, connected: 4 },
    });
    expect(html).toContain("Skills");
    expect(html).toContain(">12<");
    expect(html).toContain("Agents");
    expect(html).toContain(">3<");
    expect(html).toContain("MCP");
    expect(html).toContain(">4<");
  });

  it("marks the MCP count when a configured server is not answering", () => {
    // The state this whole row exists for. A bare "4" cannot tell four working
    // servers from four broken ones, and a server that is configured and not
    // there is the failure people actually hit.
    const html = draw({ mcp: { total: 4, connected: 3 } });
    expect(html).toContain("data-warn");
    expect(html).toContain("3/4");
    // The fraction carries it without colour, for a screenshot and for a reader
    // who cannot tell amber from grey.
    expect(html).toContain("not connected");
  });

  it("shows no MCP badge at all when nothing is configured", () => {
    // A "0" beside MCP reads as a failure. Nothing configured is not a problem.
    const html = draw({ mcp: { total: 0, connected: 0 } });
    expect(html).toContain("MCP");
    expect(html).toContain("None configured yet");
  });

  it("omits a count it does not have yet rather than showing a zero", () => {
    // Before the first status arrives there is no roster to report, and "0
    // agents" would be wrong: two ship with the harness.
    const html = draw({
      skillCount: null,
      agentCount: null,
      noteCount: null,
      mcp: null,
    });
    expect(html).toContain("Agents");
    expect(html).not.toContain("count");
  });

  it("offers memory, and says nothing about it when there is none", () => {
    // A zero here means something different from the nulls above: the status
    // did arrive and the answer is that nothing has been written down. That is
    // the ordinary state of a fresh workspace and it earns no badge — a rail
    // full of zeroes reads as a rail full of things needing attention.
    const empty = draw({ noteCount: 0 });
    expect(empty).toContain("Memory");
    expect(empty).not.toContain(">0<");

    expect(draw({ noteCount: 4 })).toContain(">4<");
  });
});
