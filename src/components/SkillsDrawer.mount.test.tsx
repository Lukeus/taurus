// @vitest-environment jsdom
//
// The rest of the suite renders components to a string, which asks a component
// one question: what does your first paint look like. That misses everything
// about staying mounted — and the bug this file exists for was invisible to all
// of it. Here the drawer is mounted into a real document, the way the app
// mounts it.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import { SkillsDrawer } from "./SkillsDrawer";
import { useStore } from "../state/store";

// Tells React this is a test renderer, so `act` flushes effects instead of
// warning that it cannot.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mount = (node: React.ReactNode) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  // React reports a render-phase throw to console.error before rethrowing it.
  // Silenced so a deliberately-failing render does not look like a crash in
  // the test output.
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

describe("opening the drawer", () => {
  it("mounts and stays mounted with nothing loaded yet", () => {
    // The state the drawer is always opened from at least once: launched, no
    // status back from the backend. Selecting `s.status?.mcp_servers ?? []`
    // hands React a brand new array on every snapshot here, and it tears the
    // tree down rather than loop — the drawer opened onto nothing at all.
    useStore.setState({ status: null });

    const { html, unmount } = mount(<SkillsDrawer onClose={() => {}} />);
    expect(html).toContain("Skills");
    expect(html).toContain("MCP servers");
    unmount();
  });

  it("stays mounted once a status with problems has arrived", () => {
    // The other half of it: `.filter(…)` allocates on every call whether or not
    // anything matched, so a loaded status was no safer than an empty one.
    useStore.setState({
      status: {
        workspace: "/Users/x/code/taurus",
        problems: [
          { source: "skills", message: "release-notes: python3 not found" },
          { source: "provider", message: "not this drawer's problem" },
        ],
        mcp_servers: [
          { name: "filesystem", connected: true, tool_count: 11, error: null },
        ],
      } as never,
    });

    const { html, unmount } = mount(<SkillsDrawer onClose={() => {}} />);
    expect(html).toContain("filesystem");
    expect(html).toContain("python3 not found");
    // Settings owns provider failures; this drawer must not repeat them.
    expect(html).not.toContain("not this drawer's problem");
    unmount();
  });
});
