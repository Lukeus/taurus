// @vitest-environment jsdom
//
// The static tests in `ChangesDrawer.test.tsx` ask one question: what does the
// first paint look like. Everything this file covers happens after that — the
// turn list arrives, a turn is expanded, its diff is fetched, a commit is made
// and reported back. None of it exists in a first paint.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { ChangesDrawer } from "./ChangesDrawer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const TURN = {
  turn: 1,
  prompt: "rename the widget",
  at: Math.floor(Date.now() / 1000),
  files: ["src/widget.rs"],
};

/** A second turn, uncommitted, so a commit of the first has something to strand. */
const LATER_TURN = {
  turn: 2,
  prompt: "tidy the caller",
  at: Math.floor(Date.now() / 1000),
  files: ["src/main.rs"],
};

const DIFF = {
  kind: "diff",
  diff: {
    path: "src/widget.rs",
    created: false,
    deleted: false,
    added: 1,
    removed: 1,
    elided: 0,
    hunks: [
      {
        lines: [
          { kind: "removed", text: "let old = 1;", old_line: 1, new_line: null },
          { kind: "added", text: "let new = 2;", old_line: null, new_line: 1 },
        ],
      },
    ],
  },
};

/**
 * Answers each Tauri command with whatever the test set for it.
 *
 * Keyed by command name rather than call order, because the drawer fires the
 * checkpoint list and the repository status together and neither is promised
 * to land first.
 */
function backend(replies: Record<string, unknown>) {
  invoke.mockImplementation((command: string) => {
    if (command in replies) {
      const reply = replies[command];
      return reply instanceof Error
        ? Promise.reject(reply)
        : Promise.resolve(reply);
    }
    return Promise.resolve([]);
  });
}

/** Mounts the drawer and flushes the effects its open fires. */
async function open(sessionId = "s1") {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const render = (id: string) =>
    act(async () => {
      root.render(<ChangesDrawer sessionId={id} busy={false} onClose={() => {}} />);
    });
  await render(sessionId);
  return {
    host,
    /** Moves the same mounted pane to another conversation, as App does. */
    switchTo: render,
    html: () => host.innerHTML,
    // A diff line is one span per run of syntax now, and another wherever the
    // intra-line mark starts and stops. Anything asking whether a line of code
    // is on screen has to read it the way a person does.
    text: () => host.textContent ?? "",
    /** Clicks the first button whose visible text matches. */
    click: async (text: string) => {
      const button = [...host.querySelectorAll("button")].find((b) =>
        (b.textContent ?? "").includes(text),
      );
      if (!button) throw new Error(`no button reading "${text}"`);
      await act(async () => button.click());
    },
    type: async (value: string) => {
      const input = host.querySelector("input");
      if (!input) throw new Error("no message field");
      await act(async () => {
        // React tracks the last value it set, so assigning through the
        // prototype setter is what makes it see this as a change.
        const setter = Object.getOwnPropertyDescriptor(
          HTMLInputElement.prototype,
          "value",
        )?.set;
        setter?.call(input, value);
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
    },
    unmount: () => act(() => root.unmount()),
  };
}

beforeEach(() => invoke.mockReset());
afterEach(() => {
  document.body.innerHTML = "";
});

describe("confirming a rewind", () => {
  const REWIND = {
    restored: [{ action: "reverted", path: "src/main.rs" }],
    warnings: [],
  };

  it("sends one rewind however many times it is pressed", async () => {
    // The write this pane exists to keep from being a single click must not
    // become two: a second press while the first is in flight is a second
    // rewind racing it over the same files.
    const writes: unknown[] = [];
    invoke.mockImplementation(
      (command: string, args?: { dryRun?: boolean }) => {
        if (command === "list_checkpoints") {
          return Promise.resolve([TURN, LATER_TURN]);
        }
        if (command === "rewind_to") {
          if (args?.dryRun) return Promise.resolve(REWIND);
          writes.push(args);
          return new Promise(() => {});
        }
        return Promise.resolve([]);
      },
    );
    const ui = await open();
    await ui.click("Rewind to before this");
    const confirm = () =>
      ui.host.querySelector(".rewind-plan .danger") as HTMLButtonElement;
    await act(async () => confirm().click());
    await act(async () => confirm().click());

    expect(writes).toHaveLength(1);
    ui.unmount();
  });

  it("shows the plan for the turn asked about last", async () => {
    // Two quick presses are two requests, and the one that answers last is
    // not always the one asked last.
    const answers: Record<number, (value: unknown) => void> = {};
    invoke.mockImplementation(
      (command: string, args?: { turn?: number }) => {
        if (command === "list_checkpoints") {
          return Promise.resolve([TURN, LATER_TURN]);
        }
        if (command === "rewind_to") {
          return new Promise((resolve) => {
            answers[args?.turn ?? 0] = resolve;
          });
        }
        return Promise.resolve([]);
      },
    );
    const ui = await open();
    const previews = () =>
      [...ui.host.querySelectorAll("button")].filter((b) =>
        (b.textContent ?? "").includes("Rewind to before this"),
      );
    // Newest first, so turn 2 and then turn 1.
    await act(async () => previews()[0].click());
    await act(async () => previews()[1].click());
    await act(async () => answers[1](REWIND));
    await act(async () => answers[2](REWIND));

    expect(ui.text()).toContain("Rewind to before turn 1");
    expect(ui.text()).not.toContain("Rewind to before turn 2");
    ui.unmount();
  });
});

describe("switching conversations", () => {
  const REWIND = {
    restored: [{ action: "reverted", path: "src/main.rs" }],
    warnings: [],
  };

  it("drops a rewind plan made in the conversation it left", async () => {
    // The pane is not remounted when the conversation changes. A plan kept
    // across the switch is offered against the next conversation's turn of
    // the same number, and its button rewinds that conversation instead.
    backend({
      list_checkpoints: [TURN, LATER_TURN],
      repo_status: { repository: false },
      rewind_to: REWIND,
    });
    const ui = await open("s1");
    await ui.click("Rewind to before this");
    expect(ui.text()).toContain("Rewind to before turn 2");

    await ui.switchTo("s2");
    expect(ui.text()).not.toContain("Rewind to before turn 2");
    expect(ui.text()).not.toContain("This cannot be undone");
    ui.unmount();
  });

  it("ignores a preview that answers after the switch", async () => {
    // The same failure by another road: the question was asked in one
    // conversation and answered once the pane was showing the next.
    let answer: (value: unknown) => void = () => {};
    invoke.mockImplementation((command: string) => {
      if (command === "list_checkpoints") {
        return Promise.resolve([TURN, LATER_TURN]);
      }
      if (command === "rewind_to") {
        return new Promise((resolve) => {
          answer = resolve;
        });
      }
      return Promise.resolve([]);
    });
    const ui = await open("s1");
    await ui.click("Rewind to before this");
    await ui.switchTo("s2");
    await act(async () => answer(REWIND));

    expect(ui.text()).not.toContain("This cannot be undone");
    ui.unmount();
  });
});

describe("reading a turn", () => {
  it("fetches nothing until a turn is expanded", async () => {
    // A conversation of thirty turns would otherwise build thirty diffs to
    // draw a drawer that shows one.
    backend({ list_checkpoints: [TURN], repo_status: { repository: false } });
    const ui = await open();

    expect(invoke.mock.calls.map(([c]) => c)).not.toContain("turn_changes");

    await ui.click("View changes");
    expect(invoke.mock.calls.map(([c]) => c)).toContain("turn_changes");
    ui.unmount();
  });

  it("shows the diff of what that turn changed", async () => {
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: false },
      turn_changes: [DIFF],
    });
    const ui = await open();
    await ui.click("View changes");

    expect(ui.text()).toContain("let old = 1;");
    expect(ui.text()).toContain("let new = 2;");
    ui.unmount();
  });

  it("names a file it could not diff rather than leaving it out", async () => {
    // The same files a rewind reports as skipped. Dropping them would make the
    // turn look smaller than it was.
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: false },
      turn_changes: [
        {
          kind: "opaque",
          path: "assets/logo.png",
          reason: "was not text when it was recorded",
        },
      ],
    });
    const ui = await open();
    await ui.click("View changes");

    expect(ui.html()).toContain("assets/logo.png");
    expect(ui.html()).toContain("was not text when it was recorded");
    ui.unmount();
  });
});

describe("committing a turn", () => {
  it("offers no commit for a workspace that is not a repository", async () => {
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: false },
      turn_changes: [DIFF],
    });
    const ui = await open();
    await ui.click("View changes");

    expect(ui.html()).not.toContain("Commit this turn");
    expect(ui.html()).toContain("not a git repository");
    ui.unmount();
  });

  it("seeds the message from what the turn was asked to do", async () => {
    // The prompt and the commit message agree often enough to be a useful
    // start — and the field is editable because they agree rarely enough that
    // committing one unread is a bad habit to build.
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: true, branch: "main" },
      turn_changes: [DIFF],
    });
    const ui = await open();
    await ui.click("View changes");

    expect(ui.host.querySelector("input")?.value).toBe("rename the widget");
    ui.unmount();
  });

  it("sends the edited message and reports the commit back", async () => {
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: true, branch: "main" },
      turn_changes: [DIFF],
      commit_turn: {
        sha: "a1b2c3d",
        subject: "rename Widget to Gadget",
        files: ["src/widget.rs"],
        skipped: [],
      },
    });
    const ui = await open();
    await ui.click("View changes");
    await ui.type("rename Widget to Gadget");
    await ui.click("Commit this turn");

    const call = invoke.mock.calls.find(([c]) => c === "commit_turn");
    expect(call?.[1]).toMatchObject({
      sessionId: "s1",
      turn: 1,
      message: "rename Widget to Gadget",
    });
    expect(ui.html()).toContain("a1b2c3d");
    ui.unmount();
  });

  it("says which of the turn's files did not go in", async () => {
    // A commit that quietly covered three of a turn's four files is the
    // failure this whole surface exists to prevent.
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: true, branch: "main" },
      turn_changes: [DIFF],
      commit_turn: {
        sha: "a1b2c3d",
        subject: "rename the widget",
        files: ["src/widget.rs"],
        skipped: [
          {
            path: ".env",
            reason: "is ignored by git, so it is not in the repository to commit",
          },
        ],
      },
    });
    const ui = await open();
    await ui.click("View changes");
    await ui.click("Commit this turn");

    expect(ui.html()).toContain(".env");
    expect(ui.html()).toContain("ignored by git");
    ui.unmount();
  });

  it("shows the reason a commit was refused and keeps the field", async () => {
    // The refusal carries every reason it collected, and it is the whole point
    // that the user gets to read it rather than watch the button do nothing.
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: true, branch: "main" },
      turn_changes: [DIFF],
      commit_turn: new Error(
        "Nothing to commit from that turn: src/widget.rs already matches the last commit.",
      ),
    });
    const ui = await open();
    await ui.click("View changes");
    await ui.click("Commit this turn");

    expect(ui.html()).toContain("already matches the last commit");
    expect(ui.host.querySelector("input")).not.toBeNull();
    ui.unmount();
  });

  it("names the branch a commit would land on", async () => {
    // A detached HEAD is the case worth catching: a commit made there is hard
    // to find again, and the drawer says so before the button is pressed.
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: true, head: "9f8e7d6" },
      turn_changes: [DIFF],
    });
    const ui = await open();

    expect(ui.html()).toContain("detached at 9f8e7d6");
    ui.unmount();
  });
});

describe("what a rewind cannot put back", () => {
  it("shows the warnings between the file list and the button", async () => {
    // The whole point of moving these into the log: the sweep said it when
    // the command ran, which can be days before anyone reaches for undo.
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: false },
      rewind_to: {
        restored: [{ action: "reverted", path: "src/widget.rs" }],
        warnings: [
          "Turn 1 moved git's own state. `git reflog` is the way back to where HEAD was.",
        ],
      },
    });
    const ui = await open();

    await ui.click("Rewind to before this");
    expect(ui.html()).toContain("git reflog");
    expect(ui.html()).toContain("not undone");
    ui.unmount();
  });

  it("keeps saying them after the rewind has run", async () => {
    // A commit left pointing at a tree that no longer exists does not stop
    // being a problem because the rewind finished.
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: false },
      rewind_to: {
        restored: [{ action: "reverted", path: "src/widget.rs" }],
        warnings: ["Turn 1 was committed as abc1234."],
      },
    });
    const ui = await open();

    await ui.click("Rewind to before this");
    await ui.click("Rewind to before turn 1");
    expect(ui.html()).toContain("abc1234");
    expect(ui.html()).toContain("still to sort out");
    ui.unmount();
  });

  it("says nothing extra when there is nothing extra to say", async () => {
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: false },
      rewind_to: {
        restored: [{ action: "reverted", path: "src/widget.rs" }],
        warnings: [],
      },
    });
    const ui = await open();

    await ui.click("Rewind to before this");
    expect(ui.html()).not.toContain("not undone");
    ui.unmount();
  });
});

describe("a turn that is already kept", () => {
  it("labels it with the commit it is in", async () => {
    // Read from the log rather than from a click, so it survives closing the
    // drawer and reopening the conversation.
    backend({
      list_checkpoints: [{ ...TURN, commit: "abc1234" }],
      repo_status: { repository: false },
    });
    const ui = await open();

    expect(ui.html()).toContain("committed abc1234");
    ui.unmount();
  });

  it("warns before committing one turn over an earlier uncommitted one", async () => {
    backend({
      list_checkpoints: [TURN, LATER_TURN],
      repo_status: { repository: true, branch: "main" },
      turn_changes: [DIFF],
    });
    const ui = await open();

    // Turn 2 is drawn first — the list is newest-first — so expanding it is
    // the case that has turn 1 sitting uncommitted underneath.
    await ui.click("View changes");
    expect(ui.html()).toContain("Turn 1 changed files");
    expect(ui.html()).toContain("out of order");
    ui.unmount();
  });
});

describe("reviewing a turn", () => {
  const REPORT = {
    turn: 1,
    files: 1,
    model: "qwen3.6:27b",
    text: "The rename misses the caller in `src/main.rs:12`.",
    omitted: [],
  };

  it("offers no review until a turn's diff is on screen", async () => {
    // There is nothing to hand a reviewer before the diff has been read, and a
    // button that starts a model round trip must not be reachable from a list.
    backend({ list_checkpoints: [TURN], repo_status: { repository: false } });
    const drawer = await open();
    expect(drawer.text()).not.toContain("Review this turn");
  });

  it("reports what a review found, and what produced it", async () => {
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: false },
      turn_changes: [DIFF],
      review_turn: REPORT,
    });
    const drawer = await open();
    await drawer.click("View changes");
    await drawer.click("Review this turn");

    expect(drawer.text()).toContain("misses the caller");
    // Which model, and the sentence that keeps this from being read as a
    // verdict: it never saw what was asked for.
    expect(drawer.text()).toContain("qwen3.6:27b");
    expect(drawer.text()).toContain("cannot know what was asked for");
  });

  it("names the files the reviewer was not shown", async () => {
    // A review that covered one of two files and did not say so reads as a
    // clean bill of health for both.
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: false },
      turn_changes: [DIFF],
      review_turn: { ...REPORT, omitted: ["logo.png"] },
    });
    const drawer = await open();
    await drawer.click("View changes");
    await drawer.click("Review this turn");

    expect(drawer.text()).toContain("not reviewed");
    expect(drawer.text()).toContain("logo.png");
  });

  it("keeps a failed review out of the commit box's error", async () => {
    // The two can be in flight at once, and a review that could not reach the
    // model must not read as a commit that failed.
    backend({
      list_checkpoints: [TURN],
      repo_status: { repository: true, branch: "main" },
      turn_changes: [DIFF],
      review_turn: new Error("provider unreachable"),
    });
    const drawer = await open();
    await drawer.click("View changes");
    await drawer.click("Review this turn");

    expect(drawer.text()).toContain("provider unreachable");
    // The commit path is untouched and still offered.
    expect(drawer.text()).toContain("Commit this turn");
  });
});
