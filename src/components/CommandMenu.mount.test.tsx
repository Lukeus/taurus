// @vitest-environment jsdom
//
// The pure matching rules are tested next door without a DOM. What is left is
// the row itself, and the one thing on it that is new: a `/name` no longer says
// what it will do. A skill runs a procedure in this turn; an agent hands the
// job to a second model with its own context. That difference reaches the user
// only through the badge, so the badge is worth a render.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandSummary } from "../lib/api";
import { CommandMenu } from "./CommandMenu";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mount = (node: React.ReactNode) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => root.render(node));
  return { host, unmount: () => act(() => root.unmount()) };
};

afterEach(() => {
  document.body.innerHTML = "";
});

const commands: CommandSummary[] = [
  { name: "review", kind: "skill", when_to_use: "when reviewing a change" },
  { name: "reviewer", kind: "agent", when_to_use: "Reviews a diff for bugs" },
];

describe("the command menu", () => {
  it("says which kind each row is", () => {
    const { host, unmount } = mount(
      <CommandMenu commands={commands} active={0} onPick={() => {}} />,
    );

    const rows = [...host.querySelectorAll(".command-row")];
    expect(rows.map((r) => r.querySelector(".command-name")?.textContent)).toEqual(
      ["/review", "/reviewer"],
    );
    expect(rows.map((r) => r.querySelector(".command-kind")?.textContent)).toEqual(
      ["skill", "agent"],
    );
    // Tinted, and only on the row that spends a second model.
    expect(rows[1].querySelector(".command-kind")?.className).toContain("agent");
    unmount();
  });

  it("marks the highlighted row for a screen reader as well as the eye", () => {
    const { host, unmount } = mount(
      <CommandMenu commands={commands} active={1} onPick={() => {}} />,
    );

    const selected = [...host.querySelectorAll('[role="option"]')].map((o) =>
      o.getAttribute("aria-selected"),
    );
    expect(selected).toEqual(["false", "true"]);
    unmount();
  });

  it("completes the name on mousedown rather than click", () => {
    // Click fires after blur, and blur takes the caret out of the composer the
    // user is mid-word in. The row has to act before that.
    const picked: string[] = [];
    const { host, unmount } = mount(
      <CommandMenu
        commands={commands}
        active={0}
        onPick={(c) => picked.push(c.name)}
      />,
    );

    const row = host.querySelectorAll(".command-row")[1];
    act(() => {
      row.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
    expect(picked).toEqual(["reviewer"]);
    unmount();
  });

  it("renders nothing when there is nothing to offer", () => {
    const { host, unmount } = mount(
      <CommandMenu commands={[]} active={0} onPick={() => {}} />,
    );
    expect(host.innerHTML).toBe("");
    unmount();
  });
});
