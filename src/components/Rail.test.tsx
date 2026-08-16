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
      health={{ state: "connected", id: "ollama", models: 4 }}
      theme="dark"
      onPickWorkspace={() => {}}
      onNew={() => {}}
      onOpen={() => {}}
      onDelete={() => {}}
      onTheme={() => {}}
      onSkills={() => {}}
      onAgents={() => {}}
      onSettings={() => {}}
      {...props}
    />,
  );

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
    expect(html).not.toContain("on main");
  });

  it("says nothing about branches for a workspace that has none", () => {
    // No repository, or a transcript written before branches were recorded.
    // Neither is "elsewhere", and guessing would label every old conversation.
    const html = draw({
      sessions: [session("a", "Fix the parser", now)],
      branch: null,
    });
    expect(html).not.toContain(" on ");
  });
});

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
    expect(html).toContain("dot error");
  });

  it("points at the missing configuration when there is no provider at all", () => {
    expect(label({ state: "none" })).toContain("no provider configured");
  });
});

describe("the foot links", () => {
  it("carries both counts, so neither library is behind an unlabelled door", () => {
    const html = draw({ skillCount: 12, agentCount: 3 });
    expect(html).toContain("Skills");
    expect(html).toContain(">12<");
    expect(html).toContain("Agents");
    expect(html).toContain(">3<");
  });

  it("omits a count it does not have yet rather than showing a zero", () => {
    // Before the first status arrives there is no roster to report, and "0
    // agents" would be wrong: two ship with the harness.
    const html = draw({ skillCount: null, agentCount: null });
    expect(html).toContain("Agents");
    expect(html).not.toContain("count");
  });
});
