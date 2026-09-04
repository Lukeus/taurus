import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import { ChangesDrawer, Outcome, commitCaveats } from "./ChangesDrawer";
import type { Checkpoint } from "../lib/api";

describe("restore outcomes", () => {
  it("names the file it put back", () => {
    const html = renderToStaticMarkup(
      <Outcome outcome={{ action: "reverted", path: "src/main.rs" }} />,
    );
    expect(html).toContain("src/main.rs");
    expect(html).toContain("reverted");
  });

  it("distinguishes a file that was deleted rather than reverted", () => {
    // A turn that created a file is undone by removing it, and that reads very
    // differently to the user than "reverted".
    const html = renderToStaticMarkup(
      <Outcome outcome={{ action: "deleted", path: "src/new.rs" }} />,
    );
    expect(html).toContain("deleted");
    expect(html).not.toContain("reverted");
  });

  it("gives the reason a file could not be restored", () => {
    // The one outcome the user must not skim past: the rewind reported
    // success overall but this file is still as the model left it.
    const html = renderToStaticMarkup(
      <Outcome
        outcome={{
          action: "skipped",
          path: "assets/logo.png",
          reason: "was not text when it was recorded",
        }}
      />,
    );
    expect(html).toContain("assets/logo.png");
    expect(html).toContain("was not text when it was recorded");
    expect(html).toContain("warn");
  });
});

describe("rendering", () => {
  it("survives a first paint before the checkpoint list has loaded", () => {
    const html = renderToStaticMarkup(
      <ChangesDrawer sessionId="s1" busy={false} onClose={() => {}} />,
    );
    expect(html).toContain("Changes");
  });

  it("brings no chrome of its own to the slot it is docked in", () => {
    /*
     * It used to be a modal, and carried the three things a modal needs: a
     * scrim, its own drag handle, and an inline width. All three now belong to
     * `App`, which owns the column this and the canvas take turns in — and a
     * panel that still drew its own would be a second opinion about how wide
     * it is, laid over the first.
     */
    const html = renderToStaticMarkup(
      <ChangesDrawer sessionId="s1" busy={false} onClose={() => {}} />,
    );
    expect(html).not.toContain("scrim");
    expect(html).not.toContain('role="separator"');
    expect(html).not.toContain("width:");
    // Docked, and saying so. The name carries no rule — the width comes from
    // `.drawer` — so this is an identity mark rather than a style, which is
    // what `styles.test.ts` has it listed as.
    expect(html).toContain("changes-pane");
  });

  it("offers the whole conversation as one diff, folded", () => {
    // Folded because opening it reads every file the conversation touched.
    // The turn list above answers "what changed"; this answers the question
    // asked before a commit, which no single turn's diff can.
    const html = renderToStaticMarkup(
      <ChangesDrawer sessionId="s1" busy={false} onClose={() => {}} />,
    );
    // Not before the listing has arrived: with no turns there is nothing for
    // it to be the whole of.
    expect(html).not.toContain("the whole diff");
  });

  it("shows no empty-state message until the list has actually arrived", () => {
    // `null` means "not loaded"; only an empty array means "nothing here".
    // Conflating them flashes "no changes" on every open.
    const html = renderToStaticMarkup(
      <ChangesDrawer sessionId="s1" busy={false} onClose={() => {}} />,
    );
    expect(html).not.toContain("has not changed any files");
  });
});

describe("committing one turn out of several", () => {
  /** A turn as the listing hands it over. */
  const turn = (n: number, files: string[], commit?: string): Checkpoint => ({
    turn: n,
    prompt: `turn ${n}`,
    at: 0,
    files,
    branch: "main",
    moved_git: false,
    commit: commit ?? null,
  });

  it("says nothing when there is nothing earlier to strand", () => {
    // The ordinary case. A caveat on every commit is one nobody reads.
    const turns = [turn(1, ["a.txt"])];
    expect(commitCaveats(turns, turns[0])).toEqual([]);
  });

  it("says nothing when everything earlier is already committed", () => {
    const turns = [turn(1, ["a.txt"], "aaaaaaa"), turn(2, ["b.txt"])];
    expect(commitCaveats(turns, turns[1])).toEqual([]);
  });

  it("names the uncommitted turn this one would jump ahead of", () => {
    // Committing turn 3 and then turn 5 leaves turn 4 in the tree,
    // uncommitted, now sitting under a commit it is not in.
    const turns = [turn(1, ["a.txt"], "aaaaaaa"), turn(2, ["b.txt"]), turn(3, ["c.txt"])];
    const [caveat] = commitCaveats(turns, turns[2]);
    expect(caveat).toContain("Turn 2");
    expect(caveat).not.toContain("Turn 1");
    expect(caveat).toContain("ahead of work it came after");
  });

  it("warns harder when the earlier turn touched the same file", () => {
    // `git commit -- <paths>` commits what those paths hold now, so turn 2's
    // edits to a shared file go in wearing turn 3's message. The commit is
    // wrong about its own contents, not merely out of order.
    const turns = [turn(1, ["a.txt"]), turn(2, ["a.txt", "b.txt"])];
    const [caveat] = commitCaveats(turns, turns[1]);
    expect(caveat).toContain("a.txt");
    expect(caveat).toContain("what those files hold now");
  });

  it("agrees in number with however many turns it names", () => {
    const turns = [turn(1, ["a.txt"]), turn(2, ["b.txt"]), turn(3, ["c.txt"])];
    expect(commitCaveats(turns, turns[2])[0]).toContain("Turns 1, 2");
    expect(commitCaveats(turns, turns[1])[0]).toContain("Turn 1 changed");
  });

  it("ignores later turns entirely", () => {
    // They are not underneath this commit; they are after it, which is where
    // uncommitted work is supposed to be.
    const turns = [turn(1, ["a.txt"]), turn(2, ["b.txt"])];
    expect(commitCaveats(turns, turns[0])).toEqual([]);
  });
});
