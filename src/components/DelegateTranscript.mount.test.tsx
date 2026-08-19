// @vitest-environment jsdom
//
// The drawer's whole job happens after the first paint: it asks the backend for
// a conversation that is not in the store, turns the messages into entries, and
// draws them with the same transcript the app uses. A static render would show
// only the spinner.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { DelegateTranscript } from "./DelegateTranscript";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

Element.prototype.scrollIntoView = () => {};

let cleanup: (() => void)[] = [];

// A block body, not an expression: `mockReset` returns the mock, vitest treats
// a function returned from `beforeEach` as a teardown callback, and calling it
// would invoke the mock again — rejecting into nobody's `catch`.
beforeEach(() => {
  invoke.mockReset();
});

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

async function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <DelegateTranscript
        sessionId="parent1"
        subagentId="child1"
        agent="explorer"
        onClose={() => {}}
      />,
    );
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return host;
}

describe("a delegate's transcript", () => {
  it("draws the conversation the parent never showed", async () => {
    invoke.mockResolvedValue([
      { role: "user", content: [{ type: "text", text: "Find the parser." }] },
      {
        role: "assistant",
        content: [{ type: "text", text: "It is in src/parse.rs." }],
      },
    ]);

    const host = await mount();

    expect(invoke).toHaveBeenCalledWith("read_subagent_transcript", {
      sessionId: "parent1",
      subagentId: "child1",
    });
    expect(host.textContent).toContain("Find the parser.");
    expect(host.textContent).toContain("It is in src/parse.rs.");
    // Named for what it was, since that is what the parent's card offered.
    expect(host.querySelector(".drawer-head h2")?.textContent).toBe("explorer");
  });

  it("says so when the transcript cannot be read", async () => {
    // A reference can outlive its file — a deleted conversation takes its
    // delegates with it — and the drawer has to say that rather than sit on a
    // spinner forever.
    invoke.mockRejectedValue(
      new Error("no sub-agent 'child1' under session 'parent1'"),
    );

    const host = await mount();

    expect(host.textContent).toContain("no sub-agent");
    expect(host.querySelector(".transcript")).toBeNull();
    /*
     * And says it as a failure. This used to render through `.drawer-empty`,
     * the same class an empty result uses — so a transcript that could not be
     * found was indistinguishable from one that was found and held nothing.
     */
    expect(host.querySelector(".drawer-error")).not.toBeNull();
    expect(host.querySelector(".drawer-empty")).toBeNull();
  });
});
