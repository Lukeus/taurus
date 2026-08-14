// @vitest-environment jsdom
//
// Same reason SkillsDrawer has one of these: rendering to a string only asks
// what the first paint looks like, and the zustand-snapshot bug that kept that
// drawer from opening at all was invisible to every string test in the suite.
// This drawer reads the store the same way, so it gets the same guard.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import { AgentsDrawer } from "./AgentsDrawer";
import { useStore } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mount = (node: React.ReactNode) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
  try {
    act(() => root.render(node));
  } finally {
    quiet.mockRestore();
  }
  return { html: host.innerHTML, unmount: () => act(() => root.unmount()) };
};

afterEach(() => {
  document.body.innerHTML = "";
});

describe("opening the agents drawer", () => {
  it("mounts and stays mounted with nothing loaded yet", () => {
    useStore.setState({ status: null });

    const { html, unmount } = mount(<AgentsDrawer onClose={() => {}} />);
    expect(html).toContain("Agents");
    // The shadowing rule, which is the one thing about this list that cannot
    // be worked out by looking at it.
    expect(html).toContain("overrides a built-in");
    // And a way in for someone who has never written the frontmatter.
    expect(html).toContain("New agent");
    unmount();
  });

  it("shows only this drawer's problems once a status has arrived", () => {
    useStore.setState({
      status: {
        workspace: "/Users/x/code/taurus",
        problems: [
          { source: "agents", message: "reviewer.md: rename one to match" },
          { source: "skills", message: "not this drawer's problem" },
        ],
      } as never,
    });

    const { html, unmount } = mount(<AgentsDrawer onClose={() => {}} />);
    expect(html).toContain("rename one to match");
    // The skills drawer owns skill failures; this one must not repeat them.
    expect(html).not.toContain("not this drawer's problem");
    unmount();
  });
});
